//! Hard wall-clock limits for script execution (feature `scripting`).
//!
//! QuickJS enforces the script time limit with its interrupt handler, which only runs between
//! bytecode instructions. A regular expression that backtracks catastrophically, or a long
//! native loop such as `Array(2 ** 32 - 1).indexOf(1)`, never reaches it, and a running thread
//! can't be cancelled. So every script call runs on its own thread, supervised from the
//! calling thread:
//!
//! * The caller waits until the deadline plus a short grace period ([`GRACE`]), which gives the
//!   interrupt handler time to stop the script and report a proper error. After that the caller
//!   gets [`Failure::Abandoned`], so a request never hangs on a script.
//! * A script thread still running after that is abandoned. It ends on its own once the native
//!   call returns, because the interrupt handler then fires at the next instruction. While
//!   [`Supervisor::max_abandoned`] abandoned threads are still running, new calls are refused
//!   at once ([`Failure::Busy`]), so stuck scripts can't pile up threads and burn every core.
//! * At most [`Supervisor::max_running`] script threads run at once. Further callers wait (up
//!   to their time limit) for a slot.
//! * The thread gets a large, explicit stack ([`THREAD_STACK_SIZE`]), far above the stack
//!   limit QuickJS is configured with. Deep recursion inside the engine therefore raises a
//!   catchable `RangeError` instead of overflowing the native stack, which would abort the
//!   whole process.
//! * A panic on the script thread is reported as [`Failure::Crashed`] instead of propagating.

use parking_lot::{Condvar, Mutex};
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

/// Native stack size of a script thread. QuickJS's own limit (`JS_STACK_LIMIT` in the script
/// module) is far below this, so engine recursion is always caught before the real stack ends.
pub(crate) const THREAD_STACK_SIZE: usize = 16 << 20;

/// How long the caller keeps waiting after the deadline for the interrupt handler to stop the
/// script.
pub(crate) const GRACE: Duration = Duration::from_millis(500);

/// Why a supervised call produced no result.
#[derive(Debug)]
pub(crate) enum Failure {
    /// The call wasn't started: too many scripts are running or stuck.
    Busy(String),
    /// The call didn't finish by its deadline plus [`GRACE`]. Its thread was abandoned and
    /// keeps running until the current native call returns.
    Abandoned,
    /// The script thread couldn't be started, or it panicked.
    Crashed(String),
}

#[derive(Debug, Default)]
struct Counts {
    /// Live script threads, abandoned ones included.
    running: usize,
    /// Abandoned script threads that are still running.
    abandoned: usize,
}

/// State shared by the caller and the script thread of one call. Both flags change only while
/// [`Supervisor::counts`] is locked, which keeps them consistent with the counters.
#[derive(Default)]
struct Job {
    finished: AtomicBool,
    abandoned: AtomicBool,
}

/// Limits and bookkeeping for script threads. Production code uses [`global`]; tests can build
/// their own instance with small limits.
pub(crate) struct Supervisor {
    counts: Mutex<Counts>,
    slot_freed: Condvar,
    max_running: usize,
    max_abandoned: usize,
}

static GLOBAL: LazyLock<Supervisor> = LazyLock::new(|| {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    // Unit tests deliberately leave runaway threads behind while other tests run in parallel.
    let max_abandoned = if cfg!(test) {
        64
    } else {
        (cores / 2).clamp(2, 8)
    };
    Supervisor::new((cores * 2).clamp(8, 64), max_abandoned)
});

/// The process-wide supervisor used for every script call.
pub(crate) fn global() -> &'static Supervisor {
    &GLOBAL
}

/// Releases a running slot when the script thread ends (also when the job panics).
struct Release<'a> {
    supervisor: &'a Supervisor,
    job: Arc<Job>,
}

impl Drop for Release<'_> {
    fn drop(&mut self) {
        let mut counts = self.supervisor.counts.lock();
        counts.running = counts.running.saturating_sub(1);
        if self.job.abandoned.load(Ordering::Relaxed) {
            counts.abandoned = counts.abandoned.saturating_sub(1);
        }
        self.job.finished.store(true, Ordering::Relaxed);
        drop(counts);
        self.supervisor.slot_freed.notify_all();
    }
}

