//! Terminal interceptors.
//!
//! * `fresh-terminal` opens a new terminal window whose shell has all proxy/CA variables set.
//!   Rather than relying on the emulator passing our environment through (GApplication
//!   terminals like gnome-terminal or ptyxis often hand the launch to an already-running
//!   server with *its* environment), the terminal runs a generated launcher script that
//!   exports everything and then `exec`s the user's shell. A unique `AKEV_TERMINAL_ID`
//!   marker lets us find (and close) those shells later even when the emulator process we
//!   started has exited. Every process started from such a shell inherits the marker, so the
//!   launcher also records the shell's own pid (`AKEV_TERMINAL_PID`): only the terminal's
//!   shell itself is closed — with SIGHUP, like closing its window, never SIGKILL — and
//!   `nohup`/`setsid` jobs, tmux sessions or editors started from it keep running.
//! * `existing-terminal` hands out one-liners that fetch the setup script from the API.

use crate::envvars::{ensure_java_truststore, local_env_vars};
use crate::host::Os;
use crate::launch::{LAUNCH_SETTLE, Tracker, log_tail, stop_all};
use crate::runner::{CommandSpec, ProcessSignal};
use crate::scripts::{
    Shell, ensure_cmd_reset_file, fish_quote, posix_launcher, posix_quote, ps_quote, setup_script,
};
use crate::util::{
    Descriptor, PROBE_TTL, ProbeCache, Sys, local_proxy_url, make_executable, opt_str, short_hash,
    signal_foreign, tags,
};
use crate::{InterceptContext, InterceptError, Interceptor, Result};
use akev_core::interceptor::{ActivationResult, InterceptorCategory, InterceptorInfo};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Marker variable identifying shells started by `fresh-terminal`.
pub const TERMINAL_ID_VAR: &str = "AKEV_TERMINAL_ID";
/// Pid of the shell a `fresh-terminal` launcher started (inherited unchanged by everything
/// started from it, so only that shell's own value equals its pid).
pub const TERMINAL_PID_VAR: &str = "AKEV_TERMINAL_PID";

/// Process names that are interactive shells (what we close on deactivation).
const SHELL_NAMES: &[&str] = &[
    "bash",
    "zsh",
    "fish",
    "sh",
    "dash",
    "ksh",
    "mksh",
    "tcsh",
    "csh",
    "nu",
    "xonsh",
    "elvish",
    "pwsh",
    "powershell",
    "cmd",
    "pwsh.exe",
    "powershell.exe",
    "cmd.exe",
];

/// How the command to run is passed to an emulator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExecStyle {
    /// `term [opts] -- cmd args...`
    DoubleDash,
    /// `term [opts] -e cmd args...`
    DashE,
    /// `term [opts] -x cmd args...`
    DashX,
    /// `term [opts] cmd args...`
    Trailing,
    /// `term [opts] -e "cmd args"` (one string)
    DashEString,
}

/// A known Linux terminal emulator.
#[derive(Debug)]
struct TermDef {
    bin: &'static str,
    label: &'static str,
    args: &'static [&'static str],
    style: ExecStyle,
}

const LINUX_TERMINALS: &[TermDef] = &[
    // -s: standalone instance (inherits our env, and lives as long as its window).
    TermDef {
        bin: "ptyxis",
        label: "Ptyxis",
        args: &["-s", "--new-window", "-T", "Akev"],
        style: ExecStyle::DoubleDash,
    },
    TermDef {
        bin: "gnome-terminal",
        label: "GNOME Terminal",
        args: &["--wait"],
        style: ExecStyle::DoubleDash,
    },
    TermDef {
        bin: "kgx",
        label: "GNOME Console",
        args: &[],
        style: ExecStyle::DoubleDash,
    },
    TermDef {
        bin: "konsole",
        label: "Konsole",
        args: &["--nofork"],
        style: ExecStyle::DashE,
    },
    TermDef {
        bin: "xfce4-terminal",
        label: "Xfce Terminal",
        args: &["--disable-server"],
        style: ExecStyle::DashX,
    },
    TermDef {
        bin: "kitty",
        label: "kitty",
        args: &[],
        style: ExecStyle::Trailing,
    },
    TermDef {
        bin: "alacritty",
        label: "Alacritty",
        args: &[],
        style: ExecStyle::DashE,
    },
    TermDef {
        bin: "wezterm",
        label: "WezTerm",
        args: &["start", "--always-new-process"],
        style: ExecStyle::DoubleDash,
    },
    TermDef {
        bin: "foot",
        label: "foot",
        args: &[],
        style: ExecStyle::Trailing,
    },
    TermDef {
        bin: "ghostty",
        label: "Ghostty",
        args: &[],
        style: ExecStyle::DashE,
    },
    TermDef {
        bin: "tilix",
        label: "Tilix",
        args: &[],
        style: ExecStyle::DashEString,
    },
    TermDef {
        bin: "terminator",
        label: "Terminator",
        args: &["-u"],
        style: ExecStyle::DashX,
    },
    TermDef {
        bin: "xterm",
        label: "XTerm",
        args: &["-T", "Akev"],
        style: ExecStyle::DashE,
    },
    TermDef {
        bin: "x-terminal-emulator",
        label: "Default terminal",
        args: &[],
        style: ExecStyle::DashE,
    },
];

/// `xdg-terminal-exec` (the freedesktop "preferred terminal" launcher).
const XDG_TERMINAL_EXEC: TermDef = TermDef {
    bin: "xdg-terminal-exec",
    label: "Preferred terminal",
    args: &[],
    style: ExecStyle::Trailing,
};

/// A terminal we can launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Terminal {
    /// A Linux emulator binary.
    Linux {
        id: String,
        label: String,
        path: PathBuf,
        args: Vec<String>,
        style: ExecStyleTag,
    },
    /// macOS: `open -a <app> script.command`.
    MacOs { app: String, open: PathBuf },
    /// Windows Terminal running PowerShell.
    Wt { wt: PathBuf, powershell: PathBuf },
    /// A PowerShell console window.
    PowerShell { path: PathBuf },
    /// A cmd.exe console window.
    Cmd { path: PathBuf },
}

/// Copyable wrapper so `Terminal` can derive `Eq` (private enum inside).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExecStyleTag(ExecStyle);

