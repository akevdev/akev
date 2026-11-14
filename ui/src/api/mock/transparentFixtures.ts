/**
 * Transparent capture for the mock backend: interceptor card, `GET /api/transparent` and
 * `POST /api/transparent/plan`. Plans were captured from a real Akev daemon (Linux, port 8001,
 * user paths scrubbed); the mock substitutes the port and simulates redirect activity.
 * Regenerate when the plan format changes.
 */
import type { InterceptorInfo, TransparentInfo, TransparentPlan } from '../types';

export const TRANSPARENT_INTERCEPTOR: InterceptorInfo = {
  "id": "transparent-linux",
  "name": "Transparent capture (Linux)",
  "description": "Capture apps that ignore proxy settings: the firewall redirects their connections to Akev — per app, per user or machine-wide.",
  "category": "system",
  "tags": [
    "transparent",
    "redirect",
    "nftables",
    "nft",
    "iptables",
    "firewall",
    "cgroup",
    "per-app",
    "linux",
    "everything",
    "ignores proxy",
    "system"
  ],
  "icon": "linux",
  "supported": true,
  "available": true,
  "active": false,
  "metadata": {
    "listener": {
      "enabled": true,
      "supported": true,
      "listening": true,
      "port": 8001,
      "requestedPort": 8001,
      "addresses": [
        "127.0.0.1:8001",
        "[::1]:8001"
      ],
      "mark": 0,
      "connections": 0,
      "redirected": 0,
      "direct": 0,
      "loops": 0,
      "recent": []
    },
    "system": {
      "platform": "linux",
      "uid": 1000,
      "user": "dev",
      "cgroupV2": true,
      "ownCgroup": "/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.gnome.Terminal.slice/vte-spawn.scope",
      "userManagerCgroup": "user.slice/user-1000.slice/user@1000.service",
      "nft": true,
      "iptables": true,
      "systemdRun": true,
      "executable": "/usr/bin/akev"
    },
    "slice": "akev-capture.slice"
  },
  "needsConfig": true
};

export const TRANSPARENT_SYSTEM: TransparentInfo['system'] = {
  "platform": "linux",
  "uid": 1000,
  "user": "dev",
  "cgroupV2": true,
  "ownCgroup": "/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.gnome.Terminal.slice/vte-spawn.scope",
  "userManagerCgroup": "user.slice/user-1000.slice/user@1000.service",
  "nft": true,
  "iptables": true,
  "systemdRun": true,
  "executable": "/usr/bin/akev"
};

