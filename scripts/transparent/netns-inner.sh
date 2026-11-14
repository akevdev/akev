#!/bin/bash
# Part of verify-netns.sh. Runs inside `unshare -U --map-current-user --keep-caps -n`: a
# private user + network namespace (the host's network is never touched). Sets up a fake "internet" on a dummy interface,
# upstream servers, and a release Akev daemon with the transparent listener on.
set -u
W="$1"
AKEV="$2"
echo $$ > "$W/ns.pid"
ip link set lo up
ip link add dummy0 type dummy
ip addr add 10.99.0.1/24 dev dummy0
ip link set dummy0 up
# Default route into the void: connections to the internet are only answered if redirected.
ip route add default via 10.99.0.254 dev dummy0

mkdir -p "$W/www" "$W/data"
echo "hello from the upstream" > "$W/www/hello.txt"
python3 -m http.server 80 --bind 10.99.0.1 --directory "$W/www" > "$W/http.log" 2>&1 &
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$W/up.key" -out "$W/up.crt" -days 1 -subj /CN=upstream > /dev/null 2>&1
openssl s_server -accept 443 -cert "$W/up.crt" -key "$W/up.key" -www -quiet > "$W/tls.log" 2>&1 &

cat > "$W/data/settings.json" <<'EOF'
{"proxy":{"port":18000,"ignoreUpstreamCertErrors":["*"],
  "dnsOverrides":[{"host":"secure.test","address":"10.99.0.1"},{"host":"plain.test","address":"10.99.0.1"}],
  "transparent":{"enabled":true,"port":18001,"mark":42}}}
EOF
"$AKEV" start --data-dir "$W/data" --no-open --api-port 18456 --token akev-transparent-verification-token > "$W/akev.log" 2>&1 &
for _ in $(seq 100); do
  curl -s -o /dev/null http://127.0.0.1:18456/api/status && break
  sleep 0.1
done
touch "$W/ready"
while [ ! -e "$W/stop" ]; do sleep 0.2; done
kill $(jobs -p) 2>/dev/null
wait