impl Supervisor {
    pub(crate) const fn new(max_running: usize, max_abandoned: usize) -> Self {
        Supervisor {
            counts: Mutex::new(Counts {
                running: 0,
                abandoned: 0,
            }),
            slot_freed: Condvar::new(),
            max_running,
            max_abandoned,
        }
    }

    /// Abandoned script threads that are still running.
    #[cfg(test)]
    fn abandoned(&self) -> usize {
        self.counts.lock().abandoned
    }

    /// Run `job` on a dedicated thread. The job receives its deadline (`limit` from the moment
    /// it starts) and should stop on its own by then. The caller returns at the latest
    /// [`GRACE`] after the deadline. Waiting for a free slot adds at most `limit`.
    pub(crate) fn run<T, F>(&'static self, limit: Duration, job: F) -> Result<T, Failure>
    where
        T: Send + 'static,
        F: FnOnce(Instant) -> T + Send + 'static,
    {
        self.acquire(Instant::now() + limit)?;
        let state = Arc::new(Job::default());
        let thread_state = Arc::clone(&state);
        let deadline = Instant::now() + limit;
        let (tx, rx) = sync_channel(1);
        let spawned = std::thread::Builder::new()
            .name("akev-script".to_string())
            .stack_size(THREAD_STACK_SIZE)
            .spawn(move || {
                let _release = Release {
                    supervisor: self,
                    job: thread_state,
                };
                let outcome = catch_unwind(AssertUnwindSafe(|| job(deadline)));
                // The caller may have given up already; then nobody receives this.
                let _ = tx.send(outcome);
            });
        if let Err(e) = spawned {
            // The closure never ran, so its release guard didn't either.
            let mut counts = self.counts.lock();
            counts.running = counts.running.saturating_sub(1);
            drop(counts);
            self.slot_freed.notify_all();
            return Err(Failure::Crashed(format!(
                "could not start a script thread: {e}"
            )));
        }
        let wait = (deadline + GRACE).saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(outcome) => outcome.map_err(crashed),
            Err(RecvTimeoutError::Disconnected) => Err(Failure::Crashed(
                "the script thread ended without a result".to_string(),
            )),
            Err(RecvTimeoutError::Timeout) => {
                let abandoned = {
                    let mut counts = self.counts.lock();
                    if !state.finished.load(Ordering::Relaxed) {
                        state.abandoned.store(true, Ordering::Relaxed);
                        counts.abandoned += 1;
                    }
                    counts.abandoned
                };
                // The thread sends its result before it counts as finished, so a result that
                // arrived after the timeout fired is in the channel now.
                match rx.try_recv() {
                    Ok(outcome) => outcome.map_err(crashed),
                    Err(_) => {
                        tracing::warn!(
                            limit_ms = u64::try_from(limit.as_millis()).unwrap_or(u64::MAX),
                            abandoned,
                            "script stuck in a call that can't be interrupted; thread abandoned"
                        );
                        Err(Failure::Abandoned)
                    }
                }
            }
        }
    }

    /// Reserve a running slot, waiting until `wait_until` for one to free up.
    fn acquire(&self, wait_until: Instant) -> Result<(), Failure> {
        let mut counts = self.counts.lock();
        loop {
            if counts.abandoned >= self.max_abandoned {
                return Err(Failure::Busy(format!(
                    "scripts are suspended: {} earlier script call(s) exceeded the time limit \
                     inside a regular expression or built-in function that can't be interrupted, \
                     and are still running",
                    counts.abandoned
                )));
            }
            if counts.running < self.max_running {
                counts.running += 1;
                return Ok(());
            }
            if Instant::now() >= wait_until {
                return Err(Failure::Busy(format!(
                    "too many scripts are running at once ({})",
                    counts.running
                )));
            }
            self.slot_freed.wait_until(&mut counts, wait_until);
        }
    }
}

