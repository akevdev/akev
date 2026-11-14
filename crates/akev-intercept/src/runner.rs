//! Process execution abstraction.
//!
//! Every external command an interceptor runs (probing `adb devices`, launching a browser,
//! flipping `gsettings` keys...) goes through a [`CommandRunner`]. The production
//! implementation is [`SystemRunner`] (non-blocking `tokio::process` with hard timeouts);
//! tests substitute a scripted mock so nothing touches the real machine.

use async_trait::async_trait;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// Default timeout for short-lived helper commands.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// A command to run or spawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    /// Program to execute (absolute path or a name resolved through `PATH`).
    pub program: OsString,
    /// Arguments, passed verbatim (no shell involved).
    pub args: Vec<OsString>,
    /// Environment variables added to (or overriding) the inherited environment.
    pub env: Vec<(OsString, OsString)>,
    /// Environment variables removed from the inherited environment.
    pub env_remove: Vec<OsString>,
    /// Working directory.
    pub current_dir: Option<PathBuf>,
    /// Bytes written to stdin (only for [`CommandRunner::output`]).
    pub stdin: Option<Vec<u8>>,
    /// Hard timeout for [`CommandRunner::output`]; the process is killed when it expires.
    pub timeout: Duration,
    /// Windows only: give a spawned process its own console window (terminal launches).
    pub new_console: bool,
    /// For [`CommandRunner::spawn`]: file receiving the process' stdout & stderr
    /// (truncated on spawn). `None` discards the output.
    pub log_file: Option<PathBuf>,
}

impl CommandSpec {
    /// A new command with no arguments and the [`DEFAULT_TIMEOUT`].
    pub fn new(program: impl Into<OsString>) -> Self {
        CommandSpec {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            env_remove: Vec::new(),
            current_dir: None,
            stdin: None,
            timeout: DEFAULT_TIMEOUT,
            new_console: false,
            log_file: None,
        }
    }

    /// Append one argument.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append several arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set one environment variable.
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Set several environment variables.
    pub fn envs<I, K, V>(mut self, vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        self.env
            .extend(vars.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// Remove an inherited environment variable.
    pub fn env_remove(mut self, key: impl Into<OsString>) -> Self {
        self.env_remove.push(key.into());
        self
    }

    /// Set the working directory.
    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(dir.into());
        self
    }

    /// Feed `input` to the process' stdin.
    pub fn stdin(mut self, input: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(input.into());
        self
    }

    /// Set the timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Windows: open a new console window for the spawned process.
    pub fn new_console(mut self, value: bool) -> Self {
        self.new_console = value;
        self
    }

    /// Redirect a spawned process' output to `path`.
    pub fn log_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.log_file = Some(path.into());
        self
    }

    /// The program's file name (`/usr/bin/adb` → `adb`), for logs and matching.
    pub fn program_name(&self) -> String {
        std::path::Path::new(&self.program)
            .file_name()
            .unwrap_or(self.program.as_os_str())
            .to_string_lossy()
            .into_owned()
    }

    /// Arguments as lossy UTF-8 strings.
    pub fn args_lossy(&self) -> Vec<String> {
        self.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// Value of an environment variable set on this spec (last one wins).
    pub fn env_value(&self, key: &str) -> Option<&OsStr> {
        self.env
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_os_str())
    }
}

impl fmt::Display for CommandSpec {
    /// `program arg1 arg2` (lossy, unquoted — for logs).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.program.to_string_lossy())?;
        for arg in &self.args {
            write!(f, " {}", arg.to_string_lossy())?;
        }
        Ok(())
    }
}

/// Captured result of a finished command.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandOutput {
    /// Exit code (`None` if killed by a signal).
    pub code: Option<i32>,
    /// Stdout (lossy UTF-8).
    pub stdout: String,
    /// Stderr (lossy UTF-8).
    pub stderr: String,
}

