//! `akev` — HTTP(S) interception, inspection, mocking and debugging.
//!
//! `akev` (or `akev start`) runs the daemon and opens the web UI; the other commands talk
//! to a running daemon discovered through `<data dir>/daemon.json`.

/// `eprintln!` that ignores write errors instead of panicking. After a hang-up the terminal is
/// gone and writes to it fail (EIO); `eprintln!` would panic and cut the graceful shutdown
/// (restoring the system proxy, closing launched browsers...) short.
macro_rules! eprintln_safe {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

mod ca;
mod capture;
mod client;
mod license;
mod mcp;
mod remote;
mod start;
mod style;
mod tail;
mod update;

use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

const ABOUT: &str = "Akev — a fast, light HTTP(S) interceptor, debugger and mocking proxy";

const LONG_ABOUT: &str = "Akev — a fast, light HTTP(S) interceptor, debugger and mocking proxy.

Run `akev` to start the proxy and open the web UI. Point any client at the proxy (or use the
UI's Connect page to launch pre-configured browsers and terminals), then inspect, filter,
replay, edit and mock traffic. AI agents can drive Akev through MCP (`akev mcp`).";

const AFTER_HELP: &str = "Examples:
  akev                                  Start Akev and open the web UI
  akev start --proxy-port 9000 --no-open
  akev capture -o traffic.har --duration 60 --filter 'host:api.example.com'
  akev export --har out.har --filter 'status:>=400'
  akev tail --filter 'status:>=500'      Follow failing requests live
  akev intercept chrome                 Launch a proxied Chrome window
  akev license set AKEV1.…              Enter your Akev Pro license key
  claude mcp add akev -- akev mcp      Connect Claude Code to Akev";

fn styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
        .usage(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
        .literal(AnsiColor::Green.on_default().effects(Effects::BOLD))
        .placeholder(AnsiColor::Yellow.on_default())
        .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
        .valid(AnsiColor::Green.on_default())
        .invalid(AnsiColor::Red.on_default())
}

/// `akev --version`: the version, and whether this build has the BoringSSL impersonation
/// backend (official builds do; the release workflow checks this line).
#[cfg(feature = "impersonate")]
const VERSION_LINE: &str = concat!(env!("CARGO_PKG_VERSION"), " (with TLS impersonation)");
#[cfg(not(feature = "impersonate"))]
const VERSION_LINE: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(
    name = "akev",
    version = VERSION_LINE,
    about = ABOUT,
    long_about = LONG_ABOUT,
    after_help = AFTER_HELP,
    styles = styles(),
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Options for the default `start` command.
    #[command(flatten)]
    start: StartArgs,

    #[command(flatten)]
    global: GlobalArgs,
}

#[derive(Args, Debug, Clone)]
struct GlobalArgs {
    /// Data directory (CA, settings, rules, daemon discovery file).
    #[arg(long, global = true, env = "AKEV_DATA_DIR", value_name = "DIR")]
    data_dir: Option<PathBuf>,

    /// API token: sets the daemon's token for `start`; authenticates other commands.
    #[arg(
        long,
        global = true,
        env = "AKEV_TOKEN",
        hide_env_values = true,
        value_name = "TOKEN"
    )]
    token: Option<String>,

    /// Akev Pro license key for this run (not stored; use `akev license set` to store one).
    #[arg(
        long,
        global = true,
        env = "AKEV_LICENSE",
        hide_env_values = true,
        value_name = "KEY"
    )]
    license: Option<String>,

    /// URL of a running daemon's API (default: discovered from the data directory).
    #[arg(long, global = true, env = "AKEV_URL", value_name = "URL")]
    url: Option<String>,

    /// More logging (-v info, -vv debug, -vvv trace). RUST_LOG overrides.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Args, Debug, Clone, Default, PartialEq)]
struct StartArgs {
    /// Proxy port (default: from settings, 8000; the next free port is used if busy).
    #[arg(long, value_name = "PORT")]
    proxy_port: Option<u16>,

    /// Proxy bind address (default: from settings, 127.0.0.1). Use 0.0.0.0 to accept devices
    /// on your network (phones, containers).
    #[arg(long, value_name = "ADDR")]
    proxy_bind: Option<String>,

    /// API / web UI port (default 45456, a random port if busy; 0 = random).
    #[arg(long, value_name = "PORT")]
    api_port: Option<u16>,

