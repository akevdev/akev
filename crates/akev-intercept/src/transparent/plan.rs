//! Generates the exact commands that make Linux redirect connections to Akev's transparent
//! listener, and the commands that undo them. Akev never runs them (they need root); the
//! user copies them into a terminal.
//!
//! Three scopes:
//! * **app** — only programs launched inside a dedicated cgroup v2 slice
//!   (`systemd-run --user --scope --slice=akev-capture.slice -- <cmd>`, or a plain cgroup
//!   without systemd), matched with nftables `socket cgroupv2`;
//! * **user** — every connection of one (other) user account (`meta skuid`);
//! * **machine** — every connection except Akev's own (excluded by user id or by the
//!   firewall mark Akev sets on its upstream sockets).
//!
//! The rules live in their own table (`inet akev` / chain `AKEV`), redirect only TCP to
//! the chosen ports, and never touch loopback destinations.

use super::facts::SystemFacts;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// nftables table (family `inet`) holding Akev's redirect chain.
pub const NFT_TABLE: &str = "akev";
/// iptables chain (table `nat`) holding Akev's redirect rules.
pub const IPT_CHAIN: &str = "AKEV";
/// systemd user slice that per-app capture launches programs into. (systemd nests
/// dash-separated slice names: it lives at `<user manager>/akev.slice/akev-capture.slice`.)
pub const CAPTURE_SLICE: &str = "akev-capture.slice";
/// cgroup created directly under `/sys/fs/cgroup` when systemd isn't used.
pub const CAPTURE_CGROUP: &str = "akev-capture";
/// Address that only answers when redirection works (TEST-NET-1, never routed).
pub const TEST_IP: &str = "192.0.2.1";

/// Whose connections are redirected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureMode {
    /// Programs launched in the capture cgroup.
    #[default]
    App,
    /// One user account.
    User,
    /// Everything except Akev itself.
    Machine,
}

/// Which destination ports are redirected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PortScope {
    /// 80 and 443.
    #[default]
    Web,
    /// Every TCP port.
    All,
    /// `custom_ports`.
    Custom,
}

/// Firewall front-end the commands are written for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    /// nftables (`nft`).
    #[default]
    Nft,
    /// iptables + ip6tables (legacy or iptables-nft).
    Iptables,
}

/// How machine-wide capture keeps Akev's own connections out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Exclusion {
    /// By Akev's user id (`meta skuid`): that user's other programs aren't captured either.
    #[default]
    Uid,
    /// By the firewall mark Akev puts on its upstream sockets (needs `CAP_NET_ADMIN`).
    Mark,
}

/// How per-app capture puts programs into a cgroup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Launcher {
    /// A systemd user slice (`systemd-run --user --scope --slice=...`).
    #[default]
    Systemd,
    /// A cgroup made by hand under `/sys/fs/cgroup` (no systemd).
    Cgroupfs,
}

/// What to generate (`POST /api/transparent/plan`, or the interceptor's activation options).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlanRequest {
    pub mode: CaptureMode,
    pub ports: PortScope,
    /// Ports for [`PortScope::Custom`].
    pub custom_ports: Vec<u16>,
    pub backend: Backend,
    /// Per-app: the command line to launch in the capture cgroup (default: a new shell).
    pub command: Option<String>,
    /// Per-user: the account to capture (user name or numeric id).
    pub user: Option<String>,
    /// Machine-wide: how Akev's own traffic is excluded.
    pub exclude: Exclusion,
    pub launcher: Launcher,
    /// Destinations (IPs / CIDRs) never redirected, besides loopback.
    pub exclude_destinations: Vec<String>,
    /// Per-app: make the launched program trust Akev's CA through environment variables.
    pub trust_ca: bool,
    /// Prefix root commands with `sudo`.
    pub sudo: bool,
}

impl Default for PlanRequest {
    fn default() -> Self {
        PlanRequest {
            mode: CaptureMode::App,
            ports: PortScope::Web,
            custom_ports: Vec::new(),
            backend: Backend::Nft,
            command: None,
            user: None,
            exclude: Exclusion::Uid,
            launcher: Launcher::Systemd,
            exclude_destinations: Vec::new(),
            trust_ca: true,
            sudo: true,
        }
    }
}

/// One command to run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub title: String,
    /// Shell command(s), ready to paste (bash / zsh).
    pub command: String,
    /// Needs root (already prefixed with `sudo` unless disabled).
    pub root: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The generated commands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub mode: CaptureMode,
    pub backend: Backend,
    /// Transparent listener port the rules redirect to.
    pub port: u16,
    /// One-line description of what gets captured.
    pub summary: String,
    /// Run in order to start capturing.
    pub setup: Vec<Step>,
    /// Per-app: how to launch programs so they're captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<Step>,
    /// Prints `200` when redirection reaches Akev.
    pub test: Step,
    /// Run to stop capturing.
    pub teardown: Vec<Step>,
    /// The firewall rules alone (nft ruleset / iptables commands), for review.
    pub rules: String,
    pub warnings: Vec<String>,
    pub notes: Vec<String>,
}

/// Everything besides the request that the commands depend on.
#[derive(Clone, Debug, Default)]
pub struct PlanContext {
    pub facts: SystemFacts,
    /// Transparent listener port (actual, or the configured one while it's off).
    pub port: u16,
    /// The listener is running.
    pub listening: bool,
    /// Firewall mark configured for upstream sockets (0 = none).
    pub mark: u32,
    /// Why upstream sockets can't be marked, if they can't.
    pub mark_error: Option<String>,
    /// `NAME=value` pairs that make programs trust Akev's CA (per-app launches).
    pub ca_env: Vec<(String, String)>,
    /// The captured user resolved from [`PlanRequest::user`]: `(uid, name)`.
    pub target_user: Option<(u32, Option<String>)>,
}