impl CommandOutput {
    /// Successful output with the given stdout.
    pub fn ok(stdout: impl Into<String>) -> Self {
        CommandOutput {
            code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    /// Failed output (exit code 1) with the given stderr.
    pub fn failed(stderr: impl Into<String>) -> Self {
        CommandOutput {
            code: Some(1),
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    /// Exit code 0.
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// First non-empty line of stderr (or stdout), for error messages.
    pub fn error_summary(&self) -> String {
        self.stderr
            .lines()
            .chain(self.stdout.lines())
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("no output")
            .to_string()
    }
}

/// Signals understood by [`CommandRunner::signal`] and [`ChildHandle::signal`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProcessSignal {
    /// Polite request to exit (SIGTERM; Windows: `taskkill` without `/F`, i.e. WM_CLOSE).
    Terminate,
    /// Hang-up (SIGHUP) — what closing a terminal window sends to its shell. Windows: as
    /// `Terminate`.
    Hangup,
    /// Forceful kill of the process (and, for spawned children, its whole process group /
    /// tree).
    Kill,
}

/// Exit information of a finished child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitInfo {
    /// Exit code (`None` when killed by a signal or unknown).
    pub code: Option<i32>,
}

type SignalFn = dyn Fn(ProcessSignal) + Send + Sync;

/// Handle to a long-running process started with [`CommandRunner::spawn`].
///
/// Cloneable; the process keeps running when all handles are dropped (launched browsers
/// and terminals must survive independently).
#[derive(Clone)]
pub struct ChildHandle {
    pid: Option<u32>,
    exit: watch::Receiver<Option<ExitInfo>>,
    control: Arc<SignalFn>,
}

impl fmt::Debug for ChildHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChildHandle")
            .field("pid", &self.pid)
            .field("exit", &*self.exit.borrow())
            .finish()
    }
}

impl ChildHandle {
    /// Build a handle from its parts: `exit` publishes `Some(..)` once the process ends and
    /// `control` delivers signals. Used by runner implementations (including test mocks).
    pub fn from_parts(
        pid: Option<u32>,
        exit: watch::Receiver<Option<ExitInfo>>,
        control: impl Fn(ProcessSignal) + Send + Sync + 'static,
    ) -> Self {
        ChildHandle {
            pid,
            exit,
            control: Arc::new(control),
        }
    }

    /// OS process id.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Still running?
    pub fn is_running(&self) -> bool {
        self.exit.borrow().is_none()
    }

    /// Exit info once finished.
    pub fn exit_info(&self) -> Option<ExitInfo> {
        *self.exit.borrow()
    }

    /// Send a signal (ignored once the process has exited).
    pub fn signal(&self, signal: ProcessSignal) {
        if self.is_running() {
            (self.control)(signal);
        }
    }

    /// Wait up to `timeout` for the process to exit; returns `true` if it has.
    pub async fn wait_timeout(&self, timeout: Duration) -> bool {
        let mut rx = self.exit.clone();
        let wait = async move {
            // `wait_for` returns immediately if the condition already holds. An error means
            // the sender is gone, which only happens after the exit was published (or the
            // reaper task died) — either way there's nothing more to wait for.
            let _ = rx.wait_for(Option::is_some).await;
        };
        tokio::time::timeout(timeout, wait).await.is_ok() && !self.is_running()
    }
}

/// A process seen in a process-table snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessEntry {
    /// Process id.
    pub pid: u32,
    /// Parent process id.
    pub parent: Option<u32>,
    /// Process name (e.g. `chrome`, `bash`).
    pub name: String,
    /// Command line (empty when not readable).
    pub cmd: Vec<String>,
    /// Only `AKEV_*` environment variables (when requested and readable).
    pub akev_env: Vec<(String, String)>,
}

impl ProcessEntry {
    /// Value of an `AKEV_*` environment variable of this process.
    pub fn akev_var(&self, key: &str) -> Option<&str> {
        self.akev_env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Runs external commands, spawns long-lived processes and inspects/signals processes.
#[async_trait]
pub trait CommandRunner: Send + Sync + 'static {
    /// Run a command to completion and capture its output. Must never block the async
    /// runtime and must kill the process when `spec.timeout` expires (returning an error of
    /// kind [`io::ErrorKind::TimedOut`]).
    async fn output(&self, spec: &CommandSpec) -> io::Result<CommandOutput>;

    /// Start a long-running process (browser, terminal, app) detached from Akev's own
    /// process group. Requires a Tokio runtime.
    fn spawn(&self, spec: &CommandSpec) -> io::Result<ChildHandle>;

    /// Snapshot of the process table. With `with_environ`, `AKEV_*` environment variables
    /// are read too (more expensive).
    async fn processes(&self, with_environ: bool) -> io::Result<Vec<ProcessEntry>>;

    /// Signal an arbitrary process (not necessarily one we spawned).
    async fn signal(&self, pid: u32, signal: ProcessSignal) -> io::Result<()>;
}

/// The real [`CommandRunner`]: `tokio::process` + `sysinfo`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRunner;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

fn timed_out(spec: &CommandSpec) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!(
            "`{}` timed out after {:.1}s",
            spec.program_name(),
            spec.timeout.as_secs_f64()
        ),
    )
}