    /// API bind address (default 127.0.0.1). Other addresses expose the UI and API to the
    /// network: keep the generated token (a custom --token must be at least 24 characters).
    #[arg(long, value_name = "ADDR")]
    api_bind: Option<String>,

    /// Don't open the web UI in a browser.
    #[arg(long)]
    no_open: bool,

    /// Extra allowed Origin for the API (e.g. a UI dev server). Repeatable.
    #[arg(long = "allow-origin", value_name = "ORIGIN")]
    allow_origin: Vec<String>,

    /// Load rules from this file for this run (Akev or HTTP Toolkit rules export).
    #[arg(long, value_name = "FILE")]
    rules: Option<PathBuf>,

    /// Don't load or save settings and rules; use a temporary data directory unless --data-dir is given.
    #[arg(long)]
    ephemeral: bool,

    /// Let AI agents (MCP) activate interceptors that start arbitrary programs (`electron`).
    /// Off by default: captured traffic could otherwise talk an agent into launching programs.
    #[arg(long)]
    mcp_allow_launch: bool,

    /// Let AI agents (MCP) see secrets in captured traffic when they ask (`reveal`). Off by
    /// default: credential headers, cookies, secret query parameters and tokens in bodies
    /// are shown to agents as [redacted].
    #[arg(long)]
    mcp_reveal_secrets: bool,

    /// A folder AI agents (MCP) may make Akev read and write files in, besides the data
    /// directory's `exports` folder: HAR exports and the files their rules serve. Repeatable.
    #[arg(long = "mcp-allow-path", value_name = "DIR")]
    mcp_allow_path: Vec<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start the proxy and web UI (the default command).
    Start(StartArgs),
    /// Headless capture for CI: proxy traffic, print one line per exchange, write a HAR on exit.
    Capture(capture::CaptureArgs),
    /// Show whether Akev is running, and where.
    Status {
        /// Print the raw status JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stop the running daemon.
    Stop,
    /// CA certificate: path, details, export, regenerate.
    Ca {
        #[command(subcommand)]
        command: ca::CaCommand,
    },
    /// Launch / configure clients: `akev intercept list`, `akev intercept <id>`, `akev intercept stop <id>`.
    Intercept(remote::InterceptArgs),
    /// Export captured traffic from the running daemon.
    Export(remote::ExportArgs),
    /// Follow live traffic of the running daemon (one line per completed exchange).
    Tail(tail::TailArgs),
    /// Get or replace the rule set.
    Rules {
        #[command(subcommand)]
        command: remote::RulesCommand,
    },
    /// MCP server over stdio for AI agents (bridges to the running daemon, or starts one).
    Mcp(mcp::McpArgs),
    /// Akev Pro: show the license, enter a key, start the trial.
    License {
        #[command(subcommand)]
        command: license::LicenseCommand,
    },
    /// Check whether a newer version of Akev is available.
    Update {
        /// Print the raw result JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print version information.
    Version {
        #[arg(long)]
        json: bool,
    },
}

/// Log to stderr; the daemon (`daemon_dir`: its data directory) also logs to
/// `<data dir>/logs/akev.log` and remembers recent events for diagnostics and crash reports.
fn init_tracing(verbose: u8, daemon_dir: Option<&std::path::Path>) {
    use akev_api::observability;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer};
    let level = match verbose {
        0 => "warn".to_string(),
        1 => "info".to_string(),
        2 => "debug,hyper=info,h2=info,rustls=info".to_string(),
        _ => "trace".to_string(),
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level.clone()));
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(verbose > 0)
        .compact()
        // A failed write would otherwise be reported with `eprintln!`, which panics once the
        // terminal is gone (after SIGHUP) — in the middle of the graceful shutdown.
        .log_internal_errors(false)
        .with_filter(filter);
    // `-v` makes the log file as detailed as the terminal.
    let file_level = (verbose > 0).then_some(level.as_str());
    let (file, recent) = match daemon_dir {
        Some(dir) => {
            let file = match observability::open_log(dir) {
                Ok(writer) => Some(
                    observability::file_layer(writer)
                        .with_filter(observability::file_filter(file_level)),
                ),
                Err(e) => {
                    eprintln_safe!(
                        "warning: can't write the log file in {}: {e}",
                        dir.display()
                    );
                    None
                }
            };
            let recent =
                observability::RecentEvents.with_filter(observability::file_filter(file_level));
            (file, Some(recent))
        }
        None => (None, None),
    };
    let _ = tracing_subscriber::registry()
        .with(stderr)
        .with(file)
        .with(recent)
        .try_init();
}