/// Why no plan can be generated (bad options).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PlanError(pub String);

fn err<T>(msg: impl Into<String>) -> Result<T, PlanError> {
    Err(PlanError(msg.into()))
}

/// Quote `s` for a POSIX shell when needed.
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./:@%+=,-".contains(&b))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// A destination exclusion: an IPv4/IPv6 address or CIDR.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Cidr {
    v6: bool,
    text: String,
}

fn parse_cidr(s: &str) -> Option<Cidr> {
    let s = s.trim();
    let (addr, prefix) = match s.split_once('/') {
        Some((a, p)) => (a, Some(p.parse::<u8>().ok()?)),
        None => (s, None),
    };
    let ip: std::net::IpAddr = addr.parse().ok()?;
    let max = if ip.is_ipv6() { 128 } else { 32 };
    if prefix.is_some_and(|p| p > max) {
        return None;
    }
    Some(Cidr {
        v6: ip.is_ipv6(),
        text: match prefix {
            Some(p) => format!("{ip}/{p}"),
            None => ip.to_string(),
        },
    })
}

/// The cgroup v2 path (no leading slash) programs are captured in, and how they get there.
struct CaptureGroup {
    path: String,
}

impl CaptureGroup {
    fn level(&self) -> usize {
        self.path.split('/').filter(|p| !p.is_empty()).count()
    }
}

fn capture_group(req: &PlanRequest, facts: &SystemFacts) -> Result<CaptureGroup, PlanError> {
    match req.launcher {
        Launcher::Cgroupfs => Ok(CaptureGroup {
            path: CAPTURE_CGROUP.to_string(),
        }),
        Launcher::Systemd => {
            let slice_parent = match facts.uid {
                // Root has no user manager to speak of: use a system slice.
                Some(0) => String::new(),
                Some(_) => match &facts.user_manager_cgroup {
                    Some(m) => format!("{m}/"),
                    None => return err("couldn't find your systemd user manager's cgroup"),
                },
                None => return err("per-app capture needs a Unix user id"),
            };
            Ok(CaptureGroup {
                path: format!("{slice_parent}akev.slice/{CAPTURE_SLICE}"),
            })
        }
    }
}

fn ports_of(req: &PlanRequest) -> Result<Option<Vec<u16>>, PlanError> {
    match req.ports {
        PortScope::Web => Ok(Some(vec![80, 443])),
        PortScope::All => Ok(None),
        PortScope::Custom => {
            let mut ports: Vec<u16> = req
                .custom_ports
                .iter()
                .copied()
                .filter(|p| *p != 0)
                .collect();
            ports.sort_unstable();
            ports.dedup();
            if ports.is_empty() {
                return err("choose at least one destination port");
            }
            Ok(Some(ports))
        }
    }
}

/// Who gets redirected (the match of the redirect rule).
enum Selector {
    Cgroup(CaptureGroup),
    Uid(u32),
    All,
}

struct Rules {
    selector: Selector,
    /// Akev's own uid excluded first (machine mode).
    exclude_uid: Option<u32>,
    mark: u32,
    ports: Option<Vec<u16>>,
    excludes: Vec<Cidr>,
    port: u16,
}

impl Rules {
    fn nft_ruleset(&self) -> String {
        let mut r = String::new();
        let _ = writeln!(r, "table inet {NFT_TABLE} {{");
        let _ = writeln!(r, "\tchain output {{");
        let _ = writeln!(r, "\t\ttype nat hook output priority -100; policy accept;");
        if self.mark != 0 {
            let _ = writeln!(r, "\t\tmeta mark {:#x} return", self.mark);
        }
        if let Some(uid) = self.exclude_uid {
            let _ = writeln!(r, "\t\tmeta skuid {uid} return");
        }
        let _ = writeln!(r, "\t\tip daddr 127.0.0.0/8 return");
        let _ = writeln!(r, "\t\tip6 daddr ::1 return");
        for c in &self.excludes {
            let fam = if c.v6 { "ip6" } else { "ip" };
            let _ = writeln!(r, "\t\t{fam} daddr {} return", c.text);
        }
        let selector = match &self.selector {
            Selector::Cgroup(g) => format!("socket cgroupv2 level {} \"{}\" ", g.level(), g.path),
            Selector::Uid(uid) => format!("meta skuid {uid} "),
            Selector::All => String::new(),
        };
        let ports = match &self.ports {
            Some(p) if p.len() == 1 => format!("tcp dport {}", p[0]),
            Some(p) => format!(
                "tcp dport {{ {} }}",
                p.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")
            ),
            None => "meta l4proto tcp".to_string(),
        };
        let _ = writeln!(r, "\t\t{selector}{ports} redirect to :{}", self.port);
        let _ = writeln!(r, "\t}}");
        let _ = writeln!(r, "}}");
        r
    }