fn crashed(panic: Box<dyn Any + Send>) -> Failure {
    let message = panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string());
    Failure::Crashed(format!("the script runner panicked: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn leak(s: Supervisor) -> &'static Supervisor {
        Box::leak(Box::new(s))
    }

    /// A job that spins (ignoring its deadline, like a stuck native call) until released.
    fn stuck(release: &Arc<AtomicBool>) -> impl FnOnce(Instant) -> u32 + Send + 'static {
        let release = Arc::clone(release);
        move |_| {
            while !release.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            1
        }
    }

    fn wait_for(cond: impl Fn() -> bool) {
        let start = Instant::now();
        while !cond() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "condition timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn results_deadlines_and_panics() {
        let sup = leak(Supervisor::new(4, 2));
        let limit = Duration::from_millis(300);
        let out = sup.run(limit, move |deadline| {
            assert!(deadline > Instant::now());
            assert!(deadline <= Instant::now() + limit);
            7
        });
        assert_eq!(out.unwrap(), 7);
        match sup.run(limit, |_| -> u32 { panic!("boom") }) {
            Err(Failure::Crashed(msg)) => assert!(msg.contains("boom"), "{msg}"),
            other => panic!("unexpected {other:?}"),
        }
        // The thread has the configured stack: native recursion using several MiB (which
        // would overflow a default 2 MiB thread) works.
        fn depth(n: u64) -> u64 {
            let pad = std::hint::black_box([0u8; 1024]);
            if n == 0 {
                u64::from(pad[0])
            } else {
                depth(n - 1) + u64::from(pad[1])
            }
        }
        assert_eq!(sup.run(limit, |_| depth(4 * 1024)).unwrap(), 0);
        wait_for(|| sup.counts.lock().running == 0);
    }

    #[test]
    fn stuck_jobs_are_abandoned_and_bounded() {
        let sup = leak(Supervisor::new(8, 2));
        let release = Arc::new(AtomicBool::new(false));
        let limit = Duration::from_millis(100);
        for expected in 1..=2 {
            let start = Instant::now();
            let r = sup.run(limit, stuck(&release));
            assert!(matches!(r, Err(Failure::Abandoned)), "{r:?}");
            let waited = start.elapsed();
            assert!(waited >= limit + GRACE, "{waited:?}");
            assert!(
                waited < limit + GRACE + Duration::from_secs(1),
                "{waited:?}"
            );
            assert_eq!(sup.abandoned(), expected);
        }
        // Two abandoned threads are still running: new calls are refused at once.
        let start = Instant::now();
        match sup.run(limit, |_| 1u32) {
            Err(Failure::Busy(msg)) => assert!(msg.contains("suspended"), "{msg}"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(start.elapsed() < Duration::from_millis(50));
        // Once the stuck calls return, scripts run again.
        release.store(true, Ordering::Relaxed);
        wait_for(|| sup.abandoned() == 0 && sup.counts.lock().running == 0);
        assert_eq!(sup.run(limit, |_| 5u32).unwrap(), 5);
    }

    #[test]
    fn concurrency_is_capped() {
        let sup = leak(Supervisor::new(2, 4));
        let release = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicUsize::new(0));
        let mut callers = Vec::new();
        for _ in 0..2 {
            let release = Arc::clone(&release);
            let started = Arc::clone(&started);
            callers.push(std::thread::spawn(move || {
                sup.run(Duration::from_secs(5), move |_| {
                    started.fetch_add(1, Ordering::Relaxed);
                    while !release.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    2u32
                })
                .unwrap()
            }));
        }
        wait_for(|| started.load(Ordering::Relaxed) == 2);
        // Both slots are taken: a third call waits for its own limit, then gives up.
        let start = Instant::now();
        match sup.run(Duration::from_millis(100), |_| 3u32) {
            Err(Failure::Busy(msg)) => assert!(msg.contains("at once"), "{msg}"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(start.elapsed() >= Duration::from_millis(100));
        // A waiting call gets the slot as soon as one frees up.
        let waiter = std::thread::spawn(move || sup.run(Duration::from_secs(5), |_| 4u32));
        std::thread::sleep(Duration::from_millis(50));
        release.store(true, Ordering::Relaxed);
        assert_eq!(waiter.join().unwrap().unwrap(), 4);
        for c in callers {
            assert_eq!(c.join().unwrap(), 2);
        }
        wait_for(|| sup.counts.lock().running == 0);
    }
}
