/**
 * Copyable setup snippets: terminal one-liners & reset commands, proxy/CA environment
 * variables per shell, per-tool manual setup guides, Docker and OS trust instructions.
 *
 * Pure string builders (unit-tested). Daemon-provided snippets (interceptor metadata) always
 * win over these generated fallbacks.
 */

import type { Platform, TerminalShell } from '../../api/types';

export const SHELLS: readonly { id: TerminalShell; label: string }[] = [
  { id: 'bash', label: 'bash' },
  { id: 'zsh', label: 'zsh' },
  { id: 'fish', label: 'fish' },
  { id: 'powershell', label: 'PowerShell' },
  { id: 'cmd', label: 'cmd' },
];

/** The shell most users of a platform have open. */
export function defaultShell(platform: Platform | null | undefined): TerminalShell {
  if (platform === 'windows') return 'powershell';
  if (platform === 'macos') return 'zsh';
  return 'bash';
}

export function isPosixShell(shell: TerminalShell): boolean {
  return shell === 'bash' || shell === 'zsh' || shell === 'fish';
}

/** Environment variables the setup script sets (and the reset command clears). */
export const PROXY_ENV_VARS = ['HTTP_PROXY', 'HTTPS_PROXY', 'NO_PROXY', 'http_proxy', 'https_proxy', 'no_proxy'] as const;
export const CA_ENV_VARS = ['SSL_CERT_FILE', 'NODE_EXTRA_CA_CERTS', 'REQUESTS_CA_BUNDLE', 'CURL_CA_BUNDLE', 'GIT_SSL_CAINFO', 'DENO_CERT'] as const;
const ALL_ENV_VARS = [...PROXY_ENV_VARS, ...CA_ENV_VARS];

/** Where we suggest saving the downloaded CA, written for the given shell. */
export function caFilePath(shell: TerminalShell, provided?: string | null): string {
  if (provided) return provided;
  if (shell === 'powershell') return '$env:USERPROFILE\\Downloads\\akev-ca.pem';
  if (shell === 'cmd') return '%USERPROFILE%\\Downloads\\akev-ca.pem';
  return '$HOME/Downloads/akev-ca.pem';
}

const posixQuote = (s: string): string => `'${s.replace(/'/g, `'\\''`)}'`;
/** fish treats only `\` and `'` as special inside single quotes. */
const fishQuote = (s: string): string => `'${s.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`;
const psQuote = (s: string): string => `'${s.replace(/['\u2018\u2019\u201A\u201B]/g, (c) => c + c)}'`;