    /// iptables / ip6tables commands (without the privilege prefix).
    fn iptables(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (tool, loopback, v6) in [
            ("iptables", "127.0.0.0/8", false),
            ("ip6tables", "::1/128", true),
        ] {
            let t = format!("{tool} -t nat");
            out.push(format!(
                "{t} -N {IPT_CHAIN} 2>/dev/null || {t} -F {IPT_CHAIN}"
            ));
            if self.mark != 0 {
                out.push(format!(
                    "{t} -A {IPT_CHAIN} -m mark --mark {:#x} -j RETURN",
                    self.mark
                ));
            }
            if let Some(uid) = self.exclude_uid {
                out.push(format!(
                    "{t} -A {IPT_CHAIN} -m owner --uid-owner {uid} -j RETURN"
                ));
            }
            out.push(format!("{t} -A {IPT_CHAIN} -d {loopback} -j RETURN"));
            for c in self.excludes.iter().filter(|c| c.v6 == v6) {
                out.push(format!("{t} -A {IPT_CHAIN} -d {} -j RETURN", c.text));
            }
            let selector = match &self.selector {
                Selector::Cgroup(g) => format!(" -m cgroup --path {}", sh_quote(&g.path)),
                Selector::Uid(uid) => format!(" -m owner --uid-owner {uid}"),
                Selector::All => String::new(),
            };
            let ports = match &self.ports {
                Some(p) if p.len() == 1 => format!(" --dport {}", p[0]),
                Some(p) => format!(
                    " -m multiport --dports {}",
                    p.iter().map(u16::to_string).collect::<Vec<_>>().join(",")
                ),
                None => String::new(),
            };
            out.push(format!(
                "{t} -A {IPT_CHAIN} -p tcp{ports}{selector} -j REDIRECT --to-ports {}",
                self.port
            ));
            out.push(format!(
                "{t} -C OUTPUT -p tcp -j {IPT_CHAIN} 2>/dev/null || {t} -A OUTPUT -p tcp -j {IPT_CHAIN}"
            ));
        }
        out
    }
}

fn iptables_teardown() -> Vec<String> {
    let mut out = Vec::new();
    for tool in ["iptables", "ip6tables"] {
        let t = format!("{tool} -t nat");
        out.push(format!("{t} -D OUTPUT -p tcp -j {IPT_CHAIN}"));
        out.push(format!("{t} -F {IPT_CHAIN}"));
        out.push(format!("{t} -X {IPT_CHAIN}"));
    }
    out
}