/// Windows: stop the programs we start from inheriting our own stdin, stdout and stderr by
/// accident.
///
/// `CreateProcess` gives a child *every* inheritable handle of ours, not just the ones passed
/// as its stdio, and our standard handles are inheritable (that's how we received them). A
/// long-lived program we start (the adb server that `adb devices` launches, a browser, a
/// terminal) would then hold the pipe our output goes to open after we exit, and whatever
/// reads it to the end (`akev … | tee`, a CI step) waits until that program exits too.
/// Clearing the flag only stops this accidental inheritance: a child meant to share our stdio
/// (`Stdio::inherit`) gets its own inheritable duplicate from the standard library.
#[cfg(windows)]
fn keep_std_handles_private() {
    use std::sync::Once;
    use windows_sys::Win32::Foundation::{
        HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: `GetStdHandle` has no preconditions. `SetHandleInformation` only changes
            // the inherit flag of one of our own handles; a process without a console (the
            // desktop app) has null standard handles, which are skipped.
            unsafe {
                let handle = GetStdHandle(which);
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                    SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    });
}

fn base_command(spec: &CommandSpec) -> tokio::process::Command {
    #[cfg(windows)]
    keep_std_handles_private();
    let mut cmd = tokio::process::Command::new(&spec.program);
    cmd.args(&spec.args);
    for key in &spec.env_remove {
        cmd.env_remove(key);
    }
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    if let Some(dir) = &spec.current_dir {
        cmd.current_dir(dir);
    }
    cmd
}

/// Refuse to signal pids that would hit more than one process or ourselves.
fn signalable(pid: u32) -> io::Result<()> {
    if pid <= 1 || pid == std::process::id() || pid > i32::MAX as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to signal pid {pid}"),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn unix_kill(pid: i32, signal: ProcessSignal) -> io::Result<()> {
    let sig = match signal {
        ProcessSignal::Terminate => libc::SIGTERM,
        ProcessSignal::Hangup => libc::SIGHUP,
        ProcessSignal::Kill => libc::SIGKILL,
    };
    // SAFETY: `kill` has no memory-safety preconditions; callers validate `pid` so it never
    // targets 0 / -1 (every process) or our own process.
    let rc = unsafe { libc::kill(pid, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
async fn taskkill(pid: u32, force: bool) -> io::Result<()> {
    let mut cmd = tokio::process::Command::new("taskkill");
    cmd.arg("/PID").arg(pid.to_string()).arg("/T");
    if force {
        cmd.arg("/F");
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true);
    let status = tokio::time::timeout(Duration::from_secs(10), cmd.status())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "taskkill timed out"))??;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("taskkill exited with {status}")))
    }
}