/// The data directory of the daemon this invocation runs (`akev` / `akev start` without
/// `--ephemeral`), for its log file and crash reports.
fn daemon_data_dir(cli: &Cli) -> Option<PathBuf> {
    let start = match &cli.command {
        None => &cli.start,
        Some(Command::Start(args)) => args,
        Some(_) => return None,
    };
    if start.ephemeral {
        return None;
    }
    data_dir(&cli.global).ok()
}

/// Resolve the data directory (flag/env, else the platform default).
pub(crate) fn data_dir(global: &GlobalArgs) -> anyhow::Result<PathBuf> {
    match &global.data_dir {
        Some(d) => Ok(d.clone()),
        None => akev_api::default_data_dir().ok_or_else(|| {
            anyhow::anyhow!("could not determine the data directory; pass --data-dir")
        }),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let daemon_dir = daemon_data_dir(&cli);
    init_tracing(cli.global.verbose, daemon_dir.as_deref());
    if let Some(dir) = &daemon_dir {
        akev_api::observability::install_panic_hook(dir.join(akev_api::observability::CRASH_DIR));
    }
    // Before any handler is installed: remember signals we were told to ignore (`nohup`).
    #[cfg(unix)]
    let _ = inherited_ignored_signals();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("akev")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(run(cli));
    // Don't wait for lingering blocking tasks (e.g. a stdin read) on exit.
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    match result {
        Ok(code) => code,
        Err(e) => {
            let p = style::Paint::stderr();
            eprintln!("{} {e:#}", p.red(&p.bold("error:")));
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    check_start_args(&cli)?;
    let global = cli.global;
    match cli.command {
        None => start::run(cli.start, &global).await,
        Some(Command::Start(args)) => start::run(args, &global).await,
        Some(Command::Capture(args)) => capture::run(args, &global).await,
        Some(Command::Status { json }) => remote::status(&global, json).await,
        Some(Command::Stop) => remote::stop(&global).await,
        Some(Command::Ca { command }) => ca::run(command, &global).await,
        Some(Command::Intercept(args)) => remote::intercept(args, &global).await,
        Some(Command::Export(args)) => remote::export(args, &global).await,
        Some(Command::Tail(args)) => tail::run(args, &global).await,
        Some(Command::Rules { command }) => remote::rules(command, &global).await,
        Some(Command::Mcp(args)) => mcp::run(args, &global).await,
        Some(Command::License { command }) => license::run(command, &global).await,
        Some(Command::Update { json }) => update::run(json, &global).await,
        Some(Command::Version { json }) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "name": "akev",
                        "version": akev_core::VERSION,
                        "releaseDate": akev_core::RELEASE_DATE,
                        "tlsImpersonation": cfg!(feature = "impersonate"),
                    })
                );
            } else {
                println!("akev {VERSION_LINE}");
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Start options at the top level only make sense without a subcommand.
fn check_start_args(cli: &Cli) -> anyhow::Result<()> {
    if cli.command.is_some() && cli.start != StartArgs::default() {
        anyhow::bail!(
            "start options (--proxy-port, --no-open, ...) must be used with `akev` or `akev start`, not with another command"
        );
    }
    Ok(())
}

/// Why the process is asked to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopSignal {
    /// Ctrl+C (SIGINT) or Ctrl+Break.
    Interrupt,
    /// SIGTERM.
    #[cfg_attr(not(unix), allow(dead_code))]
    Terminate,
    /// SIGQUIT (Ctrl+\).
    #[cfg_attr(not(unix), allow(dead_code))]
    Quit,
    /// SIGHUP: the terminal window was closed, or the ssh session dropped.
    #[cfg_attr(not(unix), allow(dead_code))]
    Hangup,
    /// Windows: the console window was closed, the user logged off or the system shuts down.
    #[cfg_attr(not(windows), allow(dead_code))]
    ConsoleClosed,
}

impl StopSignal {
    /// Whether this signal arriving *during* the graceful shutdown forces an immediate exit.
    /// Only signals a user sends on purpose do: a hang-up or console event must not cut the
    /// cleanup (restoring the system proxy, closing launched clients) short.
    pub(crate) fn forces_exit(self) -> bool {
        matches!(
            self,
            StopSignal::Interrupt | StopSignal::Terminate | StopSignal::Quit
        )
    }
}

/// Whether SIGHUP / SIGQUIT were ignored when the process started (e.g. SIGHUP under
/// `nohup`, SIGQUIT for background jobs of non-interactive shells). Those stay ignored rather
/// than turning into a shutdown request.
#[cfg(unix)]
fn inherited_ignored_signals() -> (bool, bool) {
    static IGNORED: std::sync::OnceLock<(bool, bool)> = std::sync::OnceLock::new();
    *IGNORED.get_or_init(|| (signal_ignored(libc::SIGHUP), signal_ignored(libc::SIGQUIT)))
}

#[cfg(unix)]
fn signal_ignored(signal: libc::c_int) -> bool {
    // SAFETY: `sigaction` with a null new action only reads the current disposition of
    // `signal` into `old`, a properly sized, zero-initialised `sigaction` struct.
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        libc::sigaction(signal, std::ptr::null(), &mut old) == 0
            && old.sa_sigaction == libc::SIG_IGN
    }
}