/** Captured plans by `mode-backend` (default options otherwise; user mode for `nobody`). */
export const TRANSPARENT_PLANS: Record<string, TransparentPlan> = {
  "app-nft": {
    "mode": "app",
    "backend": "nft",
    "port": 8001,
    "summary": "Programs launched in the akev-capture.slice systemd slice",
    "setup": [
      {
        "title": "Create the capture slice",
        "command": "systemctl --user start akev-capture.slice",
        "root": false,
        "description": "Run as yourself. Programs launched in this slice (user.slice/user-1000.slice/user@1000.service/akev.slice/akev-capture.slice) are captured; it stays until you stop it or log out."
      },
      {
        "title": "Redirect connections to Akev",
        "command": "sudo nft -f - <<'EOF'\ntable inet akev\ndelete table inet akev\ntable inet akev {\n\tchain output {\n\t\ttype nat hook output priority -100; policy accept;\n\t\tip daddr 127.0.0.0/8 return\n\t\tip6 daddr ::1 return\n\t\tsocket cgroupv2 level 5 \"user.slice/user-1000.slice/user@1000.service/akev.slice/akev-capture.slice\" tcp dport { 80, 443 } redirect to :8001\n\t}\n}\nEOF",
        "root": true,
        "description": "Loads the rules (replacing earlier Akev rules) that send TCP connections to ports 80, 443 to port 8001."
      }
    ],
    "launch": {
      "title": "Launch programs inside the slice",
      "command": "systemd-run --user --scope --quiet --slice=akev-capture.slice -- env SSL_CERT_FILE=/home/dev/.local/share/akev/ca-bundle.pem REQUESTS_CA_BUNDLE=/home/dev/.local/share/akev/ca-bundle.pem CURL_CA_BUNDLE=/home/dev/.local/share/akev/ca-bundle.pem GIT_SSL_CAINFO=/home/dev/.local/share/akev/ca-bundle.pem AWS_CA_BUNDLE=/home/dev/.local/share/akev/ca-bundle.pem DENO_CERT=/home/dev/.local/share/akev/ca-bundle.pem NODE_EXTRA_CA_CERTS=/home/dev/.local/share/akev/ca.pem \"$SHELL\"",
      "root": false,
      "description": "Opens a shell whose programs are all captured. Programs already running elsewhere are not."
    },
    "test": {
      "title": "Check it works",
      "command": "systemd-run --user --scope --quiet --slice=akev-capture.slice -- curl -sS -o /dev/null -w '%{http_code}\\n' --max-time 5 --resolve akev.cert:80:192.0.2.1 http://akev.cert/",
      "root": false,
      "description": "Prints 200 when redirection works (and the connection shows up below)."
    },
    "teardown": [
      {
        "title": "Remove the redirect rules",
        "command": "sudo nft delete table inet akev",
        "root": true
      },
      {
        "title": "Stop the capture slice",
        "command": "systemctl --user stop akev-capture.slice",
        "root": false,
        "description": "Also ends programs still running in it."
      }
    ],
    "rules": "table inet akev {\n\tchain output {\n\t\ttype nat hook output priority -100; policy accept;\n\t\tip daddr 127.0.0.0/8 return\n\t\tip6 daddr ::1 return\n\t\tsocket cgroupv2 level 5 \"user.slice/user-1000.slice/user@1000.service/akev.slice/akev-capture.slice\" tcp dport { 80, 443 } redirect to :8001\n\t}\n}\n",
    "warnings": [],
    "notes": [
      "Check where a program runs with: cat /proc/<pid>/cgroup (it must end in /akev-capture.slice/run-…scope).",
      "Only TCP is redirected: QUIC / HTTP/3 (UDP 443) isn't, and browsers fall back to HTTPS over TCP when QUIC fails — block UDP 443 if a program keeps using QUIC.",
      "HTTPS is intercepted only if the program trusts Akev's CA (browsers use their own trust store: use the browser interceptors or trust the CA system-wide).",
      "While the rules are loaded and Akev isn't running, the redirected connections fail: run the teardown commands when you're done.",
      "The commands use a here-document: paste them into bash or zsh (in fish, run bash first)."
    ]
  },
  "app-iptables": {
    "mode": "app",
    "backend": "iptables",
    "port": 8001,
    "summary": "Programs launched in the akev-capture.slice systemd slice",
    "setup": [
      {
        "title": "Create the capture slice",
        "command": "systemctl --user start akev-capture.slice",
        "root": false,
        "description": "Run as yourself. Programs launched in this slice (user.slice/user-1000.slice/user@1000.service/akev.slice/akev-capture.slice) are captured; it stays until you stop it or log out."
      },
      {
        "title": "Redirect connections to Akev",
        "command": "sudo iptables -t nat -N AKEV 2>/dev/null || sudo iptables -t nat -F AKEV\nsudo iptables -t nat -A AKEV -d 127.0.0.0/8 -j RETURN\nsudo iptables -t nat -A AKEV -p tcp -m multiport --dports 80,443 -m cgroup --path user.slice/user-1000.slice/user@1000.service/akev.slice/akev-capture.slice -j REDIRECT --to-ports 8001\nsudo iptables -t nat -C OUTPUT -p tcp -j AKEV 2>/dev/null || sudo iptables -t nat -A OUTPUT -p tcp -j AKEV\nsudo ip6tables -t nat -N AKEV 2>/dev/null || sudo ip6tables -t nat -F AKEV\nsudo ip6tables -t nat -A AKEV -d ::1/128 -j RETURN\nsudo ip6tables -t nat -A AKEV -p tcp -m multiport --dports 80,443 -m cgroup --path user.slice/user-1000.slice/user@1000.service/akev.slice/akev-capture.slice -j REDIRECT --to-ports 8001\nsudo ip6tables -t nat -C OUTPUT -p tcp -j AKEV 2>/dev/null || sudo ip6tables -t nat -A OUTPUT -p tcp -j AKEV",
        "root": true,
        "description": "Loads the rules (replacing earlier Akev rules) that send TCP connections to ports 80, 443 to port 8001."
      }
    ],
    "launch": {
      "title": "Launch programs inside the slice",
      "command": "systemd-run --user --scope --quiet --slice=akev-capture.slice -- env SSL_CERT_FILE=/home/dev/.local/share/akev/ca-bundle.pem REQUESTS_CA_BUNDLE=/home/dev/.local/share/akev/ca-bundle.pem CURL_CA_BUNDLE=/home/dev/.local/share/akev/ca-bundle.pem GIT_SSL_CAINFO=/home/dev/.local/share/akev/ca-bundle.pem AWS_CA_BUNDLE=/home/dev/.local/share/akev/ca-bundle.pem DENO_CERT=/home/dev/.local/share/akev/ca-bundle.pem NODE_EXTRA_CA_CERTS=/home/dev/.local/share/akev/ca.pem \"$SHELL\"",
      "root": false,
      "description": "Opens a shell whose programs are all captured. Programs already running elsewhere are not."
    },
    "test": {
      "title": "Check it works",
      "command": "systemd-run --user --scope --quiet --slice=akev-capture.slice -- curl -sS -o /dev/null -w '%{http_code}\\n' --max-time 5 --resolve akev.cert:80:192.0.2.1 http://akev.cert/",
      "root": false,
      "description": "Prints 200 when redirection works (and the connection shows up below)."
    },
    "teardown": [
      {
        "title": "Remove the redirect rules",
        "command": "sudo iptables -t nat -D OUTPUT -p tcp -j AKEV\nsudo iptables -t nat -F AKEV\nsudo iptables -t nat -X AKEV\nsudo ip6tables -t nat -D OUTPUT -p tcp -j AKEV\nsudo ip6tables -t nat -F AKEV\nsudo ip6tables -t nat -X AKEV",
        "root": true
      },
      {
        "title": "Stop the capture slice",
        "command": "systemctl --user stop akev-capture.slice",
        "root": false,
        "description": "Also ends programs still running in it."
      }
    ],
    "rules": "iptables -t nat -N AKEV 2>/dev/null || iptables -t nat -F AKEV\niptables -t nat -A AKEV -d 127.0.0.0/8 -j RETURN\niptables -t nat -A AKEV -p tcp -m multiport --dports 80,443 -m cgroup --path user.slice/user-1000.slice/user@1000.service/akev.slice/akev-capture.slice -j REDIRECT --to-ports 8001\niptables -t nat -C OUTPUT -p tcp -j AKEV 2>/dev/null || iptables -t nat -A OUTPUT -p tcp -j AKEV\nip6tables -t nat -N AKEV 2>/dev/null || ip6tables -t nat -F AKEV\nip6tables -t nat -A AKEV -d ::1/128 -j RETURN\nip6tables -t nat -A AKEV -p tcp -m multiport --dports 80,443 -m cgroup --path user.slice/user-1000.slice/user@1000.service/akev.slice/akev-capture.slice -j REDIRECT --to-ports 8001\nip6tables -t nat -C OUTPUT -p tcp -j AKEV 2>/dev/null || ip6tables -t nat -A OUTPUT -p tcp -j AKEV",
    "warnings": [],
    "notes": [
      "Check where a program runs with: cat /proc/<pid>/cgroup (it must end in /akev-capture.slice/run-…scope).",
      "Only TCP is redirected: QUIC / HTTP/3 (UDP 443) isn't, and browsers fall back to HTTPS over TCP when QUIC fails — block UDP 443 if a program keeps using QUIC.",
      "HTTPS is intercepted only if the program trusts Akev's CA (browsers use their own trust store: use the browser interceptors or trust the CA system-wide).",
      "While the rules are loaded and Akev isn't running, the redirected connections fail: run the teardown commands when you're done."
    ]
  },
  "user-nft": {
    "mode": "user",
    "backend": "nft",
    "port": 8001,
    "summary": "Every program run by nobody (uid 65534)",
    "setup": [
      {
        "title": "Redirect connections to Akev",
        "command": "sudo nft -f - <<'EOF'\ntable inet akev\ndelete table inet akev\ntable inet akev {\n\tchain output {\n\t\ttype nat hook output priority -100; policy accept;\n\t\tip daddr 127.0.0.0/8 return\n\t\tip6 daddr ::1 return\n\t\tmeta skuid 65534 tcp dport { 80, 443 } redirect to :8001\n\t}\n}\nEOF",
        "root": true,
        "description": "Loads the rules (replacing earlier Akev rules) that send TCP connections to ports 80, 443 to port 8001."
      }
    ],
    "test": {
      "title": "Check it works",
      "command": "sudo -u '#65534' curl -sS -o /dev/null -w '%{http_code}\\n' --max-time 5 --resolve akev.cert:80:192.0.2.1 http://akev.cert/",
      "root": true,
      "description": "Runs curl as the captured user: prints 200 when redirection works."
    },
    "teardown": [
      {
        "title": "Remove the redirect rules",
        "command": "sudo nft delete table inet akev",
        "root": true
      }
    ],
    "rules": "table inet akev {\n\tchain output {\n\t\ttype nat hook output priority -100; policy accept;\n\t\tip daddr 127.0.0.0/8 return\n\t\tip6 daddr ::1 return\n\t\tmeta skuid 65534 tcp dport { 80, 443 } redirect to :8001\n\t}\n}\n",
    "warnings": [],
    "notes": [
      "Only TCP is redirected: QUIC / HTTP/3 (UDP 443) isn't, and browsers fall back to HTTPS over TCP when QUIC fails — block UDP 443 if a program keeps using QUIC.",
      "HTTPS is intercepted only if the program trusts Akev's CA (browsers use their own trust store: use the browser interceptors or trust the CA system-wide).",
      "While the rules are loaded and Akev isn't running, the redirected connections fail: run the teardown commands when you're done.",
      "The commands use a here-document: paste them into bash or zsh (in fish, run bash first)."
    ]
  },
  "user-iptables": {
    "mode": "user",
    "backend": "iptables",
    "port": 8001,
    "summary": "Every program run by nobody (uid 65534)",
    "setup": [
      {
        "title": "Redirect connections to Akev",
        "command": "sudo iptables -t nat -N AKEV 2>/dev/null || sudo iptables -t nat -F AKEV\nsudo iptables -t nat -A AKEV -d 127.0.0.0/8 -j RETURN\nsudo iptables -t nat -A AKEV -p tcp -m multiport --dports 80,443 -m owner --uid-owner 65534 -j REDIRECT --to-ports 8001\nsudo iptables -t nat -C OUTPUT -p tcp -j AKEV 2>/dev/null || sudo iptables -t nat -A OUTPUT -p tcp -j AKEV\nsudo ip6tables -t nat -N AKEV 2>/dev/null || sudo ip6tables -t nat -F AKEV\nsudo ip6tables -t nat -A AKEV -d ::1/128 -j RETURN\nsudo ip6tables -t nat -A AKEV -p tcp -m multiport --dports 80,443 -m owner --uid-owner 65534 -j REDIRECT --to-ports 8001\nsudo ip6tables -t nat -C OUTPUT -p tcp -j AKEV 2>/dev/null || sudo ip6tables -t nat -A OUTPUT -p tcp -j AKEV",
        "root": true,
        "description": "Loads the rules (replacing earlier Akev rules) that send TCP connections to ports 80, 443 to port 8001."
      }
    ],
    "test": {
      "title": "Check it works",
      "command": "sudo -u '#65534' curl -sS -o /dev/null -w '%{http_code}\\n' --max-time 5 --resolve akev.cert:80:192.0.2.1 http://akev.cert/",
      "root": true,
      "description": "Runs curl as the captured user: prints 200 when redirection works."
    },
    "teardown": [
      {
        "title": "Remove the redirect rules",
        "command": "sudo iptables -t nat -D OUTPUT -p tcp -j AKEV\nsudo iptables -t nat -F AKEV\nsudo iptables -t nat -X AKEV\nsudo ip6tables -t nat -D OUTPUT -p tcp -j AKEV\nsudo ip6tables -t nat -F AKEV\nsudo ip6tables -t nat -X AKEV",
        "root": true
      }
    ],
    "rules": "iptables -t nat -N AKEV 2>/dev/null || iptables -t nat -F AKEV\niptables -t nat -A AKEV -d 127.0.0.0/8 -j RETURN\niptables -t nat -A AKEV -p tcp -m multiport --dports 80,443 -m owner --uid-owner 65534 -j REDIRECT --to-ports 8001\niptables -t nat -C OUTPUT -p tcp -j AKEV 2>/dev/null || iptables -t nat -A OUTPUT -p tcp -j AKEV\nip6tables -t nat -N AKEV 2>/dev/null || ip6tables -t nat -F AKEV\nip6tables -t nat -A AKEV -d ::1/128 -j RETURN\nip6tables -t nat -A AKEV -p tcp -m multiport --dports 80,443 -m owner --uid-owner 65534 -j REDIRECT --to-ports 8001\nip6tables -t nat -C OUTPUT -p tcp -j AKEV 2>/dev/null || ip6tables -t nat -A OUTPUT -p tcp -j AKEV",
    "warnings": [],
    "notes": [
      "Only TCP is redirected: QUIC / HTTP/3 (UDP 443) isn't, and browsers fall back to HTTPS over TCP when QUIC fails — block UDP 443 if a program keeps using QUIC.",
      "HTTPS is intercepted only if the program trusts Akev's CA (browsers use their own trust store: use the browser interceptors or trust the CA system-wide).",
      "While the rules are loaded and Akev isn't running, the redirected connections fail: run the teardown commands when you're done."
    ]
  },
  "machine-nft": {
    "mode": "machine",
    "backend": "nft",
    "port": 8001,
    "summary": "Every program on this machine except those of dev (uid 1000)",
    "setup": [
      {
        "title": "Redirect connections to Akev",
        "command": "sudo nft -f - <<'EOF'\ntable inet akev\ndelete table inet akev\ntable inet akev {\n\tchain output {\n\t\ttype nat hook output priority -100; policy accept;\n\t\tmeta skuid 1000 return\n\t\tip daddr 127.0.0.0/8 return\n\t\tip6 daddr ::1 return\n\t\ttcp dport { 80, 443 } redirect to :8001\n\t}\n}\nEOF",
        "root": true,
        "description": "Loads the rules (replacing earlier Akev rules) that send TCP connections to ports 80, 443 to port 8001."
      }
    ],
    "test": {
      "title": "Check it works",
      "command": "sudo curl -sS -o /dev/null -w '%{http_code}\\n' --max-time 5 --resolve akev.cert:80:192.0.2.1 http://akev.cert/",
      "root": true,
      "description": "Runs curl as root (your own user is excluded): prints 200 when redirection works."
    },
    "teardown": [
      {
        "title": "Remove the redirect rules",
        "command": "sudo nft delete table inet akev",
        "root": true
      }
    ],
    "rules": "table inet akev {\n\tchain output {\n\t\ttype nat hook output priority -100; policy accept;\n\t\tmeta skuid 1000 return\n\t\tip daddr 127.0.0.0/8 return\n\t\tip6 daddr ::1 return\n\t\ttcp dport { 80, 443 } redirect to :8001\n\t}\n}\n",
    "warnings": [
      "Programs run by dev (uid 1000) — the user Akev runs as — are not captured (their connections are indistinguishable from Akev's). Run Akev as a separate user, exclude it by firewall mark, or use per-app capture for your own programs."
    ],
    "notes": [
      "Only TCP is redirected: QUIC / HTTP/3 (UDP 443) isn't, and browsers fall back to HTTPS over TCP when QUIC fails — block UDP 443 if a program keeps using QUIC.",
      "HTTPS is intercepted only if the program trusts Akev's CA (browsers use their own trust store: use the browser interceptors or trust the CA system-wide).",
      "While the rules are loaded and Akev isn't running, the redirected connections fail: run the teardown commands when you're done.",
      "The commands use a here-document: paste them into bash or zsh (in fish, run bash first)."
    ]
  },
  "machine-iptables": {
    "mode": "machine",
    "backend": "iptables",
    "port": 8001,
    "summary": "Every program on this machine except those of dev (uid 1000)",
    "setup": [
      {
        "title": "Redirect connections to Akev",
        "command": "sudo iptables -t nat -N AKEV 2>/dev/null || sudo iptables -t nat -F AKEV\nsudo iptables -t nat -A AKEV -m owner --uid-owner 1000 -j RETURN\nsudo iptables -t nat -A AKEV -d 127.0.0.0/8 -j RETURN\nsudo iptables -t nat -A AKEV -p tcp -m multiport --dports 80,443 -j REDIRECT --to-ports 8001\nsudo iptables -t nat -C OUTPUT -p tcp -j AKEV 2>/dev/null || sudo iptables -t nat -A OUTPUT -p tcp -j AKEV\nsudo ip6tables -t nat -N AKEV 2>/dev/null || sudo ip6tables -t nat -F AKEV\nsudo ip6tables -t nat -A AKEV -m owner --uid-owner 1000 -j RETURN\nsudo ip6tables -t nat -A AKEV -d ::1/128 -j RETURN\nsudo ip6tables -t nat -A AKEV -p tcp -m multiport --dports 80,443 -j REDIRECT --to-ports 8001\nsudo ip6tables -t nat -C OUTPUT -p tcp -j AKEV 2>/dev/null || sudo ip6tables -t nat -A OUTPUT -p tcp -j AKEV",
        "root": true,
        "description": "Loads the rules (replacing earlier Akev rules) that send TCP connections to ports 80, 443 to port 8001."
      }
    ],
    "test": {
      "title": "Check it works",
      "command": "sudo curl -sS -o /dev/null -w '%{http_code}\\n' --max-time 5 --resolve akev.cert:80:192.0.2.1 http://akev.cert/",
      "root": true,
      "description": "Runs curl as root (your own user is excluded): prints 200 when redirection works."
    },
    "teardown": [
      {
        "title": "Remove the redirect rules",
        "command": "sudo iptables -t nat -D OUTPUT -p tcp -j AKEV\nsudo iptables -t nat -F AKEV\nsudo iptables -t nat -X AKEV\nsudo ip6tables -t nat -D OUTPUT -p tcp -j AKEV\nsudo ip6tables -t nat -F AKEV\nsudo ip6tables -t nat -X AKEV",
        "root": true
      }
    ],
    "rules": "iptables -t nat -N AKEV 2>/dev/null || iptables -t nat -F AKEV\niptables -t nat -A AKEV -m owner --uid-owner 1000 -j RETURN\niptables -t nat -A AKEV -d 127.0.0.0/8 -j RETURN\niptables -t nat -A AKEV -p tcp -m multiport --dports 80,443 -j REDIRECT --to-ports 8001\niptables -t nat -C OUTPUT -p tcp -j AKEV 2>/dev/null || iptables -t nat -A OUTPUT -p tcp -j AKEV\nip6tables -t nat -N AKEV 2>/dev/null || ip6tables -t nat -F AKEV\nip6tables -t nat -A AKEV -m owner --uid-owner 1000 -j RETURN\nip6tables -t nat -A AKEV -d ::1/128 -j RETURN\nip6tables -t nat -A AKEV -p tcp -m multiport --dports 80,443 -j REDIRECT --to-ports 8001\nip6tables -t nat -C OUTPUT -p tcp -j AKEV 2>/dev/null || ip6tables -t nat -A OUTPUT -p tcp -j AKEV",
    "warnings": [
      "Programs run by dev (uid 1000) — the user Akev runs as — are not captured (their connections are indistinguishable from Akev's). Run Akev as a separate user, exclude it by firewall mark, or use per-app capture for your own programs."
    ],
    "notes": [
      "Only TCP is redirected: QUIC / HTTP/3 (UDP 443) isn't, and browsers fall back to HTTPS over TCP when QUIC fails — block UDP 443 if a program keeps using QUIC.",
      "HTTPS is intercepted only if the program trusts Akev's CA (browsers use their own trust store: use the browser interceptors or trust the CA system-wide).",
      "While the rules are loaded and Akev isn't running, the redirected connections fail: run the teardown commands when you're done."
    ]
  }
};