#[async_trait]
impl CommandRunner for SystemRunner {
    async fn output(&self, spec: &CommandSpec) -> io::Result<CommandOutput> {
        use std::process::Stdio;
        use tokio::io::AsyncWriteExt;

        let mut cmd = base_command(spec);
        cmd.stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut child = cmd.spawn()?;
        if let (Some(input), Some(mut stdin)) = (spec.stdin.clone(), child.stdin.take()) {
            tokio::spawn(async move {
                // A child that exits without reading stdin produces EPIPE; that's its call.
                let _ = stdin.write_all(&input).await;
                let _ = stdin.shutdown().await;
            });
        }
        // Dropping the future on timeout drops the child, and `kill_on_drop` kills it.
        let out = tokio::time::timeout(spec.timeout, child.wait_with_output())
            .await
            .map_err(|_| timed_out(spec))??;
        Ok(CommandOutput {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    fn spawn(&self, spec: &CommandSpec) -> io::Result<ChildHandle> {
        use std::process::Stdio;

        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| io::Error::other("spawning processes requires a Tokio runtime"))?;
        let mut cmd = base_command(spec);
        cmd.stdin(Stdio::null()).kill_on_drop(false);
        match &spec.log_file {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let file = std::fs::File::create(path)?;
                let err = file.try_clone()?;
                cmd.stdout(Stdio::from(file)).stderr(Stdio::from(err));
            }
            None => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        // Own process group: Ctrl+C on Akev's terminal must not take launched apps down,
        // and we can kill a browser together with all its helper processes.
        #[cfg(unix)]
        cmd.process_group(0);
        #[cfg(windows)]
        cmd.creation_flags(
            CREATE_NEW_PROCESS_GROUP
                | if spec.new_console {
                    CREATE_NEW_CONSOLE
                } else {
                    0
                },
        );

        let mut child = {
            let _guard = runtime.enter();
            cmd.spawn()?
        };
        let pid = child.id();
        let (exit_tx, exit_rx) = watch::channel(None);
        let (ctl_tx, mut ctl_rx) = tokio::sync::mpsc::unbounded_channel::<ProcessSignal>();
        let program = spec.program_name();

        runtime.spawn(async move {
            loop {
                tokio::select! {
                    status = child.wait() => {
                        let code = status.ok().and_then(|s| s.code());
                        tracing::debug!(%program, ?pid, ?code, "launched process exited");
                        let _ = exit_tx.send(Some(ExitInfo { code }));
                        break;
                    }
                    Some(signal) = ctl_rx.recv() => {
                        // We own the child, so it can't have been reaped behind our back:
                        // if `try_wait` says it's running, its pid is still ours to signal.
                        if !matches!(child.try_wait(), Ok(None)) {
                            continue;
                        }
                        let Some(pid) = pid else { continue };
                        if let Err(e) = deliver_child_signal(&mut child, pid, signal).await {
                            tracing::debug!(%program, pid, ?signal, "signal failed: {e}");
                        }
                    }
                }
            }
        });

        Ok(ChildHandle::from_parts(pid, exit_rx, move |signal| {
            let _ = ctl_tx.send(signal);
        }))
    }

    async fn processes(&self, with_environ: bool) -> io::Result<Vec<ProcessEntry>> {
        tokio::task::spawn_blocking(move || snapshot_processes(with_environ))
            .await
            .map_err(|e| io::Error::other(format!("process scan failed: {e}")))
    }

    async fn signal(&self, pid: u32, signal: ProcessSignal) -> io::Result<()> {
        signalable(pid)?;
        #[cfg(unix)]
        {
            unix_kill(pid as i32, signal)
        }
        #[cfg(windows)]
        {
            taskkill(pid, signal == ProcessSignal::Kill).await
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = signal;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "signals unsupported on this platform",
            ))
        }
    }
}