/** A curl config line (`-K`) sending the token as a Bearer header (`\` and `"` escaped). */
export function curlAuthConfig(token: string): string {
  const value = `Authorization: Bearer ${token}`.replace(/[\r\n]/g, '').replace(/["\\]/g, (c) => `\\${c}`);
  return `header = "${value}"`;
}

/**
 * One-liner that configures an already-open terminal from the daemon's setup script (same
 * form as the daemon's own snippets). The token never goes on a process's command line, which
 * other local users can read: curl reads it as a header from a config on stdin (`-K -`)
 * written by the `printf` builtin; PowerShell downloads in-process; cmd uses a config file in
 * `%TEMP%`. `scriptUrl` must not carry the token. `--noproxy`/`Proxy=$null` keep an already
 * proxied terminal from sending the token through the proxy.
 */
export function terminalOneLiner(shell: TerminalShell, scriptUrl: string, token?: string | null): string {
  const cfg = token ? curlAuthConfig(token) : null;
  switch (shell) {
    case 'fish':
      return cfg
        ? `printf '%s\\n' ${fishQuote(cfg)} | curl -sS --noproxy '*' -K - ${fishQuote(scriptUrl)} | source`
        : `curl -sS --noproxy '*' ${fishQuote(scriptUrl)} | source`;
    case 'powershell': {
      const header = token ? `$c.Headers['Authorization'] = ${psQuote(`Bearer ${token}`)}; ` : '';
      return `iex (& { $c = New-Object Net.WebClient -Property @{Proxy=$null}; ${header}$c.DownloadString(${psQuote(scriptUrl)}) })`;
    }
    case 'cmd': {
      const file = '"%TEMP%\\akev-curl.cfg"';
      const script = '"%TEMP%\\akev-setup.cmd"';
      return cfg
        ? `>${file} echo ${cfg}&& curl -sS --noproxy "*" -K ${file} -o ${script} "${scriptUrl}" && (del ${file} & call ${script}) || del ${file} 2>nul`
        : `curl -sS --noproxy "*" -o ${script} "${scriptUrl}" && call ${script}`;
    }
    default:
      return cfg
        ? `eval "$(printf '%s\\n' ${posixQuote(cfg)} | curl -sS --noproxy '*' -K - ${posixQuote(scriptUrl)})"`
        : `eval "$(curl -sS --noproxy '*' ${posixQuote(scriptUrl)})"`;
  }
}

/** Command that undoes {@link terminalOneLiner} in the same terminal. */
export function terminalResetCommand(shell: TerminalShell): string {
  switch (shell) {
    case 'fish':
      return `set -e ${ALL_ENV_VARS.join(' ')}`;
    case 'powershell':
      return `Remove-Item ${ALL_ENV_VARS.filter((v) => v === v.toUpperCase()).map((v) => `Env:${v}`).join(', ')} -ErrorAction SilentlyContinue`;
    case 'cmd':
      return `for %v in (${ALL_ENV_VARS.filter((v) => v === v.toUpperCase()).join(' ')}) do @set %v=`;
    default:
      return `unset ${ALL_ENV_VARS.join(' ')}`;
  }
}

/** A harmless HTTPS request to confirm a terminal is intercepted (shows up in Traffic). */
export function terminalTestCommand(shell: TerminalShell): string {
  if (shell === 'powershell') return 'Invoke-WebRequest https://example.com -UseBasicParsing | Select-Object StatusCode';
  return 'curl -sS -o /dev/null -w "%{http_code}\\n" https://example.com'.replace('/dev/null', shell === 'cmd' ? 'NUL' : '/dev/null');
}

/** Render `NAME=value` assignments for a shell (values quoted as needed). */
export function setEnvLines(shell: TerminalShell, vars: readonly (readonly [string, string])[]): string {
  switch (shell) {
    case 'fish':
      return vars.map(([k, v]) => `set -gx ${k} "${v}"`).join('\n');
    case 'powershell':
      return vars.map(([k, v]) => `$env:${k} = "${v}"`).join('\n');
    case 'cmd':
      return vars.map(([k, v]) => `set "${k}=${v}"`).join('\n');
    default:
      return vars.map(([k, v]) => `export ${k}="${v}"`).join('\n');
  }
}

/** Variables that *replace* the trusted roots (vs. Node/Deno's, which add to them). */
const ROOT_REPLACING_VARS: ReadonlySet<string> = new Set(['SSL_CERT_FILE', 'REQUESTS_CA_BUNDLE', 'CURL_CA_BUNDLE', 'GIT_SSL_CAINFO']);

/**
 * Proxy + CA environment variables for most CLI tools and runtimes. `bundlePath` (system roots
 * + the Akev CA, provided by the daemon) is used for variables that replace the trusted roots,
 * so requests that bypass the proxy keep working.
 */
export function envSnippet(shell: TerminalShell, proxy: string, caPath: string, noProxy = 'localhost,127.0.0.1,::1', bundlePath?: string | null): string {
  const url = `http://${proxy}`;
  const vars: [string, string][] = [
    ['HTTP_PROXY', url],
    ['HTTPS_PROXY', url],
    ['NO_PROXY', noProxy],
  ];
  // Windows env vars are case-insensitive; POSIX tools read either spelling.
  if (isPosixShell(shell)) vars.push(['http_proxy', url], ['https_proxy', url], ['no_proxy', noProxy]);
  for (const name of CA_ENV_VARS) vars.push([name, bundlePath && ROOT_REPLACING_VARS.has(name) ? bundlePath : caPath]);
  return setEnvLines(shell, vars);
}

/** `docker run` flags for *new* containers (attach handles running ones). */
export function dockerRunSnippet(port: number, caHostPath = '$HOME/Downloads/akev-ca.pem'): string {
  const url = `http://host.docker.internal:${port}`;
  return [
    'docker run \\',
    '  --add-host host.docker.internal:host-gateway \\',
    ...['HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy'].map((v) => `  -e ${v}=${url} \\`),
    `  -v "${caHostPath}:/akev-ca.pem:ro" \\`,
    ...['SSL_CERT_FILE', 'NODE_EXTRA_CA_CERTS', 'REQUESTS_CA_BUNDLE'].map((v) => `  -e ${v}=/akev-ca.pem \\`),
    '  your-image',
  ].join('\n');
}

// =====================================================================================
// Guides (manual setup per tool, trust instructions per OS)
// =====================================================================================

export interface GuideBlock {
  title?: string;
  text?: string;
  code?: string;
  /** Ordered steps (rendered as a numbered list). */
  steps?: string[];
  /** Muted footnote. */
  note?: string;
}

export interface Guide {
  id: string;
  label: string;
  /** AppIcon key. */
  icon: string;
  intro: string;
  blocks: GuideBlock[];
}

export interface GuideContext {
  /** Local proxy `host:port` (e.g. `127.0.0.1:8000`). */
  proxy: string;
  /** LAN proxy `host:port` for devices, when the proxy accepts LAN connections. */
  lanProxy?: string | null;
  /** Shell flavour for env snippets. */
  shell: TerminalShell;
  /** CA path written for {@link GuideContext.shell}. */
  caPath: string;
  /** Daemon-provided absolute CA path, if known. */
  providedCaPath?: string | null;
  /** Device setup page URL (`http://<lan>:<port>/`), when LAN is enabled. */
  setupUrl?: string | null;
  /** Daemon-provided CA bundle (system roots + Akev CA) for root-replacing variables. */
  bundlePath?: string | null;
  /** The exact variables Akev's terminal interceptors set (from the daemon), if known. */
  envVars?: readonly { name: string; value: string }[];
}

function hostAndPort(addr: string): [string, string] {
  const i = addr.lastIndexOf(':');
  return i < 0 ? [addr, '8000'] : [addr.slice(0, i).replace(/^\[|\]$/g, ''), addr.slice(i + 1)];
}

/** Per-tool manual setup instructions (DESIGN.md §8.2 "Manual setup"). */
export function manualGuides(ctx: GuideContext): Guide[] {
  const { proxy, shell, caPath } = ctx;
  const url = `http://${proxy}`;
  const [host, port] = hostAndPort(proxy);
  const posix = isPosixShell(shell);
  const device = ctx.lanProxy ?? null;
  const [devHost, devPort] = device ? hostAndPort(device) : ["<your computer's LAN IP>", port];
  const javaCa = posix ? caPath : caPath.replace(/^\$env:USERPROFILE/, '%USERPROFILE%');
  // Root-replacing variables (SSL_CERT_FILE, REQUESTS_CA_BUNDLE…) want the full bundle.
  const bundle = ctx.bundlePath ?? caPath;
  const known = !!ctx.providedCaPath;
  const envVars = ctx.envVars ?? [];

  return [
    {
      id: 'env',
      label: 'Env vars',
      icon: 'terminal',
      intro: 'Most command-line tools and language runtimes read these variables. Set them in a terminal, then start your program from it.',
      blocks:
        envVars.length > 0
          ? [
              { code: setEnvLines(shell, envVars.map((v) => [v.name, v.value] as const)) },
              { note: "These are exactly the variables Akev's terminal interceptors set — the certificate paths already exist on this machine." },
            ]
          : [
              { code: envSnippet(shell, proxy, caPath, undefined, ctx.bundlePath) },
              { note: known ? 'The certificate paths already exist on this machine.' : 'Save the CA certificate (PEM) to the path above first, or change the path.' },
            ],
    },
    {
      id: 'node',
      label: 'Node.js',
      icon: 'node',
      intro: 'Node 24+ can use the standard proxy variables for fetch and http when NODE_USE_ENV_PROXY is set.',
      blocks: [
        {
          title: 'Node.js 24 and newer',
          code: `${setEnvLines(shell, [
            ['NODE_USE_ENV_PROXY', '1'],
            ['HTTP_PROXY', url],
            ['HTTPS_PROXY', url],
            ['NODE_EXTRA_CA_CERTS', caPath],
          ])}\nnode app.js`,
        },
        {
          title: 'Older versions (global-agent)',
          code: `npm install global-agent\n${setEnvLines(shell, [
            ['GLOBAL_AGENT_HTTP_PROXY', url],
            ['NODE_EXTRA_CA_CERTS', caPath],
          ])}\nnode -r global-agent/bootstrap app.js`,
        },
      ],
    },
    {
      id: 'python',
      label: 'Python',
      icon: 'python',
      intro: 'requests, httpx, urllib and pip use the proxy variables automatically.',
      blocks: [
        {
          code: `${setEnvLines(shell, [
            ['HTTP_PROXY', url],
            ['HTTPS_PROXY', url],
            ['REQUESTS_CA_BUNDLE', bundle],
            ['SSL_CERT_FILE', bundle],
          ])}\npython app.py`,
        },
        { note: 'aiohttp only reads the variables with ClientSession(trust_env=True).' },
      ],
    },
    {
      id: 'java',
      label: 'Java',
      icon: 'java',
      intro: 'Import the CA into a truststore, then point the JVM at the proxy and the truststore.',
      blocks: [
        {
          title: 'Create a truststore (once)',
          code: `keytool -importcert -noprompt -alias akev -file "${javaCa}" -keystore akev-truststore.p12 -storetype PKCS12 -storepass changeit`,
        },
        {
          title: 'Run your app',
          code: [
            `java -Dhttp.proxyHost=${host} -Dhttp.proxyPort=${port} \\`,
            `     -Dhttps.proxyHost=${host} -Dhttps.proxyPort=${port} \\`,
            '     -Djavax.net.ssl.trustStore=akev-truststore.p12 -Djavax.net.ssl.trustStorePassword=changeit \\',
            '     -jar app.jar',
          ].join('\n'),
        },
        { note: 'For Gradle, Maven or IDE runs, put the same -D flags in JAVA_TOOL_OPTIONS.' },
      ],
    },
    {
      id: 'go',
      label: 'Go',
      icon: 'go',
      intro: "net/http's default transport uses HTTP_PROXY / HTTPS_PROXY. Go skips the proxy for localhost requests.",
      blocks: [
        { code: `${setEnvLines(shell, [
          ['HTTP_PROXY', url],
          ['HTTPS_PROXY', url],
          ['SSL_CERT_FILE', bundle],
        ])}\ngo run .` },
        { note: 'SSL_CERT_FILE only applies on Linux. On macOS and Windows Go uses the system trust store, so trust the CA system-wide (Settings → Certificates).' },
      ],
    },
    {
      id: 'rust',
      label: 'Rust',
      icon: 'rust',
      intro: 'reqwest reads the proxy variables by default. With rustls-native-certs or OpenSSL, SSL_CERT_FILE adds the CA.',
      blocks: [
        { code: `${setEnvLines(shell, [
          ['HTTPS_PROXY', url],
          ['HTTP_PROXY', url],
          ['SSL_CERT_FILE', bundle],
        ])}\ncargo run` },
        {
          title: 'Or configure the client in code',
          code: [
            'let ca = reqwest::Certificate::from_pem(&std::fs::read("akev-ca.pem")?)?;',
            'let client = reqwest::Client::builder()',
            `    .proxy(reqwest::Proxy::all("${url}")?)`,
            '    .add_root_certificate(ca)',
            '    .build()?;',
          ].join('\n'),
        },
      ],
    },
    {
      id: 'curl',
      label: 'curl',
      icon: 'curl',
      intro: 'Pass the proxy and CA on the command line, or set the environment variables once.',
      blocks: [{ code: `curl --proxy ${url} --cacert "${caPath}" https://example.com` }, { note: 'curl also reads https_proxy and CURL_CA_BUNDLE.' }],
    },
    {
      id: 'ios',
      label: 'iOS',
      icon: 'apple',
      intro: 'Your iPhone or iPad must be on the same network, and “Allow other devices” must be on.',
      blocks: [
        {
          steps: [
            `Settings → Wi-Fi → ⓘ next to your network → Configure Proxy → Manual: server ${devHost}, port ${devPort}.`,
            `In Safari, open ${ctx.setupUrl ?? 'http://akev.cert/'} and tap “Download profile”.`,
            'Settings → General → VPN & Device Management → Akev CA → Install.',
            'Settings → General → About → Certificate Trust Settings → turn on full trust for Akev CA.',
          ],
        },
        { note: "Turn the Wi-Fi proxy off again when you're done, or the device loses connectivity once Akev stops." },
      ],
    },
    {
      id: 'android',
      label: 'Android',
      icon: 'android',
      intro: 'Prefer the Android (ADB) card for automatic setup. To set up a device by hand:',
      blocks: [
        {
          steps: [
            `Long-press your Wi-Fi network → Modify → Advanced → Proxy: Manual, host ${devHost}, port ${devPort}.`,
            `Open ${ctx.setupUrl ?? 'http://akev.cert/'} in the browser and download the certificate.`,
            'Settings → Security → Encryption & credentials → Install a certificate → CA certificate.',
          ],
        },
        {
          title: 'Or set the proxy over ADB',
          code: `adb shell settings put global http_proxy ${device ?? `${devHost}:${devPort}`}\n# undo:\nadb shell settings put global http_proxy :0`,
        },
        {
          title: 'Apps targeting Android 7+ ignore user CAs by default — allow them in debug builds',
          code: [
            '<!-- res/xml/network_security_config.xml -->',
            '<network-security-config>',
            '  <debug-overrides>',
            '    <trust-anchors>',
            '      <certificates src="user" />',
            '    </trust-anchors>',
            '  </debug-overrides>',
            '</network-security-config>',
          ].join('\n'),
        },
      ],
    },
  ];
}

export type TrustTargetId = 'macos' | 'windows' | 'linux' | 'firefox' | 'android' | 'ios';

export const TRUST_TARGETS: readonly { id: TrustTargetId; label: string; icon: string }[] = [
  { id: 'macos', label: 'macOS', icon: 'apple' },
  { id: 'windows', label: 'Windows', icon: 'app' },
  { id: 'linux', label: 'Linux', icon: 'terminal' },
  { id: 'firefox', label: 'Firefox', icon: 'firefox' },
  { id: 'android', label: 'Android', icon: 'android' },
  { id: 'ios', label: 'iOS', icon: 'apple' },
];

/** `CN=Akev CA, O=Akev` → `Akev CA`. */
export function commonName(subject: string | null | undefined): string {
  if (!subject) return 'Akev CA';
  const m = /(?:^|[,/]\s*)CN=([^,/]+)/i.exec(subject);
  return (m?.[1] ?? subject).trim();
}

/** OS / browser specific instructions for trusting the CA system-wide. */
export function trustGuide(target: TrustTargetId, opts: { caName: string; providedCaPath?: string | null; setupUrl?: string | null }): Guide {
  const posixPath = opts.providedCaPath ?? '$HOME/Downloads/akev-ca.pem';
  const winPath = opts.providedCaPath ?? '$env:USERPROFILE\\Downloads\\akev-ca.crt';
  const name = opts.caName;
  switch (target) {
    case 'macos':
      return {
        id: target,
        label: 'macOS',
        icon: 'apple',
        intro: 'Adds the CA to the System keychain so Safari, Chrome and most apps trust it.',
        blocks: [
          { title: 'Trust', code: `sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain "${posixPath}"` },
          { title: 'Remove later', code: `sudo security delete-certificate -c "${name}" /Library/Keychains/System.keychain` },
        ],
      };
    case 'windows':
      return {
        id: target,
        label: 'Windows',
        icon: 'app',
        intro: 'Run PowerShell as Administrator. Edge, Chrome and most apps use the Windows certificate store.',
        blocks: [
          { title: 'Trust (all users)', code: `Import-Certificate -FilePath "${winPath}" -CertStoreLocation Cert:\\LocalMachine\\Root` },
          { title: 'Or for the current user only (no admin)', code: `Import-Certificate -FilePath "${winPath}" -CertStoreLocation Cert:\\CurrentUser\\Root` },
          { title: 'Remove later', code: `Get-ChildItem Cert:\\LocalMachine\\Root | Where-Object Subject -like "*CN=${name}*" | Remove-Item` },
        ],
      };
    case 'linux':
      return {
        id: target,
        label: 'Linux',
        icon: 'terminal',
        intro: 'System trust stores differ per distribution. Chrome and Chromium additionally use the NSS database.',
        blocks: [
          { title: 'Debian / Ubuntu', code: `sudo cp "${posixPath}" /usr/local/share/ca-certificates/akev-ca.crt && sudo update-ca-certificates` },
          { title: 'Fedora / RHEL', code: `sudo cp "${posixPath}" /etc/pki/ca-trust/source/anchors/akev-ca.pem && sudo update-ca-trust` },
          { title: 'Arch', code: `sudo trust anchor --store "${posixPath}"` },
          { title: 'Chrome / Chromium (NSS, needs libnss3-tools)', code: `certutil -d sql:$HOME/.pki/nssdb -A -t "C,," -n "${name}" -i "${posixPath}"` },
        ],
      };
    case 'firefox':
      return {
        id: target,
        label: 'Firefox',
        icon: 'firefox',
        intro: 'Firefox keeps its own certificate store. Firefox windows launched from Akev already trust the CA.',
        blocks: [
          {
            steps: [
              'Settings → Privacy & Security → Certificates → View Certificates…',
              'Authorities → Import… → choose the downloaded PEM file.',
              'Tick “Trust this CA to identify websites” → OK.',
            ],
          },
          { note: 'On Windows and macOS you can instead set security.enterprise_roots.enabled to true in about:config to use the system store.' },
        ],
      };
    case 'android':
      return {
        id: target,
        label: 'Android',
        icon: 'android',
        intro: 'Use the Android (ADB) interceptor for automatic setup, including system-level trust on emulators and rooted devices.',
        blocks: [
          {
            steps: [
              `Open ${opts.setupUrl ?? 'http://akev.cert/'} on the device and download the certificate.`,
              'Settings → Security → Encryption & credentials → Install a certificate → CA certificate.',
            ],
          },
          { note: 'Apps targeting Android 7+ only trust user CAs if their network security config allows it.' },
        ],
      };
    case 'ios':
      return {
        id: target,
        label: 'iOS',
        icon: 'apple',
        intro: 'Install the configuration profile, then enable full trust.',
        blocks: [
          {
            steps: [
              `Open ${opts.setupUrl ?? 'http://akev.cert/'} in Safari and download the profile.`,
              'Settings → General → VPN & Device Management → Akev CA → Install.',
              'Settings → General → About → Certificate Trust Settings → enable full trust.',
            ],
          },
        ],
      };
  }
}