impl Terminal {
    fn id(&self) -> String {
        match self {
            Terminal::Linux { id, .. } => id.clone(),
            Terminal::MacOs { app, .. } => app.to_lowercase(),
            Terminal::Wt { .. } => "windows-terminal".into(),
            Terminal::PowerShell { .. } => "powershell".into(),
            Terminal::Cmd { .. } => "cmd".into(),
        }
    }

    fn label(&self) -> String {
        match self {
            Terminal::Linux { label, .. } => label.clone(),
            Terminal::MacOs { app, .. } => app.clone(),
            Terminal::Wt { .. } => "Windows Terminal".into(),
            Terminal::PowerShell { .. } => "PowerShell".into(),
            Terminal::Cmd { .. } => "Command Prompt".into(),
        }
    }
}

fn linux_terminal(def: &TermDef, path: PathBuf) -> Terminal {
    Terminal::Linux {
        id: def.bin.to_string(),
        label: def.label.to_string(),
        path,
        args: def.args.iter().map(|a| a.to_string()).collect(),
        style: ExecStyleTag(def.style),
    }
}

/// All usable terminals, most preferred first.
pub(crate) fn detect_terminals(env: &crate::host::HostEnv) -> Vec<Terminal> {
    let mut out: Vec<Terminal> = Vec::new();
    let push = |t: Terminal, out: &mut Vec<Terminal>| {
        if !out.iter().any(|o| o.id() == t.id()) {
            out.push(t);
        }
    };
    match env.os {
        Os::Linux => {
            // 1. $TERMINAL (a common convention for "my terminal").
            if let Some(term) = env.var("TERMINAL") {
                let name = Path::new(term)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let path = if Path::new(term).is_absolute() {
                    Some(PathBuf::from(term))
                } else {
                    env.find_in_path(term)
                };
                if let Some(path) = path.filter(|p| crate::host::is_executable(p)) {
                    let def = LINUX_TERMINALS.iter().find(|d| d.bin == name);
                    let generic = TermDef {
                        bin: "",
                        label: "",
                        args: &[],
                        style: ExecStyle::DashE,
                    };
                    let def = def.unwrap_or(&generic);
                    let mut t = linux_terminal(def, path);
                    if let Terminal::Linux { id, label, .. } = &mut t
                        && def.bin.is_empty()
                    {
                        *id = name.clone();
                        *label = name.clone();
                    }
                    push(t, &mut out);
                }
            }
            // 2. The desktop's own terminal.
            let desktop = env.desktop();
            let preferred: &[&str] = if desktop.contains("GNOME") {
                &["ptyxis", "kgx", "gnome-terminal"]
            } else if desktop.contains("KDE") {
                &["konsole"]
            } else if desktop.contains("XFCE") {
                &["xfce4-terminal"]
            } else {
                &[]
            };
            for bin in preferred {
                if let (Some(def), Some(path)) = (
                    LINUX_TERMINALS.iter().find(|d| d.bin == *bin),
                    env.find_in_path(bin),
                ) {
                    push(linux_terminal(def, path), &mut out);
                }
            }
            // 3. The freedesktop preferred-terminal launcher, then everything else.
            if let Some(path) = env.find_in_path(XDG_TERMINAL_EXEC.bin) {
                push(linux_terminal(&XDG_TERMINAL_EXEC, path), &mut out);
            }
            for def in LINUX_TERMINALS {
                if let Some(path) = env.find_in_path(def.bin) {
                    push(linux_terminal(def, path), &mut out);
                }
            }
        }
        Os::MacOs => {
            let open = env.sys_path("/usr/bin/open");
            let mut app_roots = vec![env.sys_path("/Applications")];
            if let Some(h) = env.home_path("Applications") {
                app_roots.push(h);
            }
            if app_roots.iter().any(|r| r.join("iTerm.app").exists()) {
                push(
                    Terminal::MacOs {
                        app: "iTerm".into(),
                        open: open.clone(),
                    },
                    &mut out,
                );
            }
            if env
                .sys_path("/System/Applications/Utilities/Terminal.app")
                .exists()
                || env
                    .sys_path("/Applications/Utilities/Terminal.app")
                    .exists()
            {
                push(
                    Terminal::MacOs {
                        app: "Terminal".into(),
                        open,
                    },
                    &mut out,
                );
            }
        }
        Os::Windows => {
            let powershell = env.find_in_path("pwsh").or_else(|| {
                let p = env.var_path("SYSTEMROOT").map(|r| {
                    r.join("System32")
                        .join("WindowsPowerShell")
                        .join("v1.0")
                        .join("powershell.exe")
                });
                p.filter(|p| p.is_file())
                    .or_else(|| env.find_in_path("powershell"))
            });
            let wt = env.find_in_path("wt").or_else(|| {
                env.var_path("LOCALAPPDATA")
                    .map(|l| l.join("Microsoft").join("WindowsApps").join("wt.exe"))
                    .filter(|p| p.exists())
            });
            if let (Some(wt), Some(ps)) = (wt, powershell.clone()) {
                push(Terminal::Wt { wt, powershell: ps }, &mut out);
            }
            if let Some(ps) = powershell {
                push(Terminal::PowerShell { path: ps }, &mut out);
            }
            let cmd = env
                .var_path("COMSPEC")
                .filter(|p| p.is_file())
                .or_else(|| {
                    env.var_path("SYSTEMROOT")
                        .map(|r| r.join("System32").join("cmd.exe"))
                        .filter(|p| p.is_file())
                })
                .or_else(|| env.find_in_path("cmd"));
            if let Some(cmd) = cmd {
                push(Terminal::Cmd { path: cmd }, &mut out);
            }
        }
        Os::Other => {}
    }
    out
}