/// Prefix every line that runs a privileged tool with `sudo` (also after `||`).
fn privileged(lines: &[String], sudo: bool) -> String {
    lines
        .iter()
        .map(|l| {
            if sudo {
                l.split(" || ")
                    .map(|part| match part.split_once(" 2>/dev/null") {
                        Some((cmd, rest)) => format!("sudo {cmd} 2>/dev/null{rest}"),
                        None => format!("sudo {part}"),
                    })
                    .collect::<Vec<_>>()
                    .join(" || ")
            } else {
                l.clone()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn root(cmd: &str, sudo: bool) -> String {
    if sudo {
        format!("sudo {cmd}")
    } else {
        cmd.to_string()
    }
}

/// `curl` that prints `200` only when connections to port `port` reach Akev.
fn curl_check(port: u16) -> String {
    let url = if port == 80 {
        "http://akev.cert/".to_string()
    } else {
        format!("http://akev.cert:{port}/")
    };
    format!(
        "curl -sS -o /dev/null -w '%{{http_code}}\\n' --max-time 5 --resolve akev.cert:{port}:{TEST_IP} {url}"
    )
}

/// Generate the commands for `req`.
pub fn generate(req: &PlanRequest, ctx: &PlanContext) -> Result<Plan, PlanError> {
    let facts = &ctx.facts;
    if facts.platform != "linux" {
        return err(
            "transparent capture is Linux-only for now (macOS pf and Windows WinDivert aren't supported yet)",
        );
    }
    if ctx.port == 0 {
        return err("the transparent listener has no port yet: turn it on first");
    }
    let ports = ports_of(req)?;
    let mut excludes = Vec::new();
    for d in &req.exclude_destinations {
        if d.trim().is_empty() {
            continue;
        }
        match parse_cidr(d) {
            Some(c) => excludes.push(c),
            None => return err(format!("{d:?} is not an IP address or CIDR range")),
        }
    }
    let sudo = req.sudo;
    let mut warnings = Vec::new();
    let mut notes = Vec::new();
    let mut setup = Vec::new();
    let mut launch = None;
    let mut teardown = Vec::new();
    let akev_uid = facts.uid;
    let who = match (&facts.user, akev_uid) {
        (Some(u), Some(uid)) => format!("{u} (uid {uid})"),
        (None, Some(uid)) => format!("uid {uid}"),
        _ => "Akev's user".to_string(),
    };

    let (selector, exclude_uid, summary) = match req.mode {
        CaptureMode::App => {
            if !facts.cgroup_v2 {
                return err("per-app capture needs the cgroup v2 hierarchy (/sys/fs/cgroup)");
            }
            let group = capture_group(req, facts)?;
            let summary = match req.launcher {
                Launcher::Systemd => {
                    format!("Programs launched in the {CAPTURE_SLICE} systemd slice")
                }
                Launcher::Cgroupfs => {
                    format!("Programs started from shells moved into the {CAPTURE_CGROUP} cgroup")
                }
            };
            (Selector::Cgroup(group), None, summary)
        }
        CaptureMode::User => {
            let Some((uid, name)) = ctx.target_user.clone() else {
                return err(match req.user.as_deref().map(str::trim) {
                    Some(u) if !u.is_empty() => format!("no user {u:?} on this machine"),
                    _ => "choose the user account to capture".to_string(),
                });
            };
            if Some(uid) == akev_uid {
                if ctx.mark == 0 {
                    return err(format!(
                        "Akev itself runs as {who}: capturing that user would redirect Akev's own connections. Run the apps as another user, use per-app capture, or set a firewall mark (Settings → Proxy) so Akev's sockets are excluded."
                    ));
                }
                warnings.push(format!(
                    "Akev runs as the captured user: only the firewall mark {:#x} keeps its own connections out.",
                    ctx.mark
                ));
            }
            let label = name.map_or_else(|| format!("uid {uid}"), |n| format!("{n} (uid {uid})"));
            (
                Selector::Uid(uid),
                None,
                format!("Every program run by {label}"),
            )
        }
        CaptureMode::Machine => match req.exclude {
            Exclusion::Uid => {
                let Some(uid) = akev_uid else {
                    return err("Akev's user id is unknown: exclude it by firewall mark instead");
                };
                if uid != 0 {
                    warnings.push(format!(
                        "Programs run by {who} — the user Akev runs as — are not captured (their connections are indistinguishable from Akev's). Run Akev as a separate user, exclude it by firewall mark, or use per-app capture for your own programs."
                    ));
                }
                (
                    Selector::All,
                    Some(uid),
                    format!("Every program on this machine except those of {who}"),
                )
            }
            Exclusion::Mark => {
                if ctx.mark == 0 {
                    return err(
                        "set a firewall mark first (Settings → Proxy → Transparent capture), so Akev can mark its own connections",
                    );
                }
                if let Some(e) = &ctx.mark_error {
                    let exe = facts.executable.as_deref().unwrap_or("/path/to/akev");
                    setup.push(Step {
                        title: "Let Akev mark its connections".into(),
                        command: root(&format!("setcap cap_net_admin+ep {}", sh_quote(exe)), sudo),
                        root: true,
                        description: Some(format!(
                            "{e} Restart Akev afterwards. (Re-run after updating Akev; the capability is stored on the file.)"
                        )),
                    });
                    warnings.push(
                        "Until Akev can mark its sockets, its own connections would be redirected back to it (Akev detects and refuses such loops, but those requests fail).".into(),
                    );
                }
                (
                    Selector::All,
                    None,
                    format!(
                        "Every program on this machine except Akev (firewall mark {:#x})",
                        ctx.mark
                    ),
                )
            }
        },
    };

    let rules = Rules {
        selector,
        exclude_uid,
        mark: ctx.mark,
        ports: ports.clone(),
        excludes,
        port: ctx.port,
    };

    // The capture group must exist before rules referring to it are loaded (nft resolves the
    // path to a cgroup id at load time).
    if let Selector::Cgroup(group) = &rules.selector {
        match req.launcher {
            Launcher::Systemd => {
                if !facts.systemd_run {
                    warnings.push("systemd-run wasn't found; use the “without systemd” launcher if this machine doesn't run systemd.".into());
                }
                let user_flag = if facts.uid == Some(0) { "" } else { " --user" };
                setup.push(Step {
                    title: "Create the capture slice".into(),
                    command: format!("systemctl{user_flag} start {CAPTURE_SLICE}"),
                    root: false,
                    description: Some(format!(
                        "Run as yourself. Programs launched in this slice ({}) are captured; it stays until you stop it or log out.",
                        group.path
                    )),
                });
                let cmd = launch_command(req)?;
                let env = if req.trust_ca && !ctx.ca_env.is_empty() {
                    let vars = ctx
                        .ca_env
                        .iter()
                        .map(|(k, v)| format!("{k}={}", sh_quote(v)))
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("env {vars} ")
                } else {
                    String::new()
                };
                launch = Some(Step {
                    title: "Launch programs inside the slice".into(),
                    command: format!(
                        "systemd-run{user_flag} --scope --quiet --slice={CAPTURE_SLICE} -- {env}{cmd}"
                    ),
                    root: false,
                    description: Some(
                        if req.command.as_deref().is_none_or(|c| c.trim().is_empty()) {
                            "Opens a shell whose programs are all captured. Programs already running elsewhere are not.".into()
                        } else {
                            "Programs already running outside the slice are not captured: quit them first (browsers reuse a running instance).".into()
                        },
                    ),
                });
                notes.push(format!(
                    "Check where a program runs with: cat /proc/<pid>/cgroup (it must end in /{CAPTURE_SLICE}/run-…scope)."
                ));
            }
            Launcher::Cgroupfs => {
                setup.push(Step {
                    title: "Create the capture cgroup".into(),
                    command: root(&format!("mkdir -p /sys/fs/cgroup/{CAPTURE_CGROUP}"), sudo),
                    root: true,
                    description: None,
                });
                launch = Some(Step {
                    title: "Capture a shell".into(),
                    command: format!(
                        "echo $$ | {} > /dev/null",
                        root(&format!("tee /sys/fs/cgroup/{CAPTURE_CGROUP}/cgroup.procs"), sudo)
                    ),
                    root: true,
                    description: Some(
                        "Moves the current shell into the capture cgroup: everything you start from it afterwards is captured.".into(),
                    ),
                });
                if req.trust_ca && !ctx.ca_env.is_empty() {
                    notes.push(format!(
                        "To make programs in that shell trust Akev's CA: export {}",
                        ctx.ca_env
                            .iter()
                            .map(|(k, v)| format!("{k}={}", sh_quote(v)))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ));
                }
            }
        }
    }

    let (rules_text, load) = match req.backend {
        Backend::Nft => {
            if !facts.nft {
                warnings.push("nft wasn't found on this machine; install nftables or use the iptables commands.".into());
            }
            let ruleset = rules.nft_ruleset();
            let cmd = format!(
                "{} <<'EOF'\ntable inet {NFT_TABLE}\ndelete table inet {NFT_TABLE}\n{ruleset}EOF",
                root("nft -f -", sudo)
            );
            (ruleset, cmd)
        }
        Backend::Iptables => {
            if !facts.iptables {
                warnings.push(
                    "iptables wasn't found on this machine; use the nftables commands.".into(),
                );
            }
            let lines = rules.iptables();
            (lines.join("\n"), privileged(&lines, sudo))
        }
    };
    setup.push(Step {
        title: "Redirect connections to Akev".into(),
        command: load,
        root: true,
        description: Some(format!(
            "Loads the rules (replacing earlier Akev rules) that send {} to port {}.",
            match &ports {
                Some(p) => format!(
                    "TCP connections to port{} {}",
                    if p.len() == 1 { "" } else { "s" },
                    p.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")
                ),
                None => "all TCP connections".to_string(),
            },
            ctx.port
        )),
    });

    // Teardown: rules first (so nothing points at a closed port), then the capture group.
    teardown.push(Step {
        title: "Remove the redirect rules".into(),
        command: match req.backend {
            Backend::Nft => root(&format!("nft delete table inet {NFT_TABLE}"), sudo),
            Backend::Iptables => privileged(&iptables_teardown(), sudo),
        },
        root: true,
        description: None,
    });
    if matches!(rules.selector, Selector::Cgroup(_)) {
        teardown.push(match req.launcher {
            Launcher::Systemd => Step {
                title: "Stop the capture slice".into(),
                command: format!(
                    "systemctl{} stop {CAPTURE_SLICE}",
                    if facts.uid == Some(0) { "" } else { " --user" }
                ),
                root: false,
                description: Some("Also ends programs still running in it.".into()),
            },
            Launcher::Cgroupfs => Step {
                title: "Remove the capture cgroup".into(),
                command: root(&format!("rmdir /sys/fs/cgroup/{CAPTURE_CGROUP}"), sudo),
                root: true,
                description: Some("Close the captured shells (and their programs) first.".into()),
            },
        });
    }

    // Verification: a request to an unroutable address that only Akev can answer.
    let test_port = match &ports {
        None => 80,
        Some(p) if p.contains(&80) => 80,
        Some(p) => p[0],
    };
    let curl = curl_check(test_port);
    let test = match (&rules.selector, req.launcher) {
        (Selector::Cgroup(_), Launcher::Systemd) => Step {
            title: "Check it works".into(),
            command: format!(
                "systemd-run{} --scope --quiet --slice={CAPTURE_SLICE} -- {curl}",
                if facts.uid == Some(0) { "" } else { " --user" }
            ),
            root: false,
            description: Some(
                "Prints 200 when redirection works (and the connection shows up below).".into(),
            ),
        },
        (Selector::Cgroup(_), Launcher::Cgroupfs) => Step {
            title: "Check it works".into(),
            command: curl.clone(),
            root: false,
            description: Some(
                "Run it in the captured shell: prints 200 when redirection works.".into(),
            ),
        },
        (Selector::Uid(uid), _) => Step {
            title: "Check it works".into(),
            command: format!("sudo -u '#{uid}' {curl}"),
            root: true,
            description: Some(
                "Runs curl as the captured user: prints 200 when redirection works.".into(),
            ),
        },
        (Selector::All, _) => {
            let as_root = rules.exclude_uid.is_some_and(|u| u != 0);
            Step {
                title: "Check it works".into(),
                command: if as_root {
                    root(&curl, true)
                } else {
                    curl.clone()
                },
                root: as_root,
                description: Some(if as_root {
                    "Runs curl as root (your own user is excluded): prints 200 when redirection works.".into()
                } else {
                    "Prints 200 when redirection works.".into()
                }),
            }
        }
    };

    if !ctx.listening {
        warnings.insert(
            0,
            format!(
                "The transparent listener isn't running: redirected connections to port {} will fail until you turn it on.",
                ctx.port
            ),
        );
    }
    if ports.is_none() {
        warnings.push(
            "Redirecting every TCP port also catches SSH, databases and other non-HTTP protocols: Akev tunnels them through unchanged, but they now depend on Akev running.".into(),
        );
    }
    notes.push(
        "Only TCP is redirected: QUIC / HTTP/3 (UDP 443) isn't, and browsers fall back to HTTPS over TCP when QUIC fails — block UDP 443 if a program keeps using QUIC.".into(),
    );
    notes.push(
        "HTTPS is intercepted only if the program trusts Akev's CA (browsers use their own trust store: use the browser interceptors or trust the CA system-wide).".into(),
    );
    notes.push(
        "While the rules are loaded and Akev isn't running, the redirected connections fail: run the teardown commands when you're done.".into(),
    );
    if req.backend == Backend::Nft {
        notes.push("The commands use a here-document: paste them into bash or zsh (in fish, run bash first).".into());
    }

    Ok(Plan {
        mode: req.mode,
        backend: req.backend,
        port: ctx.port,
        summary,
        setup,
        launch,
        test,
        teardown,
        rules: rules_text,
        warnings,
        notes,
    })
}

fn launch_command(req: &PlanRequest) -> Result<String, PlanError> {
    match req.command.as_deref().map(str::trim) {
        None | Some("") => Ok("\"$SHELL\"".to_string()),
        Some(c) if c.contains(['\n', '\r']) => err("the command must be a single line"),
        Some(c) => Ok(c.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> SystemFacts {
        SystemFacts {
            platform: "linux".into(),
            uid: Some(1000),
            user: Some("dev".into()),
            cgroup_v2: true,
            own_cgroup: Some(
                "/user.slice/user-1000.slice/user@1000.service/app.slice/t.scope".into(),
            ),
            user_manager_cgroup: Some("user.slice/user-1000.slice/user@1000.service".into()),
            nft: true,
            iptables: true,
            systemd_run: true,
            executable: Some("/opt/Akev/akev".into()),
        }
    }

    fn ctx() -> PlanContext {
        PlanContext {
            facts: facts(),
            port: 8001,
            listening: true,
            mark: 0,
            mark_error: None,
            ca_env: vec![
                (
                    "SSL_CERT_FILE".into(),
                    "/home/me/.local/share/akev/ca-bundle.pem".into(),
                ),
                (
                    "NODE_EXTRA_CA_CERTS".into(),
                    "/home/me/My Data/ca.pem".into(),
                ),
            ],
            target_user: None,
        }
    }

    const SLICE_PATH: &str =
        "user.slice/user-1000.slice/user@1000.service/akev.slice/akev-capture.slice";

    #[test]
    fn per_app_nft_plan() {
        let plan = generate(&PlanRequest::default(), &ctx()).unwrap();
        assert_eq!(
            (plan.mode, plan.backend, plan.port),
            (CaptureMode::App, Backend::Nft, 8001)
        );
        assert_eq!(
            plan.rules,
            format!(
                "table inet akev {{\n\
                 \tchain output {{\n\
                 \t\ttype nat hook output priority -100; policy accept;\n\
                 \t\tip daddr 127.0.0.0/8 return\n\
                 \t\tip6 daddr ::1 return\n\
                 \t\tsocket cgroupv2 level 5 \"{SLICE_PATH}\" tcp dport {{ 80, 443 }} redirect to :8001\n\
                 \t}}\n\
                 }}\n"
            )
        );
        assert_eq!(plan.setup.len(), 2);
        assert_eq!(
            plan.setup[0].command,
            "systemctl --user start akev-capture.slice"
        );
        assert!(!plan.setup[0].root);
        // Idempotent, atomic reload of the table.
        assert_eq!(
            plan.setup[1].command,
            format!(
                "sudo nft -f - <<'EOF'\ntable inet akev\ndelete table inet akev\n{}EOF",
                plan.rules
            )
        );
        assert!(plan.setup[1].root);
        let launch = plan.launch.as_ref().unwrap();
        assert_eq!(
            launch.command,
            "systemd-run --user --scope --quiet --slice=akev-capture.slice -- env \
             SSL_CERT_FILE=/home/me/.local/share/akev/ca-bundle.pem \
             NODE_EXTRA_CA_CERTS='/home/me/My Data/ca.pem' \"$SHELL\""
        );
        assert_eq!(
            plan.test.command,
            "systemd-run --user --scope --quiet --slice=akev-capture.slice -- curl -sS -o /dev/null \
             -w '%{http_code}\\n' --max-time 5 --resolve akev.cert:80:192.0.2.1 http://akev.cert/"
        );
        assert_eq!(
            plan.teardown
                .iter()
                .map(|s| s.command.as_str())
                .collect::<Vec<_>>(),
            vec![
                "sudo nft delete table inet akev",
                "systemctl --user stop akev-capture.slice"
            ]
        );
        assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
        assert!(plan.notes.iter().any(|n| n.contains("QUIC")));
    }

    #[test]
    fn per_app_with_custom_command_ports_and_no_ca() {
        let req = PlanRequest {
            command: Some("  firefox --new-instance  ".into()),
            ports: PortScope::Custom,
            custom_ports: vec![8443, 0, 8080, 8443],
            trust_ca: false,
            sudo: false,
            ..Default::default()
        };
        let plan = generate(&req, &ctx()).unwrap();
        assert!(
            plan.rules
                .contains("tcp dport { 8080, 8443 } redirect to :8001"),
            "{}",
            plan.rules
        );
        assert_eq!(
            plan.launch.unwrap().command,
            "systemd-run --user --scope --quiet --slice=akev-capture.slice -- firefox --new-instance"
        );
        assert!(plan.setup[1].command.starts_with("nft -f - <<'EOF'"));
        assert_eq!(plan.teardown[0].command, "nft delete table inet akev");
        // No port 80: the check uses the first custom port.
        assert!(
            plan.test
                .command
                .ends_with("--resolve akev.cert:8080:192.0.2.1 http://akev.cert:8080/")
        );
        // A single custom port needs no set braces.
        let one = PlanRequest {
            ports: PortScope::Custom,
            custom_ports: vec![3000],
            ..Default::default()
        };
        assert!(
            generate(&one, &ctx())
                .unwrap()
                .rules
                .contains("tcp dport 3000 redirect")
        );
    }

    #[test]
    fn per_app_iptables_plan() {
        let req = PlanRequest {
            backend: Backend::Iptables,
            ..Default::default()
        };
        let plan = generate(&req, &ctx()).unwrap();
        let load = &plan.setup[1].command;
        let lines: Vec<&str> = load.lines().collect();
        assert_eq!(
            lines[0],
            "sudo iptables -t nat -N AKEV 2>/dev/null || sudo iptables -t nat -F AKEV"
        );
        assert_eq!(
            lines[1],
            "sudo iptables -t nat -A AKEV -d 127.0.0.0/8 -j RETURN"
        );
        assert_eq!(
            lines[2],
            format!(
                "sudo iptables -t nat -A AKEV -p tcp -m multiport --dports 80,443 -m cgroup --path {SLICE_PATH} -j REDIRECT --to-ports 8001"
            )
        );
        assert_eq!(
            lines[3],
            "sudo iptables -t nat -C OUTPUT -p tcp -j AKEV 2>/dev/null || sudo iptables -t nat -A OUTPUT -p tcp -j AKEV"
        );
        assert_eq!(
            lines[5],
            "sudo ip6tables -t nat -A AKEV -d ::1/128 -j RETURN"
        );
        assert_eq!(lines.len(), 8);
        assert_eq!(
            plan.teardown[0].command,
            "sudo iptables -t nat -D OUTPUT -p tcp -j AKEV\nsudo iptables -t nat -F AKEV\nsudo iptables -t nat -X AKEV\n\
             sudo ip6tables -t nat -D OUTPUT -p tcp -j AKEV\nsudo ip6tables -t nat -F AKEV\nsudo ip6tables -t nat -X AKEV"
        );
        assert!(!plan.notes.iter().any(|n| n.contains("here-document")));
    }

    #[test]
    fn per_app_without_systemd() {
        let req = PlanRequest {
            launcher: Launcher::Cgroupfs,
            ..Default::default()
        };
        let plan = generate(&req, &ctx()).unwrap();
        assert!(
            plan.rules
                .contains("socket cgroupv2 level 1 \"akev-capture\" tcp dport { 80, 443 }")
        );
        assert_eq!(
            plan.setup[0].command,
            "sudo mkdir -p /sys/fs/cgroup/akev-capture"
        );
        assert_eq!(
            plan.launch.as_ref().unwrap().command,
            "echo $$ | sudo tee /sys/fs/cgroup/akev-capture/cgroup.procs > /dev/null"
        );
        assert_eq!(
            plan.teardown[1].command,
            "sudo rmdir /sys/fs/cgroup/akev-capture"
        );
        assert!(plan.test.command.starts_with("curl "));
        assert!(
            plan.notes
                .iter()
                .any(|n| n.starts_with("To make programs in that shell trust"))
        );
    }

    #[test]
    fn per_app_as_root_uses_a_system_slice() {
        let mut c = ctx();
        c.facts.uid = Some(0);
        c.facts.user = Some("root".into());
        let plan = generate(&PlanRequest::default(), &c).unwrap();
        assert!(
            plan.rules
                .contains("socket cgroupv2 level 2 \"akev.slice/akev-capture.slice\"")
        );
        assert_eq!(plan.setup[0].command, "systemctl start akev-capture.slice");
        assert!(
            plan.launch
                .unwrap()
                .command
                .starts_with("systemd-run --scope")
        );
    }

    #[test]
    fn per_user_plan() {
        let mut c = ctx();
        c.target_user = Some((1001, Some("capture".into())));
        let req = PlanRequest {
            mode: CaptureMode::User,
            user: Some("capture".into()),
            ports: PortScope::All,
            ..Default::default()
        };
        let plan = generate(&req, &c).unwrap();
        assert!(
            plan.rules
                .contains("\t\tmeta skuid 1001 meta l4proto tcp redirect to :8001\n"),
            "{}",
            plan.rules
        );
        assert!(!plan.rules.contains("return\n\t\tmeta skuid 1000"));
        assert_eq!(plan.setup.len(), 1);
        assert!(plan.launch.is_none());
        assert_eq!(plan.teardown.len(), 1);
        assert!(plan.summary.contains("capture (uid 1001)"));
        assert_eq!(
            plan.test.command,
            "sudo -u '#1001' curl -sS -o /dev/null -w '%{http_code}\\n' --max-time 5 --resolve akev.cert:80:192.0.2.1 http://akev.cert/"
        );
        assert!(plan.warnings.iter().any(|w| w.contains("SSH")));
        let ipt = generate(
            &PlanRequest {
                backend: Backend::Iptables,
                ..req.clone()
            },
            &c,
        )
        .unwrap();
        assert!(ipt.rules.contains("iptables -t nat -A AKEV -p tcp -m owner --uid-owner 1001 -j REDIRECT --to-ports 8001"), "{}", ipt.rules);

        // Capturing Akev's own user would loop.
        c.target_user = Some((1000, Some("dev".into())));
        let e = generate(&req, &c).unwrap_err();
        assert!(e.0.contains("Akev itself runs as dev (uid 1000)"), "{e}");
        // ...unless its sockets are marked.
        c.mark = 0x2a;
        let plan = generate(&req, &c).unwrap();
        assert!(plan.rules.contains("meta mark 0x2a return"));
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.contains("firewall mark 0x2a"))
        );

        c.target_user = None;
        assert_eq!(
            generate(
                &PlanRequest {
                    user: Some("ghost".into()),
                    ..req.clone()
                },
                &c
            )
            .unwrap_err()
            .0,
            "no user \"ghost\" on this machine"
        );
        assert!(
            generate(&PlanRequest { user: None, ..req }, &c)
                .unwrap_err()
                .0
                .contains("choose the user")
        );
    }

    #[test]
    fn machine_plan_excludes_akev_by_uid() {
        let req = PlanRequest {
            mode: CaptureMode::Machine,
            exclude_destinations: vec!["10.0.0.0/8".into(), " fd00::/8 ".into(), "".into()],
            ..Default::default()
        };
        let plan = generate(&req, &ctx()).unwrap();
        assert_eq!(
            plan.rules,
            "table inet akev {\n\
             \tchain output {\n\
             \t\ttype nat hook output priority -100; policy accept;\n\
             \t\tmeta skuid 1000 return\n\
             \t\tip daddr 127.0.0.0/8 return\n\
             \t\tip6 daddr ::1 return\n\
             \t\tip daddr 10.0.0.0/8 return\n\
             \t\tip6 daddr fd00::/8 return\n\
             \t\ttcp dport { 80, 443 } redirect to :8001\n\
             \t}\n\
             }\n"
        );
        assert!(plan.warnings.iter().any(|w| w.contains("dev (uid 1000)")));
        // Akev's own user isn't captured, so the check runs as root.
        assert!(plan.test.command.starts_with("sudo curl ") && plan.test.root);
        assert!(plan.launch.is_none());
        let ipt = generate(
            &PlanRequest {
                backend: Backend::Iptables,
                ..req
            },
            &ctx(),
        )
        .unwrap();
        assert!(
            ipt.rules
                .contains("iptables -t nat -A AKEV -m owner --uid-owner 1000 -j RETURN")
        );
        assert!(
            ipt.rules
                .contains("iptables -t nat -A AKEV -d 10.0.0.0/8 -j RETURN")
        );
        assert!(
            ipt.rules
                .contains("ip6tables -t nat -A AKEV -d fd00::/8 -j RETURN")
        );
        assert!(!ipt.rules.contains("iptables -t nat -A AKEV -d fd00::/8"));

        // Akev as root: nothing else is excluded, no warning.
        let mut c = ctx();
        c.facts.uid = Some(0);
        let plan = generate(
            &PlanRequest {
                mode: CaptureMode::Machine,
                ..Default::default()
            },
            &c,
        )
        .unwrap();
        assert!(plan.rules.contains("meta skuid 0 return"));
        assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
        assert!(plan.test.command.starts_with("curl "));
    }

    #[test]
    fn machine_plan_excludes_akev_by_mark() {
        let req = PlanRequest {
            mode: CaptureMode::Machine,
            exclude: Exclusion::Mark,
            ..Default::default()
        };
        assert!(
            generate(&req, &ctx())
                .unwrap_err()
                .0
                .contains("set a firewall mark first")
        );
        let mut c = ctx();
        c.mark = 0x1a4b;
        let plan = generate(&req, &c).unwrap();
        assert!(plan.rules.contains("\t\tmeta mark 0x1a4b return\n"));
        assert!(!plan.rules.contains("skuid"));
        assert_eq!(plan.setup.len(), 1, "Akev can mark: no setcap step");
        assert!(plan.test.command.starts_with("curl "));
        // Without the capability, the plan starts by granting it.
        c.mark_error = Some("Akev lacks CAP_NET_ADMIN.".into());
        let plan = generate(&req, &c).unwrap();
        assert_eq!(
            plan.setup[0].command,
            "sudo setcap cap_net_admin+ep /opt/Akev/akev"
        );
        assert!(plan.warnings.iter().any(|w| w.contains("loops")));
        let ipt = generate(
            &PlanRequest {
                backend: Backend::Iptables,
                ..req
            },
            &c,
        )
        .unwrap();
        assert!(
            ipt.rules
                .contains("iptables -t nat -A AKEV -m mark --mark 0x1a4b -j RETURN")
        );
    }

    #[test]
    fn rejects_bad_requests() {
        let bad = |req: PlanRequest, c: &PlanContext| generate(&req, c).unwrap_err().0;
        assert!(
            bad(
                PlanRequest {
                    ports: PortScope::Custom,
                    ..Default::default()
                },
                &ctx()
            )
            .contains("at least one")
        );
        assert!(
            bad(
                PlanRequest {
                    command: Some("a\nb".into()),
                    ..Default::default()
                },
                &ctx()
            )
            .contains("single line")
        );
        assert!(
            bad(
                PlanRequest {
                    exclude_destinations: vec!["10.0.0.0/33".into()],
                    ..Default::default()
                },
                &ctx()
            )
            .contains("not an IP")
        );
        assert!(
            bad(
                PlanRequest {
                    exclude_destinations: vec!["example.com".into()],
                    ..Default::default()
                },
                &ctx()
            )
            .contains("not an IP")
        );
        let mut c = ctx();
        c.facts.platform = "macos".into();
        assert!(bad(PlanRequest::default(), &c).contains("Linux-only"));
        let mut c = ctx();
        c.facts.cgroup_v2 = false;
        assert!(bad(PlanRequest::default(), &c).contains("cgroup v2"));
        assert!(
            generate(
                &PlanRequest {
                    mode: CaptureMode::Machine,
                    ..Default::default()
                },
                &c
            )
            .is_ok()
        );
        let mut c = ctx();
        c.port = 0;
        assert!(bad(PlanRequest::default(), &c).contains("turn it on"));
        let mut c = ctx();
        c.facts.uid = None;
        assert!(
            bad(
                PlanRequest {
                    mode: CaptureMode::Machine,
                    ..Default::default()
                },
                &c
            )
            .contains("unknown")
        );
    }

    #[test]
    fn warns_about_missing_tools_and_listener() {
        let mut c = ctx();
        c.listening = false;
        c.facts.nft = false;
        c.facts.systemd_run = false;
        let plan = generate(&PlanRequest::default(), &c).unwrap();
        assert!(plan.warnings[0].contains("isn't running"));
        assert!(plan.warnings.iter().any(|w| w.contains("nft wasn't found")));
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.contains("systemd-run wasn't found"))
        );
    }

    #[test]
    fn request_json_defaults() {
        let req: PlanRequest =
            serde_json::from_str(r#"{"mode":"machine","ports":"all","exclude":"mark"}"#).unwrap();
        assert_eq!(req.mode, CaptureMode::Machine);
        assert_eq!(req.ports, PortScope::All);
        assert_eq!(req.exclude, Exclusion::Mark);
        assert!(req.trust_ca && req.sudo);
        let plan = generate(&PlanRequest::default(), &ctx()).unwrap();
        let v = serde_json::to_value(&plan).unwrap();
        assert_eq!(v["mode"], "app");
        assert_eq!(v["backend"], "nft");
        assert!(v["setup"][0]["root"] == false && v["launch"]["command"].is_string());
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("/usr/bin/akev"), "/usr/bin/akev");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("$HOME"), "'$HOME'");
    }
}