#[cfg(unix)]
async fn unix_signal(kind: tokio::signal::unix::SignalKind) {
    match tokio::signal::unix::signal(kind) {
        Ok(mut s) => {
            s.recv().await;
        }
        Err(_) => std::future::pending::<()>().await,
    }
}

/// Waits for a Windows console event (`ctrl_break`, `ctrl_close`, ...).
#[cfg(windows)]
macro_rules! console_event {
    ($listener:path) => {
        async {
            match $listener() {
                Ok(mut listener) => {
                    listener.recv().await;
                }
                Err(_) => std::future::pending::<()>().await,
            }
        }
    };
}

/// Resolves when the process should stop: Ctrl+C, SIGTERM, SIGQUIT or SIGHUP (terminal
/// closed / ssh dropped) on Unix; Ctrl+C, Ctrl+Break, console close, logoff or shutdown on
/// Windows. Without handling the hang-up and console-close cases the process would just die
/// and leave the system proxy pointing at a dead port.
pub(crate) async fn shutdown_signal() -> StopSignal {
    let interrupt = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    {
        use tokio::signal::unix::SignalKind;
        let (hup_ignored, quit_ignored) = inherited_ignored_signals();
        let hangup = async {
            if hup_ignored {
                std::future::pending::<()>().await;
            }
            unix_signal(SignalKind::hangup()).await;
        };
        let quit = async {
            if quit_ignored {
                std::future::pending::<()>().await;
            }
            unix_signal(SignalKind::quit()).await;
        };
        tokio::select! {
            _ = interrupt => StopSignal::Interrupt,
            _ = unix_signal(SignalKind::terminate()) => StopSignal::Terminate,
            _ = hangup => StopSignal::Hangup,
            _ = quit => StopSignal::Quit,
        }
    }
    #[cfg(windows)]
    {
        tokio::select! {
            _ = interrupt => StopSignal::Interrupt,
            _ = console_event!(tokio::signal::windows::ctrl_break) => StopSignal::Interrupt,
            _ = console_event!(tokio::signal::windows::ctrl_close) => StopSignal::ConsoleClosed,
            _ = console_event!(tokio::signal::windows::ctrl_logoff) => StopSignal::ConsoleClosed,
            _ = console_event!(tokio::signal::windows::ctrl_shutdown) => StopSignal::ConsoleClosed,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        interrupt.await;
        StopSignal::Interrupt
    }
}

/// Graceful shutdown; a second Ctrl+C (or SIGTERM / SIGQUIT) forces exit. Hang-ups and
/// console events during the shutdown are ignored.
pub(crate) async fn shutdown_gracefully(handle: &akev_api::AkevHandle) {
    let shutdown = handle.shutdown();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => return,
            signal = shutdown_signal() => {
                if signal.forces_exit() {
                    eprintln_safe!("Forced exit.");
                    std::process::exit(130);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_commands() {
        let cli = Cli::try_parse_from(["akev", "--no-open", "--proxy-port", "9000"]).unwrap();
        assert!(cli.command.is_none());
        assert!(cli.start.no_open);
        assert_eq!(cli.start.proxy_port, Some(9000));

        let cli = Cli::try_parse_from([
            "akev",
            "start",
            "--api-port",
            "0",
            "--allow-origin",
            "http://localhost:5173",
            "--allow-origin",
            "x",
            "--ephemeral",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Start(a)) => {
                assert_eq!(a.api_port, Some(0));
                assert_eq!(a.allow_origin.len(), 2);
                assert!(a.ephemeral);
            }
            other => panic!("unexpected {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "akev",
            "capture",
            "-o",
            "out.har",
            "--duration",
            "5",
            "--filter",
            "status:500",
        ])
        .unwrap();
        assert!(matches!(cli.command, Some(Command::Capture(_))));

        let cli = Cli::try_parse_from(["akev", "ca", "export", "ca.crt", "--der"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Ca {
                command: ca::CaCommand::Export { der: true, .. }
            })
        ));

        let cli = Cli::try_parse_from([
            "akev",
            "intercept",
            "chrome",
            "--options",
            "{\"url\":\"https://x\"}",
        ])
        .unwrap();
        assert!(matches!(cli.command, Some(Command::Intercept(_))));
        let cli = Cli::try_parse_from(["akev", "intercept", "stop", "chrome"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Intercept(_))));
        let cli = Cli::try_parse_from(["akev", "tail", "-f", "status:500", "--json"]).unwrap();
        match cli.command {
            Some(Command::Tail(t)) => {
                assert_eq!(t.filter.as_deref(), Some("status:500"));
                assert!(t.json && !t.history);
            }
            other => panic!("unexpected {other:?}"),
        }

        let cli =
            Cli::try_parse_from(["akev", "--data-dir", "/tmp/x", "status", "--json"]).unwrap();
        assert_eq!(cli.global.data_dir, Some(PathBuf::from("/tmp/x")));
        let cli = Cli::try_parse_from(["akev", "status", "--data-dir", "/tmp/y", "-vv"]).unwrap();
        assert_eq!(cli.global.data_dir, Some(PathBuf::from("/tmp/y")));
        assert_eq!(cli.global.verbose, 2);

        let cli =
            Cli::try_parse_from(["akev", "mcp", "--url", "http://127.0.0.1:1", "--token", "t"])
                .unwrap();
        assert!(matches!(cli.command, Some(Command::Mcp(ref a)) if !a.allow_launch));
        assert_eq!(cli.global.token.as_deref(), Some("t"));
        let cli = Cli::try_parse_from(["akev", "mcp", "--allow-launch"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Mcp(ref a)) if a.allow_launch));
        let cli = Cli::try_parse_from(["akev", "start", "--mcp-allow-launch"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Start(ref a)) if a.mcp_allow_launch));
        let cli = Cli::try_parse_from(["akev"]).unwrap();
        assert!(!cli.start.mcp_allow_launch);

        // start options can't be combined with another subcommand
        let cli = Cli::try_parse_from(["akev", "--no-open", "status"]).unwrap();
        assert!(check_start_args(&cli).is_err());
        let cli = Cli::try_parse_from(["akev", "--no-open"]).unwrap();
        assert!(check_start_args(&cli).is_ok());
    }

    #[test]
    fn only_the_daemon_logs_to_the_data_dir() {
        let dir = |args: &[&str]| daemon_data_dir(&Cli::try_parse_from(args).unwrap());
        assert_eq!(
            dir(&["akev", "--data-dir", "/tmp/d"]),
            Some(PathBuf::from("/tmp/d"))
        );
        assert_eq!(
            dir(&["akev", "start", "--data-dir", "/tmp/d"]),
            Some(PathBuf::from("/tmp/d"))
        );
        assert_eq!(dir(&["akev", "--ephemeral", "--data-dir", "/tmp/d"]), None);
        assert_eq!(dir(&["akev", "start", "--ephemeral"]), None);
        assert_eq!(dir(&["akev", "status", "--data-dir", "/tmp/d"]), None);
        assert_eq!(dir(&["akev", "mcp", "--data-dir", "/tmp/d"]), None);
        assert_eq!(dir(&["akev", "capture", "-o", "x.har"]), None);
    }

    #[test]
    fn only_deliberate_signals_force_exit_during_shutdown() {
        assert!(StopSignal::Interrupt.forces_exit());
        assert!(StopSignal::Terminate.forces_exit());
        assert!(StopSignal::Quit.forces_exit());
        assert!(!StopSignal::Hangup.forces_exit());
        assert!(!StopSignal::ConsoleClosed.forces_exit());
    }

    #[cfg(unix)]
    #[test]
    fn ignored_signal_dispositions_are_detected() {
        // The Rust runtime ignores SIGPIPE; nothing ignores SIGUSR2.
        assert!(signal_ignored(libc::SIGPIPE));
        assert!(!signal_ignored(libc::SIGUSR2));
    }
}
