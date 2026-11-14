#!/bin/bash
# Real-kernel check of transparent capture (Linux), without root and without touching the host
# network: Akev (a release build), upstream servers, the nftables / iptables rules and the
# clients all live in a private user + network namespace (netns-inner.sh). The commands come
# from Akev's own plan API and run verbatim ("sudo " stripped: inside the namespace we hold
# CAP_NET_ADMIN). Per-app capture uses your real systemd user manager (systemctl --user /
# systemd-run --user) to create and stop akev-capture.slice.
#
#   cargo build --release -p akev-cli && scripts/transparent/verify-netns.sh [path/to/akev]
#
# Needs: unprivileged user namespaces, nft, iptables(-nft), curl, openssl, python3, systemd.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
AKEV="${1:-$HERE/../../target/release/akev}"
[ -x "$AKEV" ] || { echo "no Akev binary at $AKEV (cargo build --release -p akev-cli)"; exit 2; }
if systemctl --user is-active --quiet akev-capture.slice; then
  echo "akev-capture.slice is in use (real per-app capture?): not touching it"; exit 2
fi
W="$(mktemp -d "${TMPDIR:-/tmp}/akev-transparent.XXXX")"
TOKEN=akev-transparent-verification-token
unshare -U --map-current-user --keep-caps -n --fork bash "$HERE/netns-inner.sh" "$W" "$AKEV" &
OUTER=$!
for _ in $(seq 200); do [ -e "$W/ready" ] && break; sleep 0.1; done
[ -e "$W/ready" ] || { echo "namespace didn't come up"; cat "$W/akev.log"; kill $OUTER; exit 1; }
PID=$(cat "$W/ns.pid")
NS=(nsenter -t "$PID" -U -n --preserve-credentials --keep-caps)
api() { "${NS[@]}" curl -sS -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:18456$1" "${@:2}"; }
plan() { api /api/transparent/plan -X POST -H 'content-type: application/json' -d "$1"; }
field() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
in_ns() { "${NS[@]}" bash -c "$1"; }
CHECK="curl -sS -o /dev/null -w '%{http_code}\n' --max-time 4 --resolve akev.cert:80:192.0.2.1 http://akev.cert/"
fail=0
expect() { if [ "$2" = "$3" ]; then echo "  ok: $1 ($3)"; else echo "  FAIL: $1: expected [$2] got [$3]"; fail=1; fi; }

echo "== listener before any rules"
api /api/transparent | field "(d['supported'], d['listener']['listening'], d['listener']['addresses'], d['listener']['mark'], d['listener'].get('markError'), d['system']['uid'], d['system']['userManagerCgroup'])"
expect "check before rules (not redirected)" "000" "$(in_ns "$CHECK" 2>/dev/null)"

echo "== machine-wide, Akev excluded by firewall mark"
plan '{"mode":"machine","exclude":"mark"}' > "$W/plan-machine.json"
field "d['rules']" < "$W/plan-machine.json"
cmd=$(field "d['setup'][-1]['command']" < "$W/plan-machine.json")
in_ns "${cmd#sudo }" && echo "  rules loaded"
test_cmd=$(field "d['test']['command']" < "$W/plan-machine.json")
expect "generated check command" "200" "$(in_ns "$test_cmd")"
expect "plain HTTP to 10.99.0.1:80" "hello from the upstream" "$(in_ns "curl -sS --max-time 4 --resolve plain.test:80:10.99.0.1 http://plain.test/hello.txt")"
https=$(in_ns "curl -sS --max-time 4 --cacert '$W/data/ca.pem' --resolve secure.test:443:10.99.0.1 https://secure.test/ -o /dev/null -w '%{http_code}'")
expect "HTTPS intercepted with Akev's certificate for secure.test" "200" "$https"
issuer=$(in_ns "curl -sS -v --max-time 4 --cacert '$W/data/ca.pem' --resolve secure.test:443:10.99.0.1 https://secure.test/ -o /dev/null 2>&1 | grep -i 'issuer:' | head -1")
echo "  client saw: $issuer"
expect "unredirected port 8080 stays unredirected" "000" "$(in_ns "curl -sS -o /dev/null -w '%{http_code}' --max-time 2 http://10.99.0.1:8080/" 2>/dev/null)"
api /api/transparent > "$W/status-machine.json"
field "(d['active'], d['listener']['redirected'], d['listener']['direct'], d['listener']['loops'], [(r.get('originalDestination'), r.get('process')) for r in d['listener']['recent']])" < "$W/status-machine.json"
expect "no loops (Akev's marked sockets skip the rules)" "0" "$(field "d['listener']['loops']" < "$W/status-machine.json")"
api '/api/exchanges' > "$W/exchanges.json"
field "[(s['method'], s['host'], s['port'], s['path'], s['status'], s.get('source')) for s in d]" < "$W/exchanges.json"
for id in $(field "' '.join(str(s['id']) for s in d)" < "$W/exchanges.json"); do
  api "/api/exchanges/$id" | field "(d['request']['url'], d['client']['via'], (d['client'].get('tls') or {}).get('sni'), (d['client'].get('process') or {}).get('name'))"
done
cmd=$(field "d['teardown'][0]['command']" < "$W/plan-machine.json")
in_ns "${cmd#sudo }" && echo "  teardown: $cmd"
expect "check after teardown" "000" "$(in_ns "$CHECK" 2>/dev/null)"

echo "== iptables backend, machine-wide (mark)"
plan '{"mode":"machine","exclude":"mark","backend":"iptables","ports":"custom","customPorts":[80]}' > "$W/plan-ipt.json"
cmd=$(field "d['setup'][-1]['command']" < "$W/plan-ipt.json")
in_ns "$(echo "$cmd" | sed 's/sudo //g')" && echo "  rules loaded"
"${NS[@]}" iptables -t nat -S 2>/dev/null | sed 's/^/    /'
expect "iptables check" "200" "$(in_ns "$(field "d['test']['command']" < "$W/plan-ipt.json")")"
expect "iptables: port 443 not in the custom list" "000" "$(in_ns "curl -sS -o /dev/null -w '%{http_code}' --max-time 2 --resolve akev.cert:443:192.0.2.1 -k https://akev.cert/" 2>/dev/null)"
cmd=$(field "d['teardown'][0]['command']" < "$W/plan-ipt.json")
in_ns "$(echo "$cmd" | sed 's/sudo //g')" && echo "  iptables teardown done"
expect "iptables check after teardown" "000" "$(in_ns "$CHECK" 2>/dev/null)"

echo "== per-user (nobody): rules load; Akev's own user is not captured"
plan '{"mode":"user","user":"nobody"}' > "$W/plan-user.json"
field "d['rules']" < "$W/plan-user.json" | grep skuid
cmd=$(field "d['setup'][-1]['command']" < "$W/plan-user.json")
in_ns "${cmd#sudo }" && echo "  rules loaded"
expect "uid 1000 not redirected by a rule for nobody" "000" "$(in_ns "$CHECK" 2>/dev/null)"
cmd=$(field "d['teardown'][0]['command']" < "$W/plan-user.json")
in_ns "${cmd#sudo }" && echo "  teardown done"

echo "== per-app: only programs in the akev-capture.slice cgroup"
plan "{\"mode\":\"app\",\"command\":\"curl -sS -o /dev/null -w %{http_code} --max-time 4 --resolve secure.test:443:10.99.0.1 https://secure.test/\"}" > "$W/plan-app.json"
field "d['rules']" < "$W/plan-app.json"
step1=$(field "d['setup'][0]['command']" < "$W/plan-app.json")
echo "  (as you) $step1"; bash -c "$step1"
cmd=$(field "d['setup'][1]['command']" < "$W/plan-app.json")
in_ns "${cmd#sudo }" && echo "  rules loaded"
test_cmd=$(field "d['test']['command']" < "$W/plan-app.json")
expect "check inside the slice: $test_cmd" "200" "$(in_ns "$test_cmd")"
expect "same check outside the slice is not redirected" "000" "$(in_ns "$CHECK" 2>/dev/null)"
launch=$(field "d['launch']['command']" < "$W/plan-app.json")
echo "  launch: $launch"
expect "launched app: HTTPS trusted via the CA env vars, intercepted" "200" "$(in_ns "$launch")"
api /api/transparent | field "(d['active'], d['listener']['redirected'], d['listener']['loops'], [(r.get('originalDestination'), r.get('process')) for r in d['listener']['recent'][:3]])"
for step in 0 1; do
  cmd=$(field "d['teardown'][$step]['command']" < "$W/plan-app.json")
  if [ $step = 0 ]; then in_ns "${cmd#sudo }"; else bash -c "$cmd"; fi
  echo "  teardown: $cmd"
done
expect "check inside the (re-created) slice after teardown" "000" "$(in_ns "systemd-run --user --scope --quiet --slice=akev-capture.slice -- $CHECK" 2>/dev/null)"
systemctl --user stop akev-capture.slice akev.slice 2>/dev/null

touch "$W/stop"
wait $OUTER
echo "== akev log (warnings)"; grep -iE "warn|error" "$W/akev.log" | head -10
echo "artifacts: $W"
if [ $fail = 0 ]; then echo "ALL CHECKS PASSED"; else echo "SOME CHECKS FAILED"; exit 1; fi
