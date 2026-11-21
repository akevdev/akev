/**
 * Interceptor catalogue for the mock backend, captured from a real Akev daemon
 * (`GET /api/interceptors` on Linux) with local paths scrubbed and demo devices/containers added.
 * Regenerate when interceptor ids or metadata shapes change.
 */
import type { InterceptorInfo } from '../types';

export const REAL_INTERCEPTORS: InterceptorInfo[] = [
  {
    "id": "chrome",
    "name": "Chrome",
    "description": "Intercept a fresh, independent Chrome window (separate profile, logins persist)",
    "category": "browser",
    "tags": [
      "browsers",
      "web",
      "pwa",
      "browser"
    ],
    "icon": "chrome",
    "supported": true,
    "available": true,
    "active": true,
    "version": "156.0.8078.47",
    "metadata": {
      "path": "/usr/bin/google-chrome-stable",
      "install": "native",
      "profileDir": "~/.local/share/akev/profiles/chrome",
      "pids": []
    },
    "needsConfig": false
  },
  {
    "id": "firefox",
    "name": "Firefox",
    "description": "Intercept a fresh, independent Firefox window (separate profile, logins persist)",
    "category": "browser",
    "tags": [
      "browsers",
      "web",
      "pwa",
      "browser"
    ],
    "icon": "firefox",
    "supported": true,
    "available": true,
    "active": false,
    "version": "159.0",
    "metadata": {
      "path": "/usr/bin/firefox",
      "install": "native",
      "profileDir": "~/.local/share/akev/profiles/firefox",
      "pids": [],
      "caTrusted": false,
      "certutil": null
    },
    "needsConfig": false
  },
  {
    "id": "chromium",
    "name": "Chromium",
    "description": "Intercept a fresh, independent Chromium window",
    "category": "browser",
    "tags": [
      "browsers",
      "web",
      "pwa",
      "browser"
    ],
    "icon": "chromium",
    "supported": true,
    "available": false,
    "active": false,
    "unavailableReason": "Chromium isn't installed (looked in PATH, standard install locations, Snap and Flatpak)",
    "needsConfig": false
  },
  {
    "id": "brave",
    "name": "Brave",
    "description": "Intercept a fresh, independent Brave window",
    "category": "browser",
    "tags": [
      "browsers",
      "web",
      "pwa",
      "browser"
    ],
    "icon": "brave",
    "supported": true,
    "available": true,
    "active": false,
    "needsConfig": false,
    "version": "1.73.97"
  },
  {
    "id": "edge",
    "name": "Microsoft Edge",
    "description": "Intercept a fresh, independent Microsoft Edge window",
    "category": "browser",
    "tags": [
      "browsers",
      "web",
      "pwa",
      "browser"
    ],
    "icon": "edge",
    "supported": true,
    "available": false,
    "active": false,
    "unavailableReason": "Microsoft Edge isn't installed (looked in PATH, standard install locations, Snap and Flatpak)",
    "needsConfig": false
  },
  {
    "id": "opera",
    "name": "Opera",
    "description": "Intercept a fresh, independent Opera window",
    "category": "browser",
    "tags": [
      "browsers",
      "web",
      "pwa",
      "browser"
    ],
    "icon": "opera",
    "supported": true,
    "available": false,
    "active": false,
    "unavailableReason": "Opera isn't installed (looked in PATH, standard install locations, Snap and Flatpak)",
    "needsConfig": false
  },
  {
    "id": "vivaldi",
    "name": "Vivaldi",
    "description": "Intercept a fresh, independent Vivaldi window",
    "category": "browser",
    "tags": [
      "browsers",
      "web",
      "pwa",
      "browser"
    ],
    "icon": "vivaldi",
    "supported": true,
    "available": false,
    "active": false,
    "unavailableReason": "Vivaldi isn't installed (looked in PATH, standard install locations, Snap and Flatpak)",
    "needsConfig": false
  },
  {
    "id": "fresh-terminal",
    "name": "Fresh Terminal",
    "description": "Open a new terminal where every CLI tool, script and runtime you start is proxied and trusts the Akev CA",
    "category": "terminal",
    "tags": [
      "terminal",
      "command line",
      "cli",
      "bash",
      "zsh",
      "fish",
      "powershell",
      "cmd",
      "shell",
      "curl",
      "node",
      "js",
      "python",
      "ruby",
      "php",
      "go",
      "rust",
      "jvm",
      "java",
      "kotlin",
      "scala",
      "clojure",
      "groovy"
    ],
    "icon": "terminal",
    "supported": true,
    "available": true,
    "active": false,
    "metadata": {
      "terminal": "ptyxis",
      "terminalName": "Ptyxis",
      "terminals": [
        {
          "id": "ptyxis",
          "name": "Ptyxis"
        }
      ],
      "shell": "/bin/bash"
    },
    "needsConfig": false
  },
  {
    "id": "existing-terminal",
    "name": "Existing Terminal",
    "description": "Intercept everything launched from a terminal that's already open, by pasting one command (bash, zsh, fish, PowerShell or cmd)",
    "category": "terminal",
    "tags": [
      "terminal",
      "command line",
      "cli",
      "bash",
      "zsh",
      "fish",
      "powershell",
      "cmd",
      "shell",
      "curl",
      "node",
      "js",
      "python",
      "ruby",
      "php",
      "go",
      "rust",
      "jvm",
      "java",
      "kotlin",
      "scala",
      "clojure",
      "groovy"
    ],
    "icon": "terminal",
    "supported": true,
    "available": true,
    "active": false,
    "metadata": {
      "defaultShell": "bash",
      "shells": {
        "bash": {
          "label": "Bash",
          "command": "eval \"$(printf '%s\\n' 'header = \"Authorization: Bearer mock-token\"' | curl -sS --noproxy '*' -K - 'http://127.0.0.1:45456/api/setup/terminal?shell=bash')\"",
          "reset": "akev-off"
        },
        "zsh": {
          "label": "Zsh",
          "command": "eval \"$(printf '%s\\n' 'header = \"Authorization: Bearer mock-token\"' | curl -sS --noproxy '*' -K - 'http://127.0.0.1:45456/api/setup/terminal?shell=zsh')\"",
          "reset": "akev-off"
        },
        "fish": {
          "label": "Fish",
          "command": "printf '%s\\n' 'header = \"Authorization: Bearer mock-token\"' | curl -sS --noproxy '*' -K - 'http://127.0.0.1:45456/api/setup/terminal?shell=fish' | source",
          "reset": "akev-off"
        },
        "powershell": {
          "label": "PowerShell",
          "command": "iex (& { $c = New-Object Net.WebClient -Property @{Proxy=$null}; $c.Headers['Authorization'] = 'Bearer mock-token'; $c.DownloadString('http://127.0.0.1:45456/api/setup/terminal?shell=powershell') })",
          "reset": "akev-off"
        }
      },
      "proxy": "http://127.0.0.1:8000",
      "note": "Run the command in the terminal you want to intercept. Programs started from it afterwards use the proxy; run `akev-off` (or the reset command) to stop."
    },
    "needsConfig": true
  },
  {
    "id": "android-adb",
    "name": "Android Device via ADB",
    "description": "Proxy an Android device or emulator connected over ADB (USB tunnel, no Wi-Fi setup) and push the CA for installation",
    "category": "mobile",
    "tags": [
      "mobile",
      "phone",
      "app",
      "android",
      "samsung",
      "galaxy",
      "pixel",
      "google",
      "nokia",
      "lg",
      "motorola",
      "emulator",
      "adb",
      "usb",
      "jvm",
      "java",
      "kotlin"
    ],
    "icon": "android",
    "supported": true,
    "available": true,
    "active": false,
    "metadata": {
      "adbPath": "/usr/bin/adb",
      "devices": [
        {
          "id": "emulator-5554",
          "name": "Pixel 8 API 35",
          "state": "device",
          "model": "sdk_gphone64_x86_64",
          "manufacturer": "Google",
          "androidVersion": "15",
          "sdk": 35,
          "emulator": true,
          "active": false
        },
        {
          "id": "R58M41ABCDE",
          "name": "R58M41ABCDE",
          "state": "unauthorized",
          "model": null,
          "manufacturer": null,
          "androidVersion": null,
          "sdk": null,
          "emulator": false,
          "active": false
        }
      ]
    },
    "needsConfig": true
  },
  {
    "id": "ios",
    "name": "iOS via Manual Setup",
    "description": "Manually intercept all HTTP and HTTPS traffic from any iPhone or iPad on your network",
    "category": "mobile",
    "tags": [
      "mobile",
      "phone",
      "app",
      "apple",
      "ios",
      "iphone",
      "ipad",
      "qr"
    ],
    "icon": "ios",
    "supported": true,
    "available": true,
    "active": false,
    "metadata": {
      "host": "192.168.64.114",
      "port": 18200,
      "setupUrl": "http://192.168.64.114:8000/",
      "qrPayload": "http://192.168.64.114:8000/",
      "mobileconfigUrl": "http://192.168.64.114:8000/ca.mobileconfig",
      "addresses": [
        {
          "host": "192.168.64.114",
          "port": 18200,
          "proxyUrl": "http://192.168.64.114:8000",
          "setupUrl": "http://192.168.64.114:8000/",
          "pacUrl": "http://192.168.64.114:8000/proxy.pac",
          "caPemUrl": "http://192.168.64.114:8000/ca.pem",
          "caDerUrl": "http://192.168.64.114:8000/ca.crt",
          "mobileconfigUrl": "http://192.168.64.114:8000/ca.mobileconfig"
        },
        {
          "host": "100.84.10.66",
          "port": 18200,
          "proxyUrl": "http://100.84.10.66:8000",
          "setupUrl": "http://100.84.10.66:8000/",
          "pacUrl": "http://100.84.10.66:8000/proxy.pac",
          "caPemUrl": "http://100.84.10.66:8000/ca.pem",
          "caDerUrl": "http://100.84.10.66:8000/ca.crt",
          "mobileconfigUrl": "http://100.84.10.66:8000/ca.mobileconfig"
        }
      ],
      "steps": [
        {
          "title": "Connect to the same network",
          "detail": "Join the Wi-Fi network this computer is on."
        },
        {
          "title": "Set the Wi-Fi proxy",
          "detail": "Settings \u2192 Wi-Fi \u2192 tap (i) next to the network \u2192 Configure Proxy \u2192 Manual. Server: 192.168.64.114, Port: 18200, Authentication off. Save."
        },
        {
          "title": "Install the Akev profile",
          "detail": "Scan the QR code (or open http://192.168.64.114:8000/ in Safari) and tap the iPhone/iPad button, or open http://192.168.64.114:8000/ca.mobileconfig directly. Allow the download, then Settings \u2192 Profile Downloaded \u2192 Install."
        },
        {
          "title": "Trust the certificate",
          "detail": "Settings \u2192 General \u2192 About \u2192 Certificate Trust Settings \u2192 turn on full trust for the Akev CA. Without this, HTTPS fails."
        },
        {
          "title": "Undo when done",
          "detail": "Set Configure Proxy back to Off, and remove the profile in Settings \u2192 General \u2192 VPN & Device Management."
        }
      ],
      "notes": [
        "The proxy must accept connections from other devices (bind it to all interfaces in Settings) and your firewall must allow the port.",
        "Apps that pin certificates won't work through any proxy."
      ]
    },
    "needsConfig": true
  },
  {
    "id": "wireguard",
    "name": "WireGuard (phones & any device)",
    "description": "Scan a QR code with the WireGuard app: all of the device's traffic is intercepted, no proxy settings needed. iOS, Android, macOS, Windows and Linux.",
    "category": "mobile",
    "tags": [
      "wireguard",
      "vpn",
      "ios",
      "iphone",
      "ipad",
      "android",
      "phone",
      "tablet",
      "qr",
      "tunnel",
      "mobile"
    ],
    "icon": "wireguard",
    "supported": true,
    "available": true,
    "active": false,
    "metadata": {
      "listenAddr": null,
      "connected": false,
      "lastHandshakeAt": null
    },
    "needsConfig": true
  },
  {
    "id": "docker",
    "name": "Docker & Podman Containers",
    "description": "Proxy new containers with a ready-made run snippet (env file + CA mounts), and open intercepted shells in running ones. Running containers' own processes can't be re-routed without recreating them.",
    "category": "container",
    "tags": [
      "docker",
      "podman",
      "container",
      "containers",
      "bridge",
      "services",
      "images"
    ],
    "icon": "docker",
    "supported": true,
    "available": true,
    "active": false,
    "metadata": {
      "engines": [
        {
          "kind": "podman",
          "path": "/usr/bin/podman",
          "available": true,
          "error": null
        }
      ],
      "containers": [
        {
          "id": "f3a9c1d2e4b5a6978877665544332211",
          "shortId": "f3a9c1d2e4b5",
          "name": "api-server",
          "names": [
            "api-server"
          ],
          "image": "node:22-alpine",
          "state": "running",
          "status": "Up 2 hours",
          "engine": "docker",
          "intercepted": false
        },
        {
          "id": "0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e",
          "shortId": "0b1c2d3e4f5a",
          "name": "worker",
          "names": [
            "worker"
          ],
          "image": "python:3.13-slim",
          "state": "running",
          "status": "Up 14 minutes",
          "engine": "docker",
          "intercepted": false
        }
      ],
      "snippets": {
        "envFiles": [
          {
            "path": "~/.local/share/akev/docker/akev-docker.env",
            "content": "# Generated by Akev: proxy settings for containers\nHTTP_PROXY=http://host.docker.internal:8000\nHTTPS_PROXY=http://host.docker.internal:8000\nhttp_proxy=http://host.docker.internal:8000\nhttps_proxy=http://host.docker.internal:8000\nWS_PROXY=http://host.docker.internal:8000\nWSS_PROXY=http://host.docker.internal:8000\nNO_PROXY=\nno_proxy=\nGLOBAL_AGENT_HTTP_PROXY=http://host.docker.internal:8000\nCGI_HTTP_PROXY=http://host.docker.internal:8000\nnpm_config_proxy=http://host.docker.internal:8000\nnpm_config_https_proxy=http://host.docker.internal:8000\nYARN_HTTP_PROXY=http://host.docker.internal:8000\nYARN_HTTPS_PROXY=http://host.docker.internal:8000\nNODE_USE_ENV_PROXY=1\nELECTRON_GET_USE_PROXY=1\nSSL_CERT_FILE=/akev/ca-bundle.pem\nREQUESTS_CA_BUNDLE=/akev/ca-bundle.pem\nCURL_CA_BUNDLE=/akev/ca-bundle.pem\nGIT_SSL_CAINFO=/akev/ca-bundle.pem\nAWS_CA_BUNDLE=/akev/ca-bundle.pem\nCARGO_HTTP_CAINFO=/akev/ca-bundle.pem\nPIP_CERT=/akev/ca-bundle.pem\nBUNDLE_SSL_CA_CERT=/akev/ca-bundle.pem\nDENO_CERT=/akev/ca-bundle.pem\nHEX_CACERTS_PATH=/akev/ca-bundle.pem\nPERL_LWP_SSL_CA_FILE=/akev/ca-bundle.pem\nCLOUDSDK_CORE_CUSTOM_CA_CERTS_FILE=/akev/ca-bundle.pem\nGRPC_DEFAULT_SSL_ROOTS_FILE_PATH=/akev/ca-bundle.pem\nNIX_SSL_CERT_FILE=/akev/ca-bundle.pem\nHTTPLIB2_CA_CERTS=/akev/ca-bundle.pem\nNODE_EXTRA_CA_CERTS=/akev/ca.pem\nJAVA_TOOL_OPTIONS=-Dhttp.proxyHost=host.docker.internal -Dhttp.proxyPort=18200 -Dhttps.proxyHost=host.docker.internal -Dhttps.proxyPort=18200 -Dhttp.nonProxyHosts= -Djdk.http.auth.tunneling.disabledSchemes=\nAKEV_ACTIVE=1\nAKEV_PROXY=http://host.docker.internal:8000\n"
          },
          {
            "path": "~/.local/share/akev/docker/akev-podman.env",
            "content": "# Generated by Akev: proxy settings for containers\nHTTP_PROXY=http://host.containers.internal:8000\nHTTPS_PROXY=http://host.containers.internal:8000\nhttp_proxy=http://host.containers.internal:8000\nhttps_proxy=http://host.containers.internal:8000\nWS_PROXY=http://host.containers.internal:8000\nWSS_PROXY=http://host.containers.internal:8000\nNO_PROXY=\nno_proxy=\nGLOBAL_AGENT_HTTP_PROXY=http://host.containers.internal:8000\nCGI_HTTP_PROXY=http://host.containers.internal:8000\nnpm_config_proxy=http://host.containers.internal:8000\nnpm_config_https_proxy=http://host.containers.internal:8000\nYARN_HTTP_PROXY=http://host.containers.internal:8000\nYARN_HTTPS_PROXY=http://host.containers.internal:8000\nNODE_USE_ENV_PROXY=1\nELECTRON_GET_USE_PROXY=1\nSSL_CERT_FILE=/akev/ca-bundle.pem\nREQUESTS_CA_BUNDLE=/akev/ca-bundle.pem\nCURL_CA_BUNDLE=/akev/ca-bundle.pem\nGIT_SSL_CAINFO=/akev/ca-bundle.pem\nAWS_CA_BUNDLE=/akev/ca-bundle.pem\nCARGO_HTTP_CAINFO=/akev/ca-bundle.pem\nPIP_CERT=/akev/ca-bundle.pem\nBUNDLE_SSL_CA_CERT=/akev/ca-bundle.pem\nDENO_CERT=/akev/ca-bundle.pem\nHEX_CACERTS_PATH=/akev/ca-bundle.pem\nPERL_LWP_SSL_CA_FILE=/akev/ca-bundle.pem\nCLOUDSDK_CORE_CUSTOM_CA_CERTS_FILE=/akev/ca-bundle.pem\nGRPC_DEFAULT_SSL_ROOTS_FILE_PATH=/akev/ca-bundle.pem\nNIX_SSL_CERT_FILE=/akev/ca-bundle.pem\nHTTPLIB2_CA_CERTS=/akev/ca-bundle.pem\nNODE_EXTRA_CA_CERTS=/akev/ca.pem\nJAVA_TOOL_OPTIONS=-Dhttp.proxyHost=host.containers.internal -Dhttp.proxyPort=18200 -Dhttps.proxyHost=host.containers.internal -Dhttps.proxyPort=18200 -Dhttp.nonProxyHosts= -Djdk.http.auth.tunneling.disabledSchemes=\nAKEV_ACTIVE=1\nAKEV_PROXY=http://host.containers.internal:8000\n"
          }
        ],
        "run": {
          "docker": "docker run --rm -it --add-host=host.docker.internal:host-gateway --env-file ~/.local/share/akev/docker/akev-docker.env -v ~/.local/share/akev/ca.pem:/akev/ca.pem:ro,z -v ~/.local/share/akev/ca-bundle.pem:/akev/ca-bundle.pem:ro,z IMAGE",
          "podman": "podman run --rm -it --env-file ~/.local/share/akev/docker/akev-podman.env -v ~/.local/share/akev/ca.pem:/akev/ca.pem:ro,z -v ~/.local/share/akev/ca-bundle.pem:/akev/ca-bundle.pem:ro,z IMAGE"
        },
        "compose": "services:\n  app:\n    env_file: ~/.local/share/akev/docker/akev-docker.env\n    extra_hosts: [\"host.docker.internal:host-gateway\"]\n    volumes:\n      - ~/.local/share/akev/ca.pem:/akev/ca.pem:ro\n      - ~/.local/share/akev/ca-bundle.pem:/akev/ca-bundle.pem:ro\n",
        "runHostNetwork": "docker run --rm -it --network host --env-file ~/.local/share/akev/docker/akev-hostnet.env -v ~/.local/share/akev/ca.pem:/akev/ca.pem:ro,z -v ~/.local/share/akev/ca-bundle.pem:/akev/ca-bundle.pem:ro,z IMAGE",
        "notes": [
          "Bridge-networked containers reach the host through host.docker.internal / host.containers.internal: the Akev proxy must listen on all interfaces (Settings \u2192 Proxy \u2192 Allow other devices).",
          "--network host works with the default 127.0.0.1-only proxy.",
          "Already-running containers keep their environment: recreate them with these settings, or open an intercepted shell inside one."
        ]
      }
    },
    "needsConfig": true
  },
  {
    "id": "system-proxy",
    "name": "System Proxy",
    "description": "Intercept all HTTP traffic on this machine by pointing the OS-wide proxy setting at Akev (your previous settings are restored when you stop, even after a crash)",
    "category": "system",
    "tags": [
      "everything",
      "system",
      "global",
      "local",
      "machine",
      "me",
      "os"
    ],
    "icon": "desktop",
    "supported": true,
    "available": true,
    "active": false,
    "metadata": {
      "backends": [
        {
          "id": "gnome",
          "name": "GNOME"
        }
      ],
      "proxy": "127.0.0.1:8000",
      "bypass": [
        "localhost",
        "127.0.0.0/8",
        "::1"
      ],
      "backupFile": "~/.local/share/akev/system-proxy-backup.json",
      "active": false,
      "trust": {
        "summary": "Most Linux tools read the distribution's CA bundle; Chrome and Chromium read the NSS database in ~/.pki/nssdb. Add the Akev CA where you need it (requires sudo for the system bundle), and remove it when you're done.",
        "options": [
          {
            "title": "Fedora, RHEL, Arch, openSUSE (p11-kit)",
            "install": [
              "sudo trust anchor --store '~/.local/share/akev/ca.pem'"
            ],
            "remove": [
              "sudo trust anchor --remove '~/.local/share/akev/ca.pem'"
            ]
          },
          {
            "title": "Debian, Ubuntu",
            "install": [
              "sudo cp '~/.local/share/akev/ca.pem' /usr/local/share/ca-certificates/akev-ca.crt",
              "sudo update-ca-certificates"
            ],
            "remove": [
              "sudo rm /usr/local/share/ca-certificates/akev-ca.crt",
              "sudo update-ca-certificates --fresh"
            ]
          },
          {
            "title": "Chrome / Chromium (NSS, current user)",
            "install": [
              "certutil -d sql:$HOME/.pki/nssdb -A -t C,, -n 'Akev CA' -i '~/.local/share/akev/ca.pem'"
            ],
            "remove": [
              "certutil -d sql:$HOME/.pki/nssdb -D -n 'Akev CA'"
            ]
          }
        ],
        "notes": [
          "Firefox keeps its own certificate store: use the Firefox interceptor, or import the CA in Settings \u2192 Privacy & Security \u2192 Certificates.",
          "Java, Node.js and Python may ignore the system bundle: use the terminal interceptor's environment variables for them."
        ],
        "caSha256": "54E9BCE0996E55C281ACD3753CB4AEEF86BD4000606B4155D31954C87925F40A"
      },
      "note": "Apps that follow the system proxy (browsers, most GUI apps) are captured. Many CLI tools and runtimes read environment variables instead: use a Fresh Terminal for those."
    },
    "needsConfig": false
  },
  {
    "id": "electron",
    "name": "Electron Application",
    "description": "Launch an Electron application (Slack, VS Code, Postman, your own app...) with all its traffic intercepted",
    "category": "runtime",
    "tags": [
      "electron",
      "desktop",
      "app",
      "postman",
      "slack",
      "vscode"
    ],
    "icon": "electron",
    "supported": true,
    "available": true,
    "active": false,
    "metadata": {
      "running": [],
      "pathKind": "binary",
      "options": {
        "path": "absolute path to the app (a .app bundle on macOS)",
        "args": "extra arguments (optional)"
      }
    },
    "needsConfig": true
  },
  {
    "id": "manual",
    "name": "Anything",
    "description": "Manually configure any source using the proxy settings and CA certificate",
    "category": "manual",
    "tags": [
      "manual",
      "anything",
      "proxy",
      "pac",
      "certificate",
      "ca"
    ],
    "icon": "manual",
    "supported": true,
    "available": true,
    "active": false,
    "metadata": {
      "proxy": {
        "host": "127.0.0.1",
        "port": 18200,
        "url": "http://127.0.0.1:8000",
        "protocols": [
          "http",
          "https (CONNECT)",
          "socks4",
          "socks5"
        ]
      },
      "local": {
        "host": "127.0.0.1",
        "port": 18200,
        "proxyUrl": "http://127.0.0.1:8000",
        "setupUrl": "http://127.0.0.1:8000/",
        "pacUrl": "http://127.0.0.1:8000/proxy.pac",
        "caPemUrl": "http://127.0.0.1:8000/ca.pem",
        "caDerUrl": "http://127.0.0.1:8000/ca.crt",
        "mobileconfigUrl": "http://127.0.0.1:8000/ca.mobileconfig"
      },
      "lan": [
        {
          "host": "192.168.64.114",
          "port": 18200,
          "proxyUrl": "http://192.168.64.114:8000",
          "setupUrl": "http://192.168.64.114:8000/",
          "pacUrl": "http://192.168.64.114:8000/proxy.pac",
          "caPemUrl": "http://192.168.64.114:8000/ca.pem",
          "caDerUrl": "http://192.168.64.114:8000/ca.crt",
          "mobileconfigUrl": "http://192.168.64.114:8000/ca.mobileconfig"
        },
        {
          "host": "100.84.10.66",
          "port": 18200,
          "proxyUrl": "http://100.84.10.66:8000",
          "setupUrl": "http://100.84.10.66:8000/",
          "pacUrl": "http://100.84.10.66:8000/proxy.pac",
          "caPemUrl": "http://100.84.10.66:8000/ca.pem",
          "caDerUrl": "http://100.84.10.66:8000/ca.crt",
          "mobileconfigUrl": "http://100.84.10.66:8000/ca.mobileconfig"
        }
      ],
      "pacUrl": "http://127.0.0.1:8000/proxy.pac",
      "setupUrl": "http://127.0.0.1:8000/",
      "setupUrlViaProxy": "http://akev.cert/",
      "ca": {
        "path": "~/.local/share/akev/ca.pem",
        "bundlePath": "~/.local/share/akev/ca-bundle.pem",
        "spkiFingerprint": "2bXEJ0ibxsClbTsBR6o9lW8P00vyCEl4nB5locNLgUQ=",
        "pemUrl": "http://127.0.0.1:8000/ca.pem",
        "derUrl": "http://127.0.0.1:8000/ca.crt",
        "mobileconfigUrl": "http://127.0.0.1:8000/ca.mobileconfig"
      },
      "envVars": [
        {
          "name": "HTTP_PROXY",
          "value": "http://127.0.0.1:8000"
        },
        {
          "name": "HTTPS_PROXY",
          "value": "http://127.0.0.1:8000"
        },
        {
          "name": "http_proxy",
          "value": "http://127.0.0.1:8000"
        },
        {
          "name": "https_proxy",
          "value": "http://127.0.0.1:8000"
        },
        {
          "name": "WS_PROXY",
          "value": "http://127.0.0.1:8000"
        },
        {
          "name": "WSS_PROXY",
          "value": "http://127.0.0.1:8000"
        },
        {
          "name": "NO_PROXY",
          "value": ""
        },
        {
          "name": "no_proxy",
          "value": ""
        },
        {
          "name": "GLOBAL_AGENT_HTTP_PROXY",
          "value": "http://127.0.0.1:8000"
        },
        {
          "name": "CGI_HTTP_PROXY",
          "value": "http://127.0.0.1:8000"
        },
        {
          "name": "npm_config_proxy",
          "value": "http://127.0.0.1:8000"
        },
        {
          "name": "npm_config_https_proxy",
          "value": "http://127.0.0.1:8000"
        },
        {
          "name": "YARN_HTTP_PROXY",
          "value": "http://127.0.0.1:8000"
        },
        {
          "name": "YARN_HTTPS_PROXY",
          "value": "http://127.0.0.1:8000"
        },
        {
          "name": "NODE_USE_ENV_PROXY",
          "value": "1"
        },
        {
          "name": "ELECTRON_GET_USE_PROXY",
          "value": "1"
        },
        {
          "name": "SSL_CERT_FILE",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "REQUESTS_CA_BUNDLE",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "CURL_CA_BUNDLE",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "GIT_SSL_CAINFO",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "AWS_CA_BUNDLE",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "CARGO_HTTP_CAINFO",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "PIP_CERT",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "BUNDLE_SSL_CA_CERT",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "DENO_CERT",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "HEX_CACERTS_PATH",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "PERL_LWP_SSL_CA_FILE",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "CLOUDSDK_CORE_CUSTOM_CA_CERTS_FILE",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "GRPC_DEFAULT_SSL_ROOTS_FILE_PATH",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "NIX_SSL_CERT_FILE",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "HTTPLIB2_CA_CERTS",
          "value": "~/.local/share/akev/ca-bundle.pem"
        },
        {
          "name": "NODE_EXTRA_CA_CERTS",
          "value": "~/.local/share/akev/ca.pem"
        },
        {
          "name": "JAVA_TOOL_OPTIONS",
          "value": "-Dhttp.proxyHost=127.0.0.1 -Dhttp.proxyPort=18200 -Dhttps.proxyHost=127.0.0.1 -Dhttps.proxyPort=18200 -Dhttp.nonProxyHosts= -Djdk.http.auth.tunneling.disabledSchemes="
        },
        {
          "name": "AKEV_ACTIVE",
          "value": "1"
        },
        {
          "name": "AKEV_PROXY",
          "value": "http://127.0.0.1:8000"
        }
      ],
      "tools": [
        {
          "id": "curl",
          "name": "curl",
          "snippet": "curl --proxy http://127.0.0.1:8000 --cacert '~/.local/share/akev/ca.pem' https://example.com",
          "notes": [
            "Or export the environment variables below: curl reads http_proxy / https_proxy and CURL_CA_BUNDLE."
          ]
        },
        {
          "id": "node",
          "name": "Node.js",
          "snippet": "export HTTPS_PROXY=http://127.0.0.1:8000\nexport HTTP_PROXY=http://127.0.0.1:8000\nexport NODE_USE_ENV_PROXY=1\nexport NODE_EXTRA_CA_CERTS='~/.local/share/akev/ca.pem'",
          "notes": [
            "Recent Node.js releases honour HTTP(S)_PROXY for fetch and http(s) when NODE_USE_ENV_PROXY=1 is set.",
            "Older Node: `npm i global-agent` and run `node -r global-agent/bootstrap app.js` with GLOBAL_AGENT_HTTP_PROXY set, or pass an undici ProxyAgent to fetch.",
            "NODE_EXTRA_CA_CERTS adds the Akev CA to Node's built-in roots."
          ]
        },
        {
          "id": "python",
          "name": "Python",
          "snippet": "export HTTPS_PROXY=http://127.0.0.1:8000\nexport HTTP_PROXY=http://127.0.0.1:8000\nexport REQUESTS_CA_BUNDLE='~/.local/share/akev/ca-bundle.pem'\nexport SSL_CERT_FILE='~/.local/share/akev/ca-bundle.pem'",
          "notes": [
            "requests and urllib read the proxy variables; requests trusts REQUESTS_CA_BUNDLE, httpx/urllib/aiohttp trust SSL_CERT_FILE.",
            "aiohttp needs ClientSession(trust_env=True) to use the proxy variables.",
            "The bundle contains your system roots plus the Akev CA (it's written when an interceptor first needs it)."
          ]
        },
        {
          "id": "java",
          "name": "Java / JVM",
          "snippet": "java -Dhttp.proxyHost=127.0.0.1 -Dhttp.proxyPort=18200 -Dhttps.proxyHost=127.0.0.1 -Dhttps.proxyPort=18200 -Dhttp.nonProxyHosts= -Djavax.net.ssl.trustStore='~/.local/share/akev/java/akev-truststore.p12' -Djavax.net.ssl.trustStorePassword=changeit -jar app.jar",
          "notes": [
            "Or set JAVA_TOOL_OPTIONS to the same flags so every JVM (Gradle, Maven, Kotlin, Scala...) picks them up.",
            "Akev generates the truststore (JVM default roots + Akev CA) with keytool when a terminal interceptor is used. To build one yourself: keytool -importcert -noprompt -alias akev-ca -file '~/.local/share/akev/ca.pem' -keystore '~/.local/share/akev/java/akev-truststore.p12' -storetype PKCS12 -storepass changeit"
          ]
        },
        {
          "id": "go",
          "name": "Go",
          "snippet": "export HTTPS_PROXY=http://127.0.0.1:8000\nexport HTTP_PROXY=http://127.0.0.1:8000\nexport SSL_CERT_FILE='~/.local/share/akev/ca-bundle.pem'",
          "notes": [
            "net/http's default transport uses http.ProxyFromEnvironment; on Linux, crypto/x509 reads SSL_CERT_FILE.",
            "Go never proxies requests to localhost / 127.0.0.1: use your machine's hostname or LAN IP to capture local services.",
            "On macOS and Windows Go uses the OS trust store: trust the CA system-wide (see System Proxy) or add it to a custom tls.Config."
          ]
        },
        {
          "id": "rust",
          "name": "Rust",
          "snippet": "export HTTPS_PROXY=http://127.0.0.1:8000\nexport HTTP_PROXY=http://127.0.0.1:8000\nexport SSL_CERT_FILE='~/.local/share/akev/ca-bundle.pem'",
          "notes": [
            "reqwest uses the proxy variables by default; with rustls-native-certs or native-tls (OpenSSL) it trusts SSL_CERT_FILE.",
            "Builds using webpki-roots need the CA in code: reqwest::Client::builder().add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read('~/.local/share/akev/ca.pem')?)?)",
            "cargo itself honours HTTPS_PROXY and CARGO_HTTP_CAINFO (both set by the terminal interceptors)."
          ]
        },
        {
          "id": "ios",
          "name": "iPhone / iPad",
          "snippet": "http://192.168.64.114:8000/ca.mobileconfig",
          "notes": [
            "Settings \u2192 Wi-Fi \u2192 (i) \u2192 Configure Proxy \u2192 Manual: server 192.168.64.114, port 18200.",
            "Open http://192.168.64.114:8000/ in Safari, install the configuration profile (Settings \u2192 Profile Downloaded \u2192 Install).",
            "Settings \u2192 General \u2192 About \u2192 Certificate Trust Settings \u2192 enable full trust for the Akev CA."
          ]
        },
        {
          "id": "android",
          "name": "Android",
          "snippet": "http://192.168.64.114:8000/ca.crt",
          "notes": [
            "Settings \u2192 Network & internet \u2192 Wi-Fi \u2192 your network \u2192 Edit \u2192 Proxy: Manual, host 192.168.64.114, port 18200 (or use the Android via ADB interceptor over USB).",
            "Download http://192.168.64.114:8000/ca.crt in the browser, then Settings \u2192 Security \u2192 Encryption & credentials \u2192 Install a certificate \u2192 CA certificate.",
            "Apps targeting Android 7+ only trust user CAs if their network_security_config allows it, e.g. <debug-overrides><trust-anchors><certificates src=\"user\"/></trust-anchors></debug-overrides> for debug builds."
          ]
        }
      ],
      "steps": [
        "Point the client at the proxy (address above, or the PAC URL for automatic configuration).",
        "Make the client trust the Akev CA (download links above) \u2014 needed for HTTPS."
      ],
      "note": "Other devices can only connect if the proxy accepts connections from the network (bind it to all interfaces in Settings)."
    },
    "needsConfig": false
  }
];