async fn deliver_child_signal(
    child: &mut tokio::process::Child,
    pid: u32,
    signal: ProcessSignal,
) -> io::Result<()> {
    signalable(pid)?;
    #[cfg(unix)]
    {
        let _ = child;
        match signal {
            // Only the leader: a browser shuts its helpers down itself when asked nicely.
            ProcessSignal::Terminate | ProcessSignal::Hangup => unix_kill(pid as i32, signal),
            // The whole group (renderers, zygotes, the shell inside a terminal...).
            ProcessSignal::Kill => {
                unix_kill(-(pid as i32), signal).or_else(|_| unix_kill(pid as i32, signal))
            }
        }
    }
    #[cfg(windows)]
    {
        match signal {
            ProcessSignal::Kill => match taskkill(pid, true).await {
                Ok(()) => Ok(()),
                Err(_) => child.start_kill(),
            },
            _ => taskkill(pid, false).await,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = signal;
        child.start_kill()
    }
}

fn snapshot_processes(with_environ: bool) -> Vec<ProcessEntry> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let mut kind = ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always);
    if with_environ {
        kind = kind.with_environ(UpdateKind::Always);
    }
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    sys.processes()
        .values()
        .filter(|p| p.thread_kind().is_none())
        .map(|p| ProcessEntry {
            pid: p.pid().as_u32(),
            parent: p.parent().map(|pp| pp.as_u32()),
            name: p.name().to_string_lossy().into_owned(),
            cmd: p
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
            akev_env: if with_environ {
                p.environ()
                    .iter()
                    .filter_map(|kv| {
                        let kv = kv.to_str()?;
                        let (k, v) = kv.split_once('=')?;
                        k.starts_with("AKEV_")
                            .then(|| (k.to_string(), v.to_string()))
                    })
                    .collect()
            } else {
                Vec::new()
            },
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod mock {
    //! Scripted [`CommandRunner`] for tests: records every call, answers `output` from
    //! registered handlers and fakes spawned processes.

    use super::*;
    use parking_lot::Mutex;

    type Handler = Box<dyn Fn(&CommandSpec) -> Option<io::Result<CommandOutput>> + Send + Sync>;

    /// A fake spawned process.
    pub struct FakeChild {
        pub spec: CommandSpec,
        pub pid: u32,
        pub exit: watch::Sender<Option<ExitInfo>>,
        pub signals: Arc<Mutex<Vec<ProcessSignal>>>,
    }

    #[derive(Default)]
    pub struct MockRunner {
        handlers: Mutex<Vec<Handler>>,
        pub calls: Mutex<Vec<CommandSpec>>,
        pub spawned: Mutex<Vec<FakeChild>>,
        pub processes: Mutex<Vec<ProcessEntry>>,
        pub signals: Mutex<Vec<(u32, ProcessSignal)>>,
        /// Spawned children exit immediately with this code (simulates forwarding/launchers).
        pub spawn_exit_code: Mutex<Option<i32>>,
        /// Spawned children ignore Terminate/Hangup (forces the kill path).
        pub stubborn: Mutex<bool>,
        /// Process-table pids that survive Terminate/Hangup (only `Kill` removes them).
        pub stubborn_pids: Mutex<Vec<u32>>,
        /// `output` calls whose line starts with the prefix take this long (recorded first).
        pub delays: Mutex<Vec<(String, Duration)>>,
        next_pid: Mutex<u32>,
    }

    impl MockRunner {
        pub fn new() -> Arc<Self> {
            Arc::new(MockRunner {
                next_pid: Mutex::new(40_000),
                ..Default::default()
            })
        }

        /// Register a handler; the most recently registered matching handler wins.
        pub fn on(
            &self,
            f: impl Fn(&CommandSpec) -> Option<io::Result<CommandOutput>> + Send + Sync + 'static,
        ) {
            self.handlers.lock().push(Box::new(f));
        }

        /// Answer commands whose `program args...` line starts with `prefix`.
        pub fn on_prefix(&self, prefix: &str, out: CommandOutput) {
            let prefix = prefix.to_string();
            self.on(move |spec| line_of(spec).starts_with(&prefix).then(|| Ok(out.clone())));
        }

        pub fn call_lines(&self) -> Vec<String> {
            self.calls.lock().iter().map(line_of).collect()
        }

        pub fn spawned_specs(&self) -> Vec<CommandSpec> {
            self.spawned.lock().iter().map(|c| c.spec.clone()).collect()
        }

        /// Make every fake child exit.
        pub fn exit_all(&self) {
            for child in self.spawned.lock().iter() {
                let _ = child.exit.send(Some(ExitInfo { code: Some(0) }));
            }
        }
    }

    /// `program-file-name arg1 arg2...`
    pub fn line_of(spec: &CommandSpec) -> String {
        let mut line = spec.program_name();
        for a in &spec.args {
            line.push(' ');
            line.push_str(&a.to_string_lossy());
        }
        line
    }

    #[async_trait]
    impl CommandRunner for MockRunner {
        async fn output(&self, spec: &CommandSpec) -> io::Result<CommandOutput> {
            self.calls.lock().push(spec.clone());
            let line = line_of(spec);
            let result = {
                let handlers = self.handlers.lock();
                handlers
                    .iter()
                    .rev()
                    .find_map(|h| h(spec))
                    .unwrap_or_else(|| {
                        Err(io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("mock: no handler for `{line}`"),
                        ))
                    })
            };
            let delay = self
                .delays
                .lock()
                .iter()
                .find(|(prefix, _)| line.starts_with(prefix.as_str()))
                .map(|(_, d)| *d);
            if let Some(d) = delay {
                tokio::time::sleep(d).await;
            }
            result
        }

        fn spawn(&self, spec: &CommandSpec) -> io::Result<ChildHandle> {
            let pid = {
                let mut next = self.next_pid.lock();
                *next += 1;
                *next
            };
            let initial = self
                .spawn_exit_code
                .lock()
                .map(|code| ExitInfo { code: Some(code) });
            let (tx, rx) = watch::channel(initial);
            let signals = Arc::new(Mutex::new(Vec::new()));
            let stubborn = *self.stubborn.lock();
            let tx2 = tx.clone();
            let sig2 = signals.clone();
            let handle = ChildHandle::from_parts(Some(pid), rx, move |s| {
                sig2.lock().push(s);
                if s == ProcessSignal::Kill || !stubborn {
                    let _ = tx2.send(Some(ExitInfo { code: None }));
                }
            });
            self.spawned.lock().push(FakeChild {
                spec: spec.clone(),
                pid,
                exit: tx,
                signals,
            });
            Ok(handle)
        }

        async fn processes(&self, _with_environ: bool) -> io::Result<Vec<ProcessEntry>> {
            Ok(self.processes.lock().clone())
        }

        async fn signal(&self, pid: u32, signal: ProcessSignal) -> io::Result<()> {
            self.signals.lock().push((pid, signal));
            // Signalled fake processes disappear from the table (stubborn ones only when
            // killed).
            if signal == ProcessSignal::Kill || !self.stubborn_pids.lock().contains(&pid) {
                self.processes.lock().retain(|p| p.pid != pid);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_builder_and_display() {
        let spec = CommandSpec::new("/usr/bin/adb")
            .args(["-s", "emulator-5554"])
            .arg("shell")
            .env("A", "1");
        assert_eq!(spec.program_name(), "adb");
        assert_eq!(spec.to_string(), "/usr/bin/adb -s emulator-5554 shell");
        assert_eq!(spec.env_value("A"), Some(OsStr::new("1")));
        assert_eq!(spec.env_value("B"), None);
    }

    #[test]
    fn output_error_summary() {
        let out = CommandOutput {
            code: Some(1),
            stdout: "x\n".into(),
            stderr: "\n  boom  \nmore".into(),
        };
        assert_eq!(out.error_summary(), "boom");
        assert_eq!(CommandOutput::default().error_summary(), "no output");
    }

    #[test]
    fn refuses_dangerous_pids() {
        assert!(signalable(0).is_err());
        assert!(signalable(1).is_err());
        assert!(signalable(std::process::id()).is_err());
        assert!(signalable(u32::MAX).is_err());
        assert!(signalable(12345).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn system_runner_output_and_timeout() {
        let r = SystemRunner;
        let out = r
            .output(&CommandSpec::new("sh").args(["-c", "echo hi; echo err >&2; exit 3"]))
            .await
            .unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.stderr, "err\n");

        let out = r
            .output(&CommandSpec::new("cat").stdin("piped"))
            .await
            .unwrap();
        assert_eq!(out.stdout, "piped");

        let started = std::time::Instant::now();
        let err = r
            .output(
                &CommandSpec::new("sleep")
                    .arg("5")
                    .timeout(Duration::from_millis(200)),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));

        let err = r
            .output(&CommandSpec::new("/nonexistent/akev-binary"))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn system_runner_spawn_signal_and_reap() {
        let r = SystemRunner;
        let child = r.spawn(&CommandSpec::new("sleep").arg("30")).unwrap();
        assert!(child.is_running());
        assert!(child.pid().is_some());
        child.signal(ProcessSignal::Terminate);
        assert!(child.wait_timeout(Duration::from_secs(5)).await);
        assert!(!child.is_running());

        // A process ignoring SIGTERM still dies to Kill (group kill).
        let child = r
            .spawn(&CommandSpec::new("sh").args(["-c", "trap '' TERM; sleep 30"]))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        child.signal(ProcessSignal::Terminate);
        assert!(!child.wait_timeout(Duration::from_millis(300)).await);
        child.signal(ProcessSignal::Kill);
        assert!(child.wait_timeout(Duration::from_secs(5)).await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn system_runner_spawn_log_file_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("logs/out.log");
        let child = SystemRunner
            .spawn(
                &CommandSpec::new("sh")
                    .args(["-c", "echo launched; exit 7"])
                    .log_file(&log),
            )
            .unwrap();
        assert!(child.wait_timeout(Duration::from_secs(5)).await);
        assert_eq!(child.exit_info(), Some(ExitInfo { code: Some(7) }));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "launched\n");
    }

    #[tokio::test]
    async fn system_runner_lists_own_process() {
        let procs = SystemRunner.processes(false).await.unwrap();
        assert!(procs.iter().any(|p| p.pid == std::process::id()));
    }

    /// The JVM attach CI job hung for hours on Windows: the adb server that `adb devices`
    /// started had inherited the test process's stdout, the pipe CI was reading to its end.
    #[cfg(windows)]
    #[test]
    fn commands_dont_inherit_our_std_handles() {
        use windows_sys::Win32::Foundation::{
            GetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        };
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
        };

        let _ = base_command(&CommandSpec::new("cmd"));
        for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let mut flags = 0;
            // SAFETY: both calls only read information about one of our own handles.
            let known = unsafe {
                let handle = GetStdHandle(which);
                !handle.is_null()
                    && handle != INVALID_HANDLE_VALUE
                    && GetHandleInformation(handle, &mut flags) != 0
            };
            if known {
                assert_eq!(flags & HANDLE_FLAG_INHERIT, 0, "std handle {which}");
            }
        }
    }
}