/// The command that opens `terminal` running `script`.
pub(crate) fn terminal_command(
    terminal: &Terminal,
    script: &Path,
    home: Option<&Path>,
) -> CommandSpec {
    let script_s = script.to_string_lossy().into_owned();
    let mut spec = match terminal {
        Terminal::Linux {
            path, args, style, ..
        } => {
            let mut spec = CommandSpec::new(path).args(args.iter());
            let cmd = ["/bin/sh".to_string(), script_s.clone()];
            spec = match style.0 {
                ExecStyle::DoubleDash => spec.arg("--").args(cmd),
                ExecStyle::DashE => spec.arg("-e").args(cmd),
                ExecStyle::DashX => spec.arg("-x").args(cmd),
                ExecStyle::Trailing => spec.args(cmd),
                ExecStyle::DashEString => spec
                    .arg("-e")
                    .arg(format!("/bin/sh {}", posix_quote(&script_s))),
            };
            spec
        }
        Terminal::MacOs { app, open } => CommandSpec::new(open)
            .args(["-a", app.as_str()])
            .arg(script),
        Terminal::Wt { wt, powershell } => CommandSpec::new(wt)
            .args(["-w", "new", "new-tab", "--title", "Akev"])
            .arg(powershell)
            .args(["-NoLogo", "-NoExit", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(script),
        Terminal::PowerShell { path } => CommandSpec::new(path)
            .args(["-NoLogo", "-NoExit", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(script)
            .new_console(true),
        Terminal::Cmd { path } => CommandSpec::new(path)
            .args(["/k", "call"])
            .arg(script)
            .new_console(true),
    };
    if let Some(home) = home {
        spec = spec.current_dir(home);
    }
    spec
}

/// Script file extension and content for `terminal`.
fn launcher_script(
    terminal: &Terminal,
    vars: &[(String, String)],
    proxy_url: &str,
    user_shell: &str,
    data_dir: &Path,
) -> (&'static str, String) {
    match terminal {
        Terminal::Linux { .. } => (
            "sh",
            posix_launcher(vars, proxy_url, user_shell, false, Some(TERMINAL_PID_VAR)),
        ),
        // macOS terminals start login shells.
        Terminal::MacOs { .. } => (
            "command",
            posix_launcher(vars, proxy_url, user_shell, true, Some(TERMINAL_PID_VAR)),
        ),
        Terminal::Wt { .. } | Terminal::PowerShell { .. } => {
            let mut s = setup_script(Shell::PowerShell, vars, proxy_url, None);
            s.push_str(&format!("$env:{TERMINAL_PID_VAR} = \"$PID\"\n"));
            s.push_str("Remove-Item -LiteralPath $PSCommandPath -ErrorAction SilentlyContinue\n");
            ("ps1", s)
        }
        Terminal::Cmd { .. } => {
            let reset = ensure_cmd_reset_file(data_dir);
            (
                "cmd",
                setup_script(Shell::Cmd, vars, proxy_url, reset.as_deref()),
            )
        }
    }
}

/// Kick off Java truststore generation in the background (once per data dir & CA). Does
/// nothing outside a Tokio runtime or when the truststore is already there.
pub(crate) fn prepare_java_truststore(ctx: &InterceptContext, sys: &Arc<Sys>) {
    static STARTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if crate::envvars::java_truststore_if_ready(ctx).is_some() {
        return;
    }
    let key = format!(
        "{}|{}",
        ctx.data_dir.display(),
        short_hash(ctx.ca_pem.as_bytes())
    );
    if !STARTED.get_or_init(Default::default).lock().insert(key) {
        return;
    }
    let (ctx, sys) = (ctx.clone(), sys.clone());
    runtime.spawn(async move {
        ensure_java_truststore(&ctx, &sys).await;
    });
}

/// Is `name` a per-launch launcher script (`akev-<32 hex digits>.{sh,command,ps1,cmd}`)?
fn is_launcher_script(name: &str) -> bool {
    let Some((id, ext)) = name
        .strip_prefix("akev-")
        .and_then(|rest| rest.split_once('.'))
    else {
        return false;
    };
    id.len() == 32
        && id.bytes().all(|b| b.is_ascii_hexdigit())
        && matches!(ext, "sh" | "command" | "ps1" | "cmd")
}

/// `fresh-terminal`.
pub(crate) struct FreshTerminalInterceptor {
    sys: Arc<Sys>,
    detected: ProbeCache<Vec<Terminal>>,
    tracker: Tracker,
    launched: Mutex<Vec<String>>,
}

impl FreshTerminalInterceptor {
    const DESCRIPTOR: Descriptor = Descriptor {
        id: "fresh-terminal",
        name: "Fresh Terminal",
        description: "Open a new terminal where every CLI tool, script and runtime you start is proxied and trusts the Akev CA",
        category: InterceptorCategory::Terminal,
        tags: tags::TERMINAL,
        icon: "terminal",
        needs_config: false,
    };

    pub(crate) fn new(sys: Arc<Sys>) -> Self {
        FreshTerminalInterceptor {
            sys,
            detected: ProbeCache::new(PROBE_TTL),
            tracker: Tracker::default(),
            launched: Mutex::new(Vec::new()),
        }
    }

    async fn terminals(&self) -> Vec<Terminal> {
        let env = &self.sys.env;
        self.detected
            .get(|| async move { detect_terminals(env) })
            .await
    }

    /// The shells our launchers started (one per terminal window): processes carrying one of
    /// our terminal markers whose recorded shell pid is their own. Everything started *from*
    /// such a shell inherits both markers (scripts, `nohup` jobs, tmux servers and panes,
    /// editors' terminals), but with someone else's pid, and is left alone.
    async fn marked_shells(&self) -> Vec<u32> {
        let ids = self.launched.lock().clone();
        if ids.is_empty() {
            return Vec::new();
        }
        let procs = self.sys.processes(true).await;
        let tracked = self.tracker.pids();
        procs
            .iter()
            .filter(|p| !tracked.contains(&p.pid))
            .filter(|p| {
                p.akev_var(TERMINAL_ID_VAR)
                    .is_some_and(|id| ids.iter().any(|i| i == id))
            })
            .filter(|p| {
                p.akev_var(TERMINAL_PID_VAR)
                    .and_then(|v| v.trim().parse::<u32>().ok())
                    == Some(p.pid)
            })
            .filter(|p| SHELL_NAMES.contains(&p.name.to_ascii_lowercase().as_str()))
            .map(|p| p.pid)
            .collect()
    }

    /// Delete launcher scripts older than an hour (they normally delete themselves). Only
    /// per-launch files (`akev-<terminal id>.<ext>`) are touched: the directory also holds
    /// persistent files, like the cmd reset script `akev-off.cmd` that `akev-off` in
    /// already configured cmd windows calls.
    fn cleanup_old_scripts(dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            if !is_launcher_script(&e.file_name().to_string_lossy()) {
                continue;
            }
            let stale = e
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| t.elapsed().unwrap_or_default() > Duration::from_secs(3600))
                .unwrap_or(false);
            if stale {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[async_trait]
impl Interceptor for FreshTerminalInterceptor {
    fn id(&self) -> &str {
        Self::DESCRIPTOR.id
    }

    async fn info(&self, ctx: &InterceptContext) -> InterceptorInfo {
        let d = Self::DESCRIPTOR;
        if self.sys.env.os == Os::Other {
            return d.unsupported("this OS");
        }
        prepare_java_truststore(ctx, &self.sys);
        let terminals = self.terminals().await;
        let Some(first) = terminals.first() else {
            return InterceptorInfo {
                unavailable_reason: Some("No supported terminal emulator found".into()),
                ..d.info()
            };
        };
        let active = !self.tracker.alive().is_empty() || !self.marked_shells().await.is_empty();
        InterceptorInfo {
            available: true,
            active,
            metadata: json!({
                "terminal": first.id(),
                "terminalName": first.label(),
                "terminals": terminals.iter().map(|t| json!({"id": t.id(), "name": t.label()})).collect::<Vec<_>>(),
                "shell": self.sys.env.user_shell(),
            }),
            ..d.info()
        }
    }

    async fn activate(
        &self,
        ctx: &InterceptContext,
        options: serde_json::Value,
    ) -> Result<ActivationResult> {
        let terminals = self.terminals().await;
        let terminal = match opt_str(&options, "terminal") {
            Some(want) => terminals.iter().find(|t| t.id() == want).ok_or_else(|| {
                InterceptError::Unavailable(format!("terminal `{want}` not found"))
            })?,
            None => terminals.first().ok_or_else(|| {
                InterceptError::Unavailable("No supported terminal emulator found".into())
            })?,
        };

        // Include the Java truststore if it can be ready in reasonable time.
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            ensure_java_truststore(ctx, &self.sys),
        )
        .await;

        let terminal_id = uuid::Uuid::new_v4().simple().to_string();
        let mut vars = local_env_vars(ctx, &self.sys.env);
        vars.push((TERMINAL_ID_VAR.to_string(), terminal_id.clone()));
        let proxy_url = local_proxy_url(ctx);

        let dir = ctx.data_dir.join("terminal");
        std::fs::create_dir_all(&dir)?;
        Self::cleanup_old_scripts(&dir);
        if self.sys.env.os == Os::Windows {
            // cmd windows set up with the existing-terminal one-liner call it for `akev-off`
            // (recreated if something removed it).
            ensure_cmd_reset_file(&ctx.data_dir);
        }
        let (ext, content) = launcher_script(
            terminal,
            &vars,
            &proxy_url,
            &self.sys.env.user_shell(),
            &ctx.data_dir,
        );
        let script = dir.join(format!("akev-{terminal_id}.{ext}"));
        crate::util::atomic_write(&script, content.as_bytes())?;
        make_executable(&script)?;

        let log = ctx.data_dir.join("logs").join("fresh-terminal.log");
        let spec = terminal_command(terminal, &script, self.sys.env.home.as_deref())
            .envs(vars.iter().map(|(k, v)| (k.clone(), v.clone())))
            .log_file(&log);
        self.launched.lock().push(terminal_id.clone());
        let child = self.sys.runner.spawn(&spec).map_err(|e| {
            InterceptError::Failed(format!("couldn't start {}: {e}", terminal.label()))
        })?;
        let pid = child.pid();
        if child.wait_timeout(LAUNCH_SETTLE).await {
            let code = child.exit_info().and_then(|e| e.code);
            if code != Some(0) {
                let _ = std::fs::remove_file(&script);
                let tail = log_tail(&log, 400);
                return Err(InterceptError::Failed(format!(
                    "{} exited immediately (code {}){}{}",
                    terminal.label(),
                    code.map(|c| c.to_string())
                        .unwrap_or_else(|| "unknown".into()),
                    if tail.is_empty() { "" } else { ": " },
                    tail
                )));
            }
            // Exit 0: handed over to a terminal server / `open`; tracked via the marker.
        } else {
            self.tracker.add(child);
        }
        self.sys.invalidate_processes().await;
        Ok(ActivationResult {
            success: true,
            message: Some(format!("Opened an intercepted {} window", terminal.label())),
            metadata: json!({ "pid": pid, "terminal": terminal.id(), "terminalId": terminal_id }),
        })
    }

    async fn deactivate(&self, ctx: &InterceptContext, _options: serde_json::Value) -> Result<()> {
        self.deactivate_all(ctx).await
    }

    async fn deactivate_all(&self, _ctx: &InterceptContext) -> Result<()> {
        self.sys.invalidate_processes().await;
        let shells = self.marked_shells().await;
        // Shells ignore SIGTERM; SIGHUP is what closing the window sends (the shell passes it
        // on to its jobs; `nohup`/`setsid` ones survive, as they would a closed window). Not
        // escalated: a shell that ignores it is the user's business.
        if !shells.is_empty() {
            signal_foreign(&self.sys, &shells, ProcessSignal::Hangup).await;
        }
        stop_all(
            &self.sys,
            &self.tracker,
            Vec::new(),
            ProcessSignal::Terminate,
        )
        .await;
        self.launched.lock().clear();
        Ok(())
    }
}

/// URL of the setup script for `shell`. It carries no token: the one-liners send it in a
/// header (see [`setup_command`]).
pub(crate) fn setup_url(ctx: &InterceptContext, shell: Shell) -> String {
    format!(
        "{}/api/setup/terminal?shell={}",
        ctx.api_url.trim_end_matches('/'),
        shell.as_str()
    )
}

/// A curl config line (`-K`) that sends the API token as a Bearer header. A quoted config
/// value treats `\` as an escape character, so `\` and `"` are escaped (API tokens can't
/// contain either; this keeps arbitrary input from breaking the line).
fn curl_auth_config(token: &str) -> String {
    let mut value = String::with_capacity(token.len() + 32);
    for c in format!("Authorization: Bearer {token}").chars() {
        match c {
            '\\' | '"' => {
                value.push('\\');
                value.push(c);
            }
            '\r' | '\n' => {}
            c => value.push(c),
        }
    }
    format!("header = \"{value}\"")
}

/// The one-liner that configures an open terminal of `shell`.
///
/// The token never appears on a process's command line, which other local users can read
/// (`/proc/<pid>/cmdline`, `ps`): curl reads it as a header from a config on stdin (`-K -`),
/// written by `printf`, a builtin in bash, zsh and fish (no process of its own). PowerShell
/// downloads in-process. cmd writes the config to a file in the user's `%TEMP%` (`echo` is
/// a builtin; a pipe would run it in a child `cmd.exe`) and deletes it afterwards.
/// `--noproxy` / `Proxy=$null`: if the terminal is already proxied, the request must not go
/// through the proxy (which would record the token).
pub(crate) fn setup_command(ctx: &InterceptContext, shell: Shell) -> String {
    let url = setup_url(ctx, shell);
    let config = curl_auth_config(&ctx.api_token);
    match shell {
        Shell::Bash | Shell::Zsh => format!(
            "eval \"$(printf '%s\\n' {} | curl -sS --noproxy '*' -K - {})\"",
            posix_quote(&config),
            posix_quote(&url)
        ),
        Shell::Fish => format!(
            "printf '%s\\n' {} | curl -sS --noproxy '*' -K - {} | source",
            fish_quote(&config),
            fish_quote(&url)
        ),
        Shell::PowerShell => format!(
            "iex (& {{ $c = New-Object Net.WebClient -Property @{{Proxy=$null}}; $c.Headers['Authorization'] = {}; $c.DownloadString({}) }})",
            ps_quote(&format!("Bearer {}", ctx.api_token)),
            ps_quote(&url)
        ),
        Shell::Cmd => {
            let cfg = "\"%TEMP%\\akev-curl.cfg\"";
            let script = "\"%TEMP%\\akev-setup.cmd\"";
            format!(
                ">{cfg} echo {config}&& curl -sS --noproxy \"*\" -K {cfg} -o {script} \"{url}\" && (del {cfg} & call {script}) || del {cfg} 2>nul"
            )
        }
    }
}

/// The command that undoes [`setup_command`] (a function in bash/zsh/fish/PowerShell, a
/// doskey macro in cmd).
pub(crate) fn reset_command(_shell: Shell) -> &'static str {
    "akev-off"
}

/// `existing-terminal`.
pub(crate) struct ExistingTerminalInterceptor {
    sys: Arc<Sys>,
}

impl ExistingTerminalInterceptor {
    const DESCRIPTOR: Descriptor = Descriptor {
        id: "existing-terminal",
        name: "Existing Terminal",
        description: "Intercept everything launched from a terminal that's already open, by pasting one command (bash, zsh, fish, PowerShell or cmd)",
        category: InterceptorCategory::Terminal,
        tags: tags::TERMINAL,
        icon: "terminal",
        needs_config: true,
    };

    pub(crate) fn new(sys: Arc<Sys>) -> Self {
        ExistingTerminalInterceptor { sys }
    }

    fn shells(&self) -> Vec<Shell> {
        if self.sys.env.os == Os::Windows {
            vec![Shell::PowerShell, Shell::Cmd, Shell::Bash]
        } else {
            vec![Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell]
        }
    }

    fn metadata(&self, ctx: &InterceptContext) -> serde_json::Value {
        let default_shell = if self.sys.env.os == Os::Windows {
            Shell::PowerShell
        } else {
            match Shell::parse(&self.sys.env.user_shell()) {
                Shell::Cmd => Shell::Bash,
                s => s,
            }
        };
        let labels = |s: Shell| match s {
            Shell::Bash => "Bash",
            Shell::Zsh => "Zsh",
            Shell::Fish => "Fish",
            Shell::PowerShell => "PowerShell",
            Shell::Cmd => "Command Prompt",
        };
        let mut shells = serde_json::Map::new();
        for s in self.shells() {
            shells.insert(
                s.as_str().to_string(),
                json!({
                    "label": if s == Shell::Bash && self.sys.env.os == Os::Windows { "Git Bash" } else { labels(s) },
                    "command": setup_command(ctx, s),
                    "reset": reset_command(s),
                }),
            );
        }
        json!({
            "defaultShell": default_shell.as_str(),
            "shells": shells,
            "proxy": local_proxy_url(ctx),
            "note": "Run the command in the terminal you want to intercept. Programs started from it afterwards use the proxy; run `akev-off` (or the reset command) to stop.",
        })
    }
}

#[async_trait]
impl Interceptor for ExistingTerminalInterceptor {
    fn id(&self) -> &str {
        Self::DESCRIPTOR.id
    }

    async fn info(&self, ctx: &InterceptContext) -> InterceptorInfo {
        prepare_java_truststore(ctx, &self.sys);
        InterceptorInfo {
            available: true,
            metadata: self.metadata(ctx),
            ..Self::DESCRIPTOR.info()
        }
    }

    async fn activate(
        &self,
        ctx: &InterceptContext,
        _options: serde_json::Value,
    ) -> Result<ActivationResult> {
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            ensure_java_truststore(ctx, &self.sys),
        )
        .await;
        Ok(ActivationResult {
            success: true,
            message: Some(
                "Paste the command for your shell into the terminal you want to intercept".into(),
            ),
            metadata: self.metadata(ctx),
        })
    }

    async fn deactivate(&self, _ctx: &InterceptContext, _options: serde_json::Value) -> Result<()> {
        // Nothing to undo from here: each terminal resets itself with `akev-off`.
        Ok(())
    }

    async fn deactivate_all(&self, _ctx: &InterceptContext) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::HostEnv;
    use crate::host::testutil::{fake_env, make_exe};
    use crate::runner::ProcessEntry;
    use crate::runner::mock::{MockRunner, line_of};
    use crate::util::testctx::ctx;

    #[test]
    fn linux_terminal_preference() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("usr/bin");
        for t in ["xterm", "kitty", "ptyxis", "konsole", "xdg-terminal-exec"] {
            make_exe(&bin.join(t), "");
        }
        let env = fake_env(Os::Linux, dir.path());
        let ids: Vec<String> = detect_terminals(&env).iter().map(Terminal::id).collect();
        assert_eq!(
            ids,
            vec!["xdg-terminal-exec", "ptyxis", "konsole", "kitty", "xterm"]
        );

        let gnome = env.clone().with_var("XDG_CURRENT_DESKTOP", "GNOME");
        assert_eq!(detect_terminals(&gnome)[0].id(), "ptyxis");
        let kde = env.clone().with_var("XDG_CURRENT_DESKTOP", "KDE");
        assert_eq!(detect_terminals(&kde)[0].id(), "konsole");
        let custom = gnome.clone().with_var("TERMINAL", "kitty");
        assert_eq!(detect_terminals(&custom)[0].id(), "kitty");
        // An unknown $TERMINAL is still honoured (with -e).
        make_exe(&bin.join("myterm"), "");
        let unknown = env.clone().with_var("TERMINAL", "myterm");
        let t = &detect_terminals(&unknown)[0];
        assert_eq!(t.id(), "myterm");
        assert_eq!(
            line_of(&terminal_command(t, Path::new("/s.sh"), None)),
            "myterm -e /bin/sh /s.sh"
        );

        assert!(
            detect_terminals(&fake_env(Os::Linux, tempfile::tempdir().unwrap().path())).is_empty()
        );
    }

    #[test]
    fn terminal_command_lines() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("usr/bin");
        for t in LINUX_TERMINALS {
            make_exe(&bin.join(t.bin), "");
        }
        let env = fake_env(Os::Linux, dir.path());
        let terms = detect_terminals(&env);
        let script = Path::new("/data/terminal/akev-1.sh");
        let line = |id: &str| {
            line_of(&terminal_command(
                terms.iter().find(|t| t.id() == id).unwrap(),
                script,
                None,
            ))
        };
        assert_eq!(
            line("ptyxis"),
            "ptyxis -s --new-window -T Akev -- /bin/sh /data/terminal/akev-1.sh"
        );
        assert_eq!(
            line("gnome-terminal"),
            "gnome-terminal --wait -- /bin/sh /data/terminal/akev-1.sh"
        );
        assert_eq!(
            line("konsole"),
            "konsole --nofork -e /bin/sh /data/terminal/akev-1.sh"
        );
        assert_eq!(
            line("xfce4-terminal"),
            "xfce4-terminal --disable-server -x /bin/sh /data/terminal/akev-1.sh"
        );
        assert_eq!(line("kitty"), "kitty /bin/sh /data/terminal/akev-1.sh");
        assert_eq!(
            line("alacritty"),
            "alacritty -e /bin/sh /data/terminal/akev-1.sh"
        );
        assert_eq!(
            line("wezterm"),
            "wezterm start --always-new-process -- /bin/sh /data/terminal/akev-1.sh"
        );
        assert_eq!(line("tilix"), "tilix -e /bin/sh '/data/terminal/akev-1.sh'");
        assert_eq!(
            line("terminator"),
            "terminator -u -x /bin/sh /data/terminal/akev-1.sh"
        );
        assert_eq!(
            line("xterm"),
            "xterm -T Akev -e /bin/sh /data/terminal/akev-1.sh"
        );
    }

    #[test]
    fn macos_and_windows_terminals() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::MacOs, dir.path());
        std::fs::create_dir_all(env.sys_path("/System/Applications/Utilities/Terminal.app"))
            .unwrap();
        let terms = detect_terminals(&env);
        assert_eq!(
            terms.iter().map(Terminal::id).collect::<Vec<_>>(),
            vec!["terminal"]
        );
        std::fs::create_dir_all(env.home_path("Applications/iTerm.app").unwrap()).unwrap();
        let terms = detect_terminals(&env);
        assert_eq!(terms[0].id(), "iterm");
        let cmd = terminal_command(&terms[0], Path::new("/d/akev-1.command"), None);
        assert_eq!(line_of(&cmd), "open -a iTerm /d/akev-1.command");

        let win = tempfile::tempdir().unwrap();
        let sysroot = win.path().join("Windows");
        make_exe(
            &sysroot.join("System32/WindowsPowerShell/v1.0/powershell.exe"),
            "",
        );
        make_exe(&sysroot.join("System32/cmd.exe"), "");
        let lad = win.path().join("Local");
        make_exe(&lad.join("Microsoft/WindowsApps/wt.exe"), "");
        let env = HostEnv::empty(Os::Windows)
            .with_var("SystemRoot", sysroot.to_string_lossy())
            .with_var("LOCALAPPDATA", lad.to_string_lossy());
        let terms = detect_terminals(&env);
        assert_eq!(
            terms.iter().map(Terminal::id).collect::<Vec<_>>(),
            vec!["windows-terminal", "powershell", "cmd"]
        );
        let wt = terminal_command(&terms[0], Path::new("C:/d/akev-1.ps1"), None);
        let args = wt.args_lossy();
        assert_eq!(&args[..5], &["-w", "new", "new-tab", "--title", "Akev"]);
        assert!(args.ends_with(&[
            "-NoLogo".into(),
            "-NoExit".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            "C:/d/akev-1.ps1".into()
        ]));
        let ps = terminal_command(&terms[1], Path::new("x.ps1"), None);
        assert!(ps.new_console);
        let cmd = terminal_command(&terms[2], Path::new("x.cmd"), None);
        assert_eq!(cmd.args_lossy(), vec!["/k", "call", "x.cmd"]);
        assert!(cmd.new_console);
    }

    fn fresh_setup() -> (
        tempfile::TempDir,
        Arc<MockRunner>,
        FreshTerminalInterceptor,
        InterceptContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        make_exe(&dir.path().join("usr/bin/ptyxis"), "");
        let env = fake_env(Os::Linux, dir.path()).with_var("SHELL", "/bin/zsh");
        let runner = MockRunner::new();
        let i = FreshTerminalInterceptor::new(Sys::new(runner.clone(), env));
        let c = ctx(&dir.path().join("data"));
        (dir, runner, i, c)
    }

    #[tokio::test]
    async fn fresh_terminal_launch_and_marker_tracking() {
        let (dir, runner, i, c) = fresh_setup();
        let info = i.info(&c).await;
        assert!(info.available && !info.active);
        assert_eq!(info.metadata["terminal"], json!("ptyxis"));

        let res = i.activate(&c, json!({})).await.unwrap();
        let tid = res.metadata["terminalId"].as_str().unwrap().to_string();
        let spec = runner.spawned_specs().remove(0);
        let args = spec.args_lossy();
        assert_eq!(spec.program_name(), "ptyxis");
        let script = PathBuf::from(args.last().unwrap());
        assert!(script.starts_with(c.data_dir.join("terminal")));
        let content = std::fs::read_to_string(&script).unwrap();
        assert!(content.contains("export HTTP_PROXY='http://127.0.0.1:8000'"));
        assert!(content.contains(&format!("export AKEV_TERMINAL_ID='{tid}'")));
        assert!(content.contains("exec '/bin/zsh' -i"));
        // The env is also passed to the emulator process itself.
        assert_eq!(
            spec.env_value("HTTP_PROXY").unwrap(),
            "http://127.0.0.1:8000"
        );
        assert_eq!(
            spec.current_dir.as_deref(),
            Some(dir.path().join("home").as_path())
        );
        assert!(i.info(&c).await.active);

        // The emulator handed off & exited, but a marked shell is still running → active.
        runner.exit_all();
        assert!(content.contains("export AKEV_TERMINAL_PID=\"$$\""));
        runner.processes.lock().extend([
            ProcessEntry {
                pid: 501,
                name: "zsh".into(),
                akev_env: vec![
                    (TERMINAL_ID_VAR.into(), tid.clone()),
                    (TERMINAL_PID_VAR.into(), "501".into()),
                ],
                ..Default::default()
            },
            // A non-shell child with the marker (e.g. an editor started from it) is left alone.
            ProcessEntry {
                pid: 502,
                name: "code".into(),
                akev_env: vec![
                    (TERMINAL_ID_VAR.into(), tid.clone()),
                    (TERMINAL_PID_VAR.into(), "502".into()),
                ],
                ..Default::default()
            },
            // A shell from someone else's terminal.
            ProcessEntry {
                pid: 503,
                name: "bash".into(),
                akev_env: vec![(TERMINAL_ID_VAR.into(), "other".into())],
                ..Default::default()
            },
        ]);
        i.sys.invalidate_processes().await;
        assert!(i.info(&c).await.active);

        i.deactivate_all(&c).await.unwrap();
        assert_eq!(
            runner.signals.lock().clone(),
            vec![(501, ProcessSignal::Hangup)]
        );
        assert!(!i.info(&c).await.active);
    }

    #[tokio::test]
    async fn only_the_terminal_shell_is_closed_and_never_killed() {
        let (_dir, runner, i, c) = fresh_setup();
        let res = i.activate(&c, json!({})).await.unwrap();
        let tid = res.metadata["terminalId"].as_str().unwrap().to_string();
        runner.exit_all();
        let marked = |pid: u32, name: &str, shell_pid: u32| ProcessEntry {
            pid,
            name: name.into(),
            akev_env: vec![
                (TERMINAL_ID_VAR.into(), tid.clone()),
                (TERMINAL_PID_VAR.into(), shell_pid.to_string()),
            ],
            ..Default::default()
        };
        runner.processes.lock().extend([
            // The terminal's shell.
            marked(601, "zsh", 601),
            // Started from it: `nohup bash ./migrate.sh &`, a tmux pane, a `sh -c` subshell.
            marked(602, "bash", 601),
            marked(603, "zsh", 601),
            marked(604, "sh", 601),
        ]);
        // The user made the shell ignore SIGHUP.
        runner.stubborn_pids.lock().push(601);
        i.sys.invalidate_processes().await;
        assert!(i.info(&c).await.active);

        i.deactivate_all(&c).await.unwrap();
        // Well past the grace period: still nothing else.
        tokio::time::sleep(crate::util::GRACEFUL_STOP * 2).await;
        assert_eq!(
            runner.signals.lock().clone(),
            vec![(601, ProcessSignal::Hangup)]
        );
        let alive: Vec<u32> = runner.processes.lock().iter().map(|p| p.pid).collect();
        assert_eq!(alive, vec![601, 602, 603, 604]);
    }

    #[tokio::test]
    async fn powershell_launcher_records_the_shell_pid() {
        let win = tempfile::tempdir().unwrap();
        let ps = win.path().join("pwsh.exe");
        make_exe(&ps, "");
        let vars = vec![(TERMINAL_ID_VAR.to_string(), "abc".to_string())];
        let (ext, script) = launcher_script(
            &Terminal::PowerShell { path: ps },
            &vars,
            "http://127.0.0.1:8000",
            "pwsh",
            win.path(),
        );
        assert_eq!(ext, "ps1");
        assert!(
            script.contains("$env:AKEV_TERMINAL_PID = \"$PID\"\n"),
            "{script}"
        );
    }

    #[test]
    fn cleanup_keeps_persistent_files() {
        let dir = tempfile::tempdir().unwrap();
        let reset = crate::scripts::ensure_cmd_reset_file(dir.path()).unwrap();
        let terminal = reset.parent().unwrap().to_path_buf();
        let stale_launcher = terminal.join("akev-0123456789abcdef0123456789abcdef.ps1");
        let fresh_launcher = terminal.join("akev-fedcba9876543210fedcba9876543210.sh");
        std::fs::write(&stale_launcher, "x").unwrap();
        std::fs::write(&fresh_launcher, "x").unwrap();
        let two_hours_ago = std::time::SystemTime::now() - Duration::from_secs(7200);
        for p in [&reset, &stale_launcher] {
            std::fs::File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(two_hours_ago)
                .unwrap();
        }
        FreshTerminalInterceptor::cleanup_old_scripts(&terminal);
        assert!(
            reset.exists(),
            "akev-off.cmd is still needed by set-up cmd windows"
        );
        assert!(!stale_launcher.exists());
        assert!(fresh_launcher.exists());
        assert!(!is_launcher_script("akev-off.cmd"));
        assert!(!is_launcher_script("akev-0123.sh"));
        assert!(is_launcher_script(
            "akev-0123456789abcdef0123456789abcdef.command"
        ));
    }

    #[tokio::test]
    async fn fresh_terminal_errors() {
        let (_dir, runner, i, c) = fresh_setup();
        assert!(matches!(
            i.activate(&c, json!({"terminal": "kitty"})).await,
            Err(InterceptError::Unavailable(_))
        ));
        *runner.spawn_exit_code.lock() = Some(1);
        let err = i.activate(&c, json!({})).await.unwrap_err();
        assert!(err.to_string().contains("exited immediately"));
        // The launcher script is cleaned up on failure.
        let leftovers = std::fs::read_dir(c.data_dir.join("terminal"))
            .unwrap()
            .count();
        assert_eq!(leftovers, 0);

        let empty = tempfile::tempdir().unwrap();
        let i = FreshTerminalInterceptor::new(Sys::new(
            MockRunner::new(),
            fake_env(Os::Linux, empty.path()),
        ));
        let info = i.info(&c).await;
        assert!(!info.available);
        assert!(info.unavailable_reason.is_some());
    }

    #[tokio::test]
    async fn existing_terminal_commands() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ctx(dir.path());
        c.api_token = "t o'k&=".into();
        let env = fake_env(Os::Linux, dir.path()).with_var("SHELL", "/usr/bin/fish");
        let i = ExistingTerminalInterceptor::new(Sys::new(MockRunner::new(), env));
        let info = i.info(&c).await;
        assert!(info.available && info.needs_config && !info.active);
        assert_eq!(info.metadata["defaultShell"], json!("fish"));
        // The token travels in a header from curl's config on stdin, never in the URL (or
        // any other process argument).
        let bash = info.metadata["shells"]["bash"]["command"].as_str().unwrap();
        assert_eq!(
            bash,
            r#"eval "$(printf '%s\n' 'header = "Authorization: Bearer t o'\''k&="' | curl -sS --noproxy '*' -K - 'http://127.0.0.1:45456/api/setup/terminal?shell=bash')""#
        );
        assert_eq!(
            info.metadata["shells"]["fish"]["command"],
            json!(
                r#"printf '%s\n' 'header = "Authorization: Bearer t o\'k&="' | curl -sS --noproxy '*' -K - 'http://127.0.0.1:45456/api/setup/terminal?shell=fish' | source"#
            )
        );
        assert_eq!(
            info.metadata["shells"]["powershell"]["command"],
            json!(
                "iex (& { $c = New-Object Net.WebClient -Property @{Proxy=$null}; $c.Headers['Authorization'] = 'Bearer t o''k&='; $c.DownloadString('http://127.0.0.1:45456/api/setup/terminal?shell=powershell') })"
            )
        );
        for shell in info.metadata["shells"].as_object().unwrap().values() {
            assert!(!shell["command"].as_str().unwrap().contains("token="));
        }
        assert_eq!(info.metadata["shells"]["zsh"]["reset"], json!("akev-off"));
        assert!(info.metadata["shells"].get("cmd").is_none());

        let res = i.activate(&c, json!({})).await.unwrap();
        assert!(res.success);
        assert_eq!(res.metadata["shells"]["bash"]["command"], json!(bash));

        let win = ExistingTerminalInterceptor::new(Sys::new(
            MockRunner::new(),
            HostEnv::empty(Os::Windows),
        ));
        let info = win.info(&c).await;
        assert_eq!(info.metadata["defaultShell"], json!("powershell"));
        let cmd = info.metadata["shells"]["cmd"]["command"].as_str().unwrap();
        assert_eq!(
            cmd,
            r#">"%TEMP%\akev-curl.cfg" echo header = "Authorization: Bearer t o'k&="&& curl -sS --noproxy "*" -K "%TEMP%\akev-curl.cfg" -o "%TEMP%\akev-setup.cmd" "http://127.0.0.1:45456/api/setup/terminal?shell=cmd" && (del "%TEMP%\akev-curl.cfg" & call "%TEMP%\akev-setup.cmd") || del "%TEMP%\akev-curl.cfg" 2>nul"#
        );
        assert_eq!(info.metadata["shells"]["cmd"]["reset"], json!("akev-off"));
        assert_eq!(info.metadata["shells"]["bash"]["label"], json!("Git Bash"));
    }

    #[test]
    fn curl_config_line_is_escaped() {
        assert_eq!(
            curl_auth_config("abc123"),
            r#"header = "Authorization: Bearer abc123""#
        );
        assert_eq!(
            curl_auth_config("a\"b\\c\nd"),
            r#"header = "Authorization: Bearer a\"b\\cd""#
        );
    }

    /// `tool --version` runs and names itself. On Windows `bash` is usually System32's WSL
    /// launcher, which exists (and fails) even when no Linux distribution is installed.
    fn have_tool(tool: &str) -> bool {
        std::process::Command::new(tool)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains(tool))
    }

    /// Runs the bash one-liner against a stand-in API that requires the Bearer header: curl
    /// must parse the config from stdin and send it, and the setup script must be eval'd.
    #[test]
    fn bash_one_liner_sends_the_token_as_a_header() {
        if !have_tool("bash") || !have_tool("curl") {
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Write};
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                head.push(line.trim_end().to_string());
            }
            let authorized = head.iter().any(|l| l == "Authorization: Bearer t'ok&=");
            let body = if authorized {
                "export AKEV_TEST_SETUP=ok\n"
            } else {
                "echo unauthorized\n"
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            head
        });
        let dir = tempfile::tempdir().unwrap();
        let mut c = ctx(dir.path());
        c.api_url = format!("http://127.0.0.1:{port}");
        c.api_token = "t'ok&=".into();
        let line = setup_command(&c, Shell::Bash);
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(format!("{line}; echo \"result=$AKEV_TEST_SETUP\""))
            .env_remove("AKEV_TEST_SETUP")
            .output()
            .unwrap();
        let head = server.join().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("result=ok"), "{stdout} / {head:?}");
        assert_eq!(head[0], "GET /api/setup/terminal?shell=bash HTTP/1.1");
    }

    #[test]
    fn bash_one_liner_actually_parses() {
        if !have_tool("bash") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let line = setup_command(&c, Shell::Bash);
        let out = std::process::Command::new("bash")
            .arg("-n")
            .arg("-c")
            .arg(&line)
            .output()
            .unwrap();
        assert!(out.status.success());
    }
}
