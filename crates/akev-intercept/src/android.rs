//! Android devices and emulators over ADB.
//!
//! Activation (all `adb` calls have timeouts, so an unresponsive device never hangs Akev):
//! 1. push the CA (DER) to `/sdcard/Download/akev-ca.crt` so the user can install it; on
//!    rooted devices and emulators (`adb root` or `su`), also trust it as a *system* CA until
//!    the next reboot (see [`SYSTEM_CA_SCRIPT`]), so release apps trust it too;
//! 2. `adb reverse tcp:<port> tcp:<port>` so the device reaches the proxy over USB at
//!    `127.0.0.1:<port>`, re-established every few seconds while active;
//! 3. `settings put global http_proxy 127.0.0.1:<port>`.
//!
//! Configured devices are journaled (`android-proxy.json` in the platform data directory, or a
//! file in the per-user recovery directory for any other data directory, see
//! [`crate::journal`]); if Akev dies, the next start clears their proxy setting as soon as
//! they're connected again. Devices proxied by another *running* Akev instance are left
//! alone. Activation, deactivation and recovery are serialised, so a listing during an
//! activation never mistakes the new journal entry for a stale one.

use crate::host::Os;
use crate::journal::{
    dedup, instance_id, journal_file, own_process_name, same_program, scan_journals,
};
use crate::runner::CommandSpec;
use crate::util::{Descriptor, PROBE_TTL, ProbeCache, Sys, atomic_write, opt_str, tags};
use crate::{InterceptContext, InterceptError, Interceptor, Result};
use akev_core::interceptor::{ActivationResult, InterceptorCategory, InterceptorInfo};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Where the CA is pushed on the device.
pub const DEVICE_CA_PATH: &str = "/sdcard/Download/akev-ca.crt";
const SHORT: Duration = Duration::from_secs(5);
const LONG: Duration = Duration::from_secs(20);
const TUNNEL_REFRESH: Duration = Duration::from_secs(5);

/// One line of `adb devices -l`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdbDevice {
    /// Serial (`emulator-5554`, `R58M...`, `192.168.1.5:5555`).
    pub serial: String,
    /// `device`, `offline`, `unauthorized`, `no permissions`, `recovery`, ...
    pub state: String,
    /// `key:value` properties (`model`, `product`, `device`, `transport_id`, `usb`).
    pub props: BTreeMap<String, String>,
}

impl AdbDevice {
    /// Ready for commands.
    pub fn is_ready(&self) -> bool {
        self.state == "device"
    }

    /// An emulator (by serial convention).
    pub fn is_emulator(&self) -> bool {
        self.serial.starts_with("emulator-")
    }
}

/// Is `token` a `key:value` device property (and not part of a URL or message)?
fn is_prop(token: &str) -> bool {
    match token.split_once(':') {
        Some((k, v)) => {
            !k.is_empty()
                && !v.is_empty()
                && !v.starts_with("//")
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// Parse `adb devices -l` output.
pub fn parse_adb_devices(output: &str) -> Vec<AdbDevice> {
    let mut out = Vec::new();
    for line in output.lines().map(str::trim) {
        if line.is_empty() || line.starts_with("List of devices") || line.starts_with('*') {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 2 {
            continue;
        }
        let serial = tokens[0].to_string();
        // The state is every token up to the first property; "no permissions (...)" spans
        // several words, so keep just its first two.
        let state_end = tokens[1..]
            .iter()
            .position(|t| is_prop(t))
            .map(|p| p + 1)
            .unwrap_or(tokens.len());
        let state_words = &tokens[1..state_end];
        let state =
            if state_words.first() == Some(&"no") && state_words.get(1) == Some(&"permissions") {
                "no permissions".to_string()
            } else {
                state_words
                    .first()
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            };
        let props = tokens[state_end..]
            .iter()
            .filter(|t| is_prop(t))
            .filter_map(|t| {
                t.split_once(':')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect();
        out.push(AdbDevice {
            serial,
            state,
            props,
        });
    }
    out
}

/// Parse `adb shell getprop` output (`[key]: [value]` lines).
pub fn parse_getprop(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (k, v) = line.split_once("]: [")?;
            let k = k.strip_prefix('[')?;
            let v = v.strip_suffix(']')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// Human-friendly device details derived from its properties.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DeviceDetails {
    pub name: String,
    pub model: Option<String>,
    pub manufacturer: Option<String>,
    pub android_version: Option<String>,
    pub sdk: Option<u32>,
}

/// Derive display details from `getprop` output.
pub(crate) fn device_details(dev: &AdbDevice, props: &HashMap<String, String>) -> DeviceDetails {
    let get = |k: &str| {
        props
            .get(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let model =
        get("ro.product.model").or_else(|| dev.props.get("model").map(|m| m.replace('_', " ")));
    let manufacturer = get("ro.product.manufacturer");
    let android_version = get("ro.build.version.release");
    let sdk = get("ro.build.version.sdk").and_then(|s| s.parse().ok());
    let avd = get("ro.boot.qemu.avd_name")
        .or_else(|| get("ro.kernel.qemu.avd_name"))
        .map(|n| n.replace('_', " "));
    let name = if dev.is_emulator() {
        avd.unwrap_or_else(|| match &android_version {
            Some(v) => format!("Android {v} emulator"),
            None => "Android emulator".to_string(),
        })
    } else {
        match (&manufacturer, &model) {
            (Some(m), Some(md)) if !md.to_lowercase().starts_with(&m.to_lowercase()) => {
                format!("{m} {md}")
            }
            (_, Some(md)) => md.clone(),
            _ => dev.serial.clone(),
        }
    };
    DeviceDetails {
        name,
        model,
        manufacturer,
        android_version,
        sdk,
    }
}

/// Locate `adb` (PATH, then the SDK locations Android Studio uses).
pub(crate) fn find_adb(env: &crate::host::HostEnv) -> Option<PathBuf> {
    if let Some(p) = env.find_in_path("adb") {
        return Some(p);
    }
    let exe = if env.os == Os::Windows {
        "adb.exe"
    } else {
        "adb"
    };
    let mut sdks: Vec<PathBuf> = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(|k| env.var_path(k))
        .collect();
    match env.os {
        Os::Linux => sdks.extend(env.home_path("Android/Sdk")),
        Os::MacOs => sdks.extend(env.home_path("Library/Android/sdk")),
        Os::Windows => sdks.extend(
            env.var_path("LOCALAPPDATA")
                .map(|l| l.join("Android").join("Sdk")),
        ),
        Os::Other => {}
    }
    sdks.into_iter()
        .map(|s| s.join("platform-tools").join(exe))
        .find(|p| p.is_file())
}

/// File name of the device journal.
const JOURNAL_FILE: &str = "android-proxy.json";

/// A device whose proxy setting Akev changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalEntry {
    serial: String,
    port: u16,
    /// Owning process id.
    pid: u32,
    /// [`instance_id`] of the owning process.
    instance: String,
    /// Owning process name (guards against pid reuse).
    process_name: String,
}

impl JournalEntry {
    /// Written by this process?
    fn is_ours(&self) -> bool {
        self.instance == instance_id()
    }
}

fn read_journal(path: &Path) -> Vec<JournalEntry> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_journal(path: &Path, entries: &[JournalEntry]) {
    let result = if entries.is_empty() {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        serde_json::to_vec_pretty(entries)
            .map_err(std::io::Error::other)
            .and_then(|bytes| atomic_write(path, &bytes))
    };
    if let Err(e) = result {
        tracing::warn!("couldn't update the Android journal: {e}");
    }
}

/// A snapshot of connected devices.
#[derive(Clone, Debug, Default)]
struct DeviceScan {
    adb: Option<PathBuf>,
    devices: Vec<AdbDevice>,
    error: Option<String>,
}

/// `android-adb`.
pub(crate) struct AndroidAdbInterceptor {
    sys: Arc<Sys>,
    scan: ProbeCache<DeviceScan>,
    details: Mutex<HashMap<String, DeviceDetails>>,
    /// serial → proxy port, for devices configured by this process.
    active: Arc<Mutex<HashMap<String, u16>>>,
    tunnel_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Serialises activation, deactivation and crash recovery (journal read-modify-write
    /// and the device changes that go with it).
    op: tokio::sync::Mutex<()>,
    /// Devices where the CA was installed as a system CA (removed again on deactivation).
    system_trusted: Mutex<HashMap<String, RootShell>>,
}

/// How to run a command as root on a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RootShell {
    /// `adb shell` already runs as root (`adb root`, emulators without Play Store, userdebug).
    Adb,
    /// Through `su` (Magisk and the like; the device may ask the user to allow it).
    Su,
}

/// Where the system-CA files are staged on the device.
const DEVICE_STAGING: &str = "/data/local/tmp/akev-ca";

/// Trusts the Akev CA as a system CA until the device restarts (rooted devices and
/// emulators). The system store is replaced by a tmpfs holding the existing CAs plus Akev's;
/// on Android 14+, whose CAs come from the Conscrypt APEX, the same store is bind-mounted over
/// the APEX directory in zygote's and every app's mount namespace. Nothing on the system
/// partition changes; a reboot restores everything. `@HASH@` is the CA's `subject_hash_old`.
pub const SYSTEM_CA_SCRIPT: &str = r#"set -e
DIR=/data/local/tmp/akev-ca
CERT="$DIR/@HASH@.0"
STORE="$DIR/store"
SYS=/system/etc/security/cacerts
APEX=/apex/com.android.conscrypt/cacerts
rm -rf "$STORE"
mkdir -p "$STORE"
if [ -d "$APEX" ]; then cp "$APEX"/* "$STORE"/; else cp "$SYS"/* "$STORE"/; fi
cp "$CERT" "$STORE"/
mount -t tmpfs tmpfs "$SYS"
cp "$STORE"/* "$SYS"/
chown root:root "$SYS" "$SYS"/*
chmod 755 "$SYS"
chmod 644 "$SYS"/*
chcon u:object_r:system_file:s0 "$SYS" "$SYS"/* 2>/dev/null || true
if [ -d "$APEX" ]; then
  mount --bind "$SYS" "$APEX"
  for Z in $(pidof zygote zygote64); do
    nsenter --mount=/proc/$Z/ns/mnt -- mount --bind "$SYS" "$APEX" || true
    for P in $(ps -o PID -P "$Z" | tail -n +2); do
      nsenter --mount=/proc/$P/ns/mnt -- mount --bind "$SYS" "$APEX" 2>/dev/null || true
    done
  done
fi
rm -rf "$STORE"
echo AKEV_SYSTEM_CA_OK
"#;

/// Undoes [`SYSTEM_CA_SCRIPT`] for new processes; apps already running keep the CA until they
/// restart (a reboot removes it everywhere).
pub const SYSTEM_CA_REMOVE_SCRIPT: &str = r#"SYS=/system/etc/security/cacerts
APEX=/apex/com.android.conscrypt/cacerts
if [ -d "$APEX" ]; then
  for Z in $(pidof zygote zygote64); do
    nsenter --mount=/proc/$Z/ns/mnt -- umount "$APEX" 2>/dev/null || true
  done
  umount "$APEX" 2>/dev/null || true
fi
umount "$SYS" 2>/dev/null || true
rm -rf /data/local/tmp/akev-ca
echo AKEV_SYSTEM_CA_REMOVED
"#;

impl Drop for AndroidAdbInterceptor {
    fn drop(&mut self) {
        if let Some(task) = self.tunnel_task.lock().take() {
            task.abort();
        }
    }
}

impl AndroidAdbInterceptor {
    const DESCRIPTOR: Descriptor = Descriptor {
        id: "android-adb",
        name: "Android Device via ADB",
        description: "Proxy an Android device or emulator connected over ADB (USB tunnel, no Wi-Fi setup) and push the CA for installation",
        category: InterceptorCategory::Mobile,
        tags: tags::ANDROID,
        icon: "android",
        needs_config: true,
    };

    pub(crate) fn new(sys: Arc<Sys>) -> Self {
        AndroidAdbInterceptor {
            sys,
            scan: ProbeCache::new(PROBE_TTL),
            details: Mutex::new(HashMap::new()),
            active: Arc::new(Mutex::new(HashMap::new())),
            tunnel_task: Mutex::new(None),
            op: tokio::sync::Mutex::new(()),
            system_trusted: Mutex::new(HashMap::new()),
        }
    }

    /// Whether (and how) commands can run as root on `serial`.
    async fn root_shell(&self, adb: &Path, serial: &str) -> Option<RootShell> {
        let is_root = |out: Result<String>| out.is_ok_and(|o| o.trim() == "0");
        if is_root(Self::adb(&self.sys, adb, Some(serial), &["shell", "id", "-u"], SHORT).await) {
            return Some(RootShell::Adb);
        }
        // `su` may wait for the user to allow it on the device.
        let su = Self::adb(
            &self.sys,
            adb,
            Some(serial),
            &["shell", "su", "-c", "id -u"],
            LONG,
        )
        .await;
        is_root(su).then_some(RootShell::Su)
    }

    /// Run `script` (pushed to the device first) as root.
    async fn run_as_root(
        &self,
        adb: &Path,
        serial: &str,
        root: RootShell,
        local: &Path,
        script: &str,
    ) -> Result<String> {
        atomic_write(local, script.as_bytes())?;
        let device_script = format!("{DEVICE_STAGING}/{}", file_name(local));
        Self::adb(
            &self.sys,
            adb,
            Some(serial),
            &["shell", "mkdir", "-p", DEVICE_STAGING],
            SHORT,
        )
        .await?;
        let local_s = local.to_string_lossy().into_owned();
        Self::adb(
            &self.sys,
            adb,
            Some(serial),
            &["push", &local_s, &device_script],
            LONG,
        )
        .await?;
        let run = format!("sh {device_script}");
        match root {
            RootShell::Adb => {
                Self::adb(
                    &self.sys,
                    adb,
                    Some(serial),
                    &["shell", "sh", &device_script],
                    LONG,
                )
                .await
            }
            RootShell::Su => {
                Self::adb(
                    &self.sys,
                    adb,
                    Some(serial),
                    &["shell", "su", "-c", &run],
                    LONG,
                )
                .await
            }
        }
    }

    /// Trust the CA as a system CA on a rooted device or emulator. `Ok(None)`: no root.
    async fn install_system_ca(
        &self,
        adb: &Path,
        serial: &str,
        ctx: &InterceptContext,
    ) -> Result<Option<RootShell>> {
        let Some(root) = self.root_shell(adb, serial).await else {
            return Ok(None);
        };
        let hash = &ctx.ca_subject_hash_old;
        if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(InterceptError::Failed("the CA has no subject hash".into()));
        }
        let dir = ctx.data_dir.join("android");
        let cert = dir.join(format!("{hash}.0"));
        atomic_write(&cert, ctx.ca_pem.as_bytes())?;
        Self::adb(
            &self.sys,
            adb,
            Some(serial),
            &["shell", "mkdir", "-p", DEVICE_STAGING],
            SHORT,
        )
        .await?;
        let cert_s = cert.to_string_lossy().into_owned();
        let device_cert = format!("{DEVICE_STAGING}/{hash}.0");
        Self::adb(
            &self.sys,
            adb,
            Some(serial),
            &["push", &cert_s, &device_cert],
            LONG,
        )
        .await?;
        let script = SYSTEM_CA_SCRIPT.replace("@HASH@", hash);
        let out = self
            .run_as_root(adb, serial, root, &dir.join("akev-system-ca.sh"), &script)
            .await?;
        if out.contains("AKEV_SYSTEM_CA_OK") {
            Ok(Some(root))
        } else {
            Err(InterceptError::Failed(format!(
                "the system CA script didn't finish: {}",
                out.lines().last().unwrap_or("no output")
            )))
        }
    }

    /// Where the journal of `data_dir` is kept (see [`crate::journal::journal_file`]).
    fn journal_path(&self, data_dir: &Path) -> PathBuf {
        journal_file(&self.sys.env, data_dir, JOURNAL_FILE)
    }

    async fn adb(
        sys: &Sys,
        adb: &Path,
        serial: Option<&str>,
        args: &[&str],
        timeout: Duration,
    ) -> Result<String> {
        let mut spec = CommandSpec::new(adb).timeout(timeout);
        if let Some(s) = serial {
            spec = spec.args(["-s", s]);
        }
        let spec = spec.args(args.iter().copied());
        let out = sys
            .runner
            .output(&spec)
            .await
            .map_err(|e| InterceptError::Failed(format!("adb {}: {e}", args.join(" "))))?;
        if out.success() {
            Ok(out.stdout)
        } else {
            Err(InterceptError::Failed(format!(
                "adb {} failed: {}",
                args.join(" "),
                out.error_summary()
            )))
        }
    }

    async fn devices(&self) -> DeviceScan {
        let sys = self.sys.clone();
        self.scan
            .get(|| async move {
                let Some(adb) = find_adb(&sys.env) else {
                    return DeviceScan::default();
                };
                match Self::adb(&sys, &adb, None, &["devices", "-l"], SHORT).await {
                    Ok(out) => DeviceScan {
                        adb: Some(adb),
                        devices: parse_adb_devices(&out),
                        error: None,
                    },
                    Err(e) => DeviceScan {
                        adb: Some(adb),
                        devices: Vec::new(),
                        error: Some(e.to_string()),
                    },
                }
            })
            .await
    }

    async fn details_for(&self, adb: &Path, dev: &AdbDevice) -> DeviceDetails {
        if let Some(d) = self.details.lock().get(&dev.serial) {
            return d.clone();
        }
        let props = match Self::adb(
            &self.sys,
            adb,
            Some(&dev.serial),
            &["shell", "getprop"],
            SHORT,
        )
        .await
        {
            Ok(out) => parse_getprop(&out),
            Err(e) => {
                tracing::debug!(serial = %dev.serial, "getprop failed: {e}");
                return device_details(dev, &HashMap::new());
            }
        };
        let d = device_details(dev, &props);
        self.details.lock().insert(dev.serial.clone(), d.clone());
        d
    }
}

impl AndroidAdbInterceptor {
    /// Remove the proxy setting and the reverse tunnel from a device.
    async fn clear_device(sys: &Sys, adb: &Path, serial: &str, port: u16) -> Result<()> {
        // `:0` is the documented way to make the change take effect immediately.
        Self::adb(
            sys,
            adb,
            Some(serial),
            &["shell", "settings", "put", "global", "http_proxy", ":0"],
            SHORT,
        )
        .await?;
        let tunnel = format!("tcp:{port}");
        if let Err(e) = Self::adb(
            sys,
            adb,
            Some(serial),
            &["reverse", "--remove", &tunnel],
            SHORT,
        )
        .await
        {
            tracing::debug!(serial, "couldn't remove reverse tunnel: {e}");
        }
        Ok(())
    }

    /// Clear devices left proxied by an Akev process that's gone (crash recovery), whatever
    /// data directory it used. Entries for devices that aren't connected are kept until they
    /// show up; entries of running Akev instances (this one included) are left alone. Callers
    /// hold `op`.
    async fn recover_stale(&self, ctx: &InterceptContext, scan: &DeviceScan) {
        let mut paths = vec![self.journal_path(&ctx.data_dir)];
        paths.extend(scan_journals(&self.sys.env, JOURNAL_FILE));
        let journals: Vec<(PathBuf, Vec<JournalEntry>)> = dedup(paths)
            .into_iter()
            .map(|p| {
                let entries = read_journal(&p);
                (p, entries)
            })
            .filter(|(_, entries)| !entries.is_empty())
            .collect();
        if journals.is_empty() {
            return;
        }
        let Some(adb) = &scan.adb else { return };
        let active = self.active.lock().clone();

        // Who's still around? (One process scan, only when another process' entry needs it.)
        let procs = if journals
            .iter()
            .flat_map(|(_, entries)| entries)
            .any(|e| !e.is_ours())
        {
            self.sys.invalidate_processes().await;
            self.sys.processes(false).await
        } else {
            Arc::new(Vec::new())
        };
        let own = std::process::id();
        let live_owner = |e: &JournalEntry| -> bool {
            if e.is_ours() {
                return active.contains_key(&e.serial);
            }
            if procs.is_empty() {
                // Can't tell: never undo the settings of a possibly running instance.
                return true;
            }
            e.pid != own
                && procs
                    .iter()
                    .any(|p| p.pid == e.pid && same_program(&p.name, &e.process_name))
        };
        // Devices someone alive is responsible for: never cleared on behalf of a dead run.
        let mut managed: HashSet<String> = active.keys().cloned().collect();
        for (_, entries) in &journals {
            for e in entries.iter().filter(|e| live_owner(e)) {
                managed.insert(e.serial.clone());
            }
        }

        for (path, entries) in journals {
            let mut keep = Vec::with_capacity(entries.len());
            let mut changed = false;
            for entry in entries {
                if live_owner(&entry) {
                    keep.push(entry);
                    continue;
                }
                if managed.contains(&entry.serial) {
                    // Re-proxied since by a live instance: its setting must stay.
                    changed = true;
                    continue;
                }
                let connected = scan
                    .devices
                    .iter()
                    .any(|d| d.serial == entry.serial && d.is_ready());
                if !connected {
                    keep.push(entry);
                    continue;
                }
                match Self::clear_device(&self.sys, adb, &entry.serial, entry.port).await {
                    Ok(()) => {
                        tracing::info!(serial = %entry.serial, "cleared stale Akev proxy setting from Android device");
                        changed = true;
                    }
                    Err(e) => {
                        tracing::warn!(serial = %entry.serial, "couldn't clear stale proxy setting: {e}");
                        keep.push(entry);
                    }
                }
            }
            if changed {
                write_journal(&path, &keep);
            }
        }
    }

    /// Keep `adb reverse` alive (it's lost when a cable is replugged or adbd restarts).
    fn ensure_tunnel_task(&self, adb: PathBuf) {
        let mut slot = self.tunnel_task.lock();
        if slot.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }
        let (sys, active) = (self.sys.clone(), self.active.clone());
        *slot = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(TUNNEL_REFRESH).await;
                let devices: Vec<(String, u16)> =
                    active.lock().iter().map(|(s, p)| (s.clone(), *p)).collect();
                if devices.is_empty() {
                    break;
                }
                for (serial, port) in devices {
                    let tunnel = format!("tcp:{port}");
                    if let Err(e) = Self::adb(
                        &sys,
                        &adb,
                        Some(&serial),
                        &["reverse", &tunnel, &tunnel],
                        SHORT,
                    )
                    .await
                    {
                        tracing::debug!(%serial, "reverse tunnel refresh failed: {e}");
                    }
                }
            }
        }));
    }
}

/// Steps shown to the user for installing the pushed CA as a user certificate.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn install_steps() -> Vec<&'static str> {
    vec![
        "On the device open Settings → Security & privacy → More security settings → Encryption & credentials (wording varies by vendor).",
        "Tap \"Install a certificate\" → \"CA certificate\", confirm the warning, and pick Download/akev-ca.crt.",
        "Browsers (Chrome, Firefox with its own setting) and debuggable apps trust user CAs; release apps targeting Android 7+ only do if their network security config opts in.",
    ]
}

#[async_trait]
impl Interceptor for AndroidAdbInterceptor {
    fn id(&self) -> &str {
        Self::DESCRIPTOR.id
    }

    async fn info(&self, ctx: &InterceptContext) -> InterceptorInfo {
        let d = Self::DESCRIPTOR;
        let scan = self.devices().await;
        // Crash recovery runs on every refresh, but never concurrently with itself or with an
        // activation/deactivation (whose journal entry isn't backed by `active` yet). Those
        // can take a while: skip recovery this time rather than stall the listing.
        if let Ok(_guard) = self.op.try_lock() {
            self.recover_stale(ctx, &scan).await;
        }
        let Some(adb) = scan.adb.clone() else {
            return InterceptorInfo {
                unavailable_reason: Some(
                    "adb not found: install Android platform-tools (or set ANDROID_HOME)".into(),
                ),
                ..d.info()
            };
        };
        let active = self.active.lock().clone();
        let mut devices = Vec::new();
        for dev in &scan.devices {
            let details = if dev.is_ready() {
                Some(self.details_for(&adb, dev).await)
            } else {
                None
            };
            devices.push(json!({
                "id": dev.serial,
                "name": details.as_ref().map(|d| d.name.clone()).unwrap_or_else(|| dev.serial.clone()),
                "state": dev.state,
                "model": details.as_ref().and_then(|d| d.model.clone()),
                "manufacturer": details.as_ref().and_then(|d| d.manufacturer.clone()),
                "androidVersion": details.as_ref().and_then(|d| d.android_version.clone()),
                "sdk": details.as_ref().and_then(|d| d.sdk),
                "emulator": dev.is_emulator(),
                "active": active.contains_key(&dev.serial),
            }));
        }
        // Forget cached names of devices that went away.
        self.details
            .lock()
            .retain(|s, _| scan.devices.iter().any(|d| &d.serial == s));
        let ready: Vec<&str> = scan
            .devices
            .iter()
            .filter(|d| d.is_ready())
            .map(|d| d.serial.as_str())
            .collect();
        let reason = if let Some(e) = &scan.error {
            Some(e.clone())
        } else if ready.is_empty() {
            Some(match scan.devices.first() {
                Some(dev) if dev.state == "unauthorized" => {
                    format!(
                        "{} is unauthorized: accept the USB debugging prompt on the device",
                        dev.serial
                    )
                }
                Some(dev) if dev.state == "no permissions" => {
                    format!(
                        "No permission to access {} (missing udev rules?)",
                        dev.serial
                    )
                }
                Some(dev) => format!("{} is {}", dev.serial, dev.state),
                None => {
                    "No devices: enable USB debugging and connect a device, or start an emulator"
                        .into()
                }
            })
        } else {
            None
        };
        let any_active = ready.iter().any(|s| active.contains_key(*s));
        InterceptorInfo {
            available: !ready.is_empty(),
            active: any_active,
            unavailable_reason: reason,
            metadata: json!({ "adbPath": adb, "devices": devices }),
            ..d.info()
        }
    }

    async fn activate(
        &self,
        ctx: &InterceptContext,
        options: serde_json::Value,
    ) -> Result<ActivationResult> {
        self.scan.invalidate().await;
        let scan = self.devices().await;
        let adb = scan
            .adb
            .clone()
            .ok_or_else(|| InterceptError::Unavailable("adb not found".into()))?;
        let ready: Vec<&AdbDevice> = scan.devices.iter().filter(|d| d.is_ready()).collect();
        let serial = match opt_str(&options, "deviceId") {
            Some(id) => {
                let dev = scan
                    .devices
                    .iter()
                    .find(|d| d.serial == id)
                    .ok_or_else(|| {
                        InterceptError::Unavailable(format!("device {id} is not connected"))
                    })?;
                if !dev.is_ready() {
                    return Err(InterceptError::Unavailable(format!(
                        "device {id} is {}",
                        dev.state
                    )));
                }
                id.to_string()
            }
            None => match ready.as_slice() {
                [only] => only.serial.clone(),
                [] => {
                    return Err(InterceptError::Unavailable(
                        "no Android device connected".into(),
                    ));
                }
                _ => {
                    return Err(InterceptError::Failed(
                        "several devices are connected: pass options.deviceId".into(),
                    ));
                }
            },
        };
        let port = ctx.proxy_port;
        let mut steps = Vec::new();
        let _guard = self.op.lock().await;

        // 1. The CA, for installation as a user certificate.
        let local_ca = ctx.data_dir.join("android").join("akev-ca.crt");
        atomic_write(&local_ca, &ctx.ca_der)?;
        let local_ca_s = local_ca.to_string_lossy().into_owned();
        let pushed = Self::adb(
            &self.sys,
            &adb,
            Some(&serial),
            &["push", &local_ca_s, DEVICE_CA_PATH],
            LONG,
        )
        .await;
        if let Err(e) = &pushed {
            tracing::warn!(%serial, "couldn't push CA: {e}");
        }
        steps.push(json!({ "step": "push-ca", "ok": pushed.is_ok(), "detail": pushed.as_ref().err().map(|e| e.to_string()) }));

        // 1b. Rooted devices and emulators: trust it as a system CA until the next reboot.
        let system_ca = if options.get("systemTrust").and_then(|v| v.as_bool()) == Some(false) {
            None
        } else {
            match self.install_system_ca(&adb, &serial, ctx).await {
                Ok(Some(root)) => {
                    self.system_trusted.lock().insert(serial.clone(), root);
                    steps.push(json!({ "step": "system-ca", "ok": true }));
                    Some(true)
                }
                Ok(None) => {
                    steps.push(json!({ "step": "system-ca", "ok": false, "detail": "no root access (adb root or su)" }));
                    None
                }
                Err(e) => {
                    tracing::warn!(%serial, "couldn't install the system CA: {e}");
                    steps
                        .push(json!({ "step": "system-ca", "ok": false, "detail": e.to_string() }));
                    Some(false)
                }
            }
        };

        // 2. USB tunnel to the proxy; fall back to a LAN address / emulator host alias.
        let tunnel = format!("tcp:{port}");
        let reversed = Self::adb(
            &self.sys,
            &adb,
            Some(&serial),
            &["reverse", &tunnel, &tunnel],
            SHORT,
        )
        .await;
        let proxy_host = if reversed.is_ok() {
            "127.0.0.1".to_string()
        } else if serial.starts_with("emulator-") {
            "10.0.2.2".to_string()
        } else if let Some(host) = ctx.lan_addresses.first() {
            host.clone()
        } else {
            return Err(InterceptError::Failed(format!(
                "adb reverse failed ({}) and no LAN address is available for the device",
                reversed.err().map(|e| e.to_string()).unwrap_or_default()
            )));
        };
        steps.push(
            json!({ "step": "reverse-tunnel", "ok": reversed.is_ok(), "proxyHost": proxy_host }),
        );

        // 3. Journal first (crash safety), then the global proxy setting.
        let journal_path = self.journal_path(&ctx.data_dir);
        let mut journal = read_journal(&journal_path);
        journal.retain(|e| e.serial != serial);
        journal.push(JournalEntry {
            serial: serial.clone(),
            port,
            pid: std::process::id(),
            instance: instance_id().to_string(),
            process_name: own_process_name(&self.sys).await,
        });
        write_journal(&journal_path, &journal);
        let proxy = format!("{proxy_host}:{port}");
        if let Err(e) = Self::adb(
            &self.sys,
            &adb,
            Some(&serial),
            &["shell", "settings", "put", "global", "http_proxy", &proxy],
            SHORT,
        )
        .await
        {
            journal.retain(|e| e.serial != serial);
            write_journal(&journal_path, &journal);
            return Err(e);
        }
        steps.push(json!({ "step": "http-proxy", "ok": true, "proxy": proxy }));

        self.active.lock().insert(serial.clone(), port);
        if reversed.is_ok() {
            self.ensure_tunnel_task(adb);
        }
        let mut message = format!("Device {serial} now uses the Akev proxy ({proxy}).");
        if system_ca == Some(true) {
            message.push_str(
                " The Akev CA is trusted system-wide (by every app) until the device restarts.",
            );
        } else if pushed.is_ok() {
            message.push_str(" For HTTPS, install the CA from Download/akev-ca.crt (Settings → Security → Install a certificate → CA certificate).");
        }
        Ok(ActivationResult {
            success: true,
            message: Some(message),
            metadata: json!({
                "deviceId": serial,
                "proxy": proxy,
                "caPath": if pushed.is_ok() { json!(DEVICE_CA_PATH) } else { serde_json::Value::Null },
                "systemTrust": system_ca == Some(true),
                "steps": steps,
                "installSteps": install_steps(),
                "manualRecovery": "adb shell settings put global http_proxy :0",
            }),
        })
    }

    async fn deactivate(&self, ctx: &InterceptContext, options: serde_json::Value) -> Result<()> {
        let _guard = self.op.lock().await;
        let targets: Vec<(String, u16)> = match opt_str(&options, "deviceId") {
            Some(id) => {
                let port = self
                    .active
                    .lock()
                    .get(id)
                    .copied()
                    .unwrap_or(ctx.proxy_port);
                vec![(id.to_string(), port)]
            }
            None => self
                .active
                .lock()
                .iter()
                .map(|(s, p)| (s.clone(), *p))
                .collect(),
        };
        if targets.is_empty() {
            return Ok(());
        }
        let adb = find_adb(&self.sys.env)
            .ok_or_else(|| InterceptError::Unavailable("adb not found".into()))?;
        let journal_path = self.journal_path(&ctx.data_dir);
        let mut journal = read_journal(&journal_path);
        let mut first_err = None;
        for (serial, port) in targets {
            self.active.lock().remove(&serial);
            let root = self.system_trusted.lock().remove(&serial);
            if let Some(root) = root {
                let local = ctx
                    .data_dir
                    .join("android")
                    .join("akev-system-ca-remove.sh");
                if let Err(e) = self
                    .run_as_root(&adb, &serial, root, &local, SYSTEM_CA_REMOVE_SCRIPT)
                    .await
                {
                    tracing::warn!(%serial, "couldn't remove the system CA (a reboot does): {e}");
                }
            }
            match Self::clear_device(&self.sys, &adb, &serial, port).await {
                Ok(()) => journal.retain(|e| e.serial != serial),
                // Disconnected devices stay journaled and get cleared when they come back.
                Err(e) => {
                    tracing::warn!(%serial, "couldn't reset device proxy: {e}");
                    first_err.get_or_insert(e);
                }
            }
        }
        write_journal(&journal_path, &journal);
        if self.active.lock().is_empty()
            && let Some(task) = self.tunnel_task.lock().take()
        {
            task.abort();
        }
        self.scan.invalidate().await;
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn deactivate_all(&self, ctx: &InterceptContext) -> Result<()> {
        self.deactivate(ctx, json!({})).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testutil::{fake_env, make_exe};
    use crate::runner::mock::MockRunner;
    use crate::runner::{CommandOutput, ProcessEntry};
    use crate::util::testctx::ctx;

    const DEVICES: &str = "List of devices attached\n\
emulator-5554          device product:sdk_gphone64_x86_64 model:sdk_gphone64_x86_64 device:emu64xa transport_id:1\n\
R58M123456A            unauthorized usb:1-1 transport_id:2\n\
192.168.1.5:5555       offline transport_id:3\n\
0123456789ABCDEF       no permissions (missing udev rules? user is in the plugdev group); see [http://developer.android.com/tools/device.html] usb:1-2 transport_id:4\n\
HT7A1B234567           device usb:1-3 product:walleye model:Pixel_2 device:walleye transport_id:5\n\
\n";

    const GETPROP: &str = "[ro.build.version.release]: [14]\n[ro.build.version.sdk]: [34]\n\
[ro.product.manufacturer]: [Google]\n[ro.product.model]: [Pixel 7]\n[ro.boot.qemu.avd_name]: [Pixel_7_API_34]\n\
[persist.sys.locale]: [en-US]\n[weird line\n";

    #[test]
    fn parses_adb_devices() {
        let devs = parse_adb_devices(DEVICES);
        assert_eq!(devs.len(), 5);
        assert_eq!(devs[0].serial, "emulator-5554");
        assert!(devs[0].is_ready() && devs[0].is_emulator());
        assert_eq!(devs[0].props["model"], "sdk_gphone64_x86_64");
        assert_eq!(devs[1].state, "unauthorized");
        assert_eq!(devs[1].props["usb"], "1-1");
        assert_eq!(devs[2].serial, "192.168.1.5:5555");
        assert_eq!(devs[2].state, "offline");
        assert_eq!(devs[3].state, "no permissions");
        assert_eq!(devs[3].props.get("usb").map(String::as_str), Some("1-2"));
        assert!(!devs[3].props.contains_key("[http"));
        assert_eq!(devs[4].props["model"], "Pixel_2");
        assert!(parse_adb_devices("List of devices attached\n\n").is_empty());
        assert!(parse_adb_devices("* daemon not running; starting now at tcp:5037\n* daemon started successfully\nList of devices attached\n").is_empty());
    }

    #[test]
    fn parses_getprop_and_names() {
        let props = parse_getprop(GETPROP);
        assert_eq!(props["ro.build.version.release"], "14");
        assert_eq!(props.len(), 6);
        let devs = parse_adb_devices(DEVICES);
        let emu = device_details(&devs[0], &props);
        assert_eq!(emu.name, "Pixel 7 API 34");
        assert_eq!(emu.sdk, Some(34));
        let mut phone_props = props.clone();
        phone_props.remove("ro.boot.qemu.avd_name");
        assert_eq!(
            device_details(&devs[4], &phone_props).name,
            "Google Pixel 7"
        );
        phone_props.insert("ro.product.model".into(), "Google Pixel 7".into());
        assert_eq!(
            device_details(&devs[4], &phone_props).name,
            "Google Pixel 7"
        );
        let bare = device_details(&devs[4], &HashMap::new());
        assert_eq!(bare.name, "Pixel 2");
        let emu_bare = device_details(&devs[0], &HashMap::new());
        assert_eq!(emu_bare.name, "Android emulator");
    }

    fn setup() -> (
        tempfile::TempDir,
        Arc<MockRunner>,
        AndroidAdbInterceptor,
        InterceptContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        make_exe(&dir.path().join("usr/bin/adb"), "");
        let runner = MockRunner::new();
        // This test process is visible in the process table (liveness checks work).
        runner.processes.lock().push(ProcessEntry {
            pid: std::process::id(),
            name: "akev-test".into(),
            ..Default::default()
        });
        runner.on_prefix("adb devices -l", CommandOutput::ok(DEVICES));
        runner.on_prefix("adb -s", CommandOutput::ok(""));
        runner.on_prefix(
            "adb -s emulator-5554 shell getprop",
            CommandOutput::ok(GETPROP),
        );
        let i =
            AndroidAdbInterceptor::new(Sys::new(runner.clone(), fake_env(Os::Linux, dir.path())));
        let c = ctx(&dir.path().join("data"));
        (dir, runner, i, c)
    }

    /// The journal `i` keeps for `c`'s data directory.
    fn journal(i: &AndroidAdbInterceptor, c: &InterceptContext) -> PathBuf {
        i.journal_path(&c.data_dir)
    }

    /// A journal entry left by a process that's gone.
    fn dead_entry(serial: &str, port: u16) -> JournalEntry {
        JournalEntry {
            serial: serial.into(),
            port,
            pid: 999_999,
            instance: "crashed-run".into(),
            process_name: "akev".into(),
        }
    }

    fn proxy_cleared(runner: &MockRunner, serial: &str) -> bool {
        runner.call_lines().contains(&format!(
            "adb -s {serial} shell settings put global http_proxy :0"
        ))
    }

    #[tokio::test]
    async fn info_lists_devices() {
        let (_dir, _runner, i, c) = setup();
        let info = i.info(&c).await;
        assert!(info.available && info.needs_config && !info.active);
        let devices = info.metadata["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 5);
        assert_eq!(devices[0]["name"], json!("Pixel 7 API 34"));
        assert_eq!(devices[0]["androidVersion"], json!("14"));
        assert_eq!(devices[1]["state"], json!("unauthorized"));
    }

    #[tokio::test]
    async fn unavailable_reasons() {
        let dir = tempfile::tempdir().unwrap();
        let i = AndroidAdbInterceptor::new(Sys::new(
            MockRunner::new(),
            fake_env(Os::Linux, dir.path()),
        ));
        let c = ctx(&dir.path().join("data"));
        assert!(
            i.info(&c)
                .await
                .unavailable_reason
                .unwrap()
                .contains("adb not found")
        );

        make_exe(&dir.path().join("usr/bin/adb"), "");
        let runner = MockRunner::new();
        runner.on_prefix(
            "adb devices",
            CommandOutput::ok("List of devices attached\nR58M unauthorized usb:1-1\n"),
        );
        let i = AndroidAdbInterceptor::new(Sys::new(runner, fake_env(Os::Linux, dir.path())));
        let info = i.info(&c).await;
        assert!(!info.available);
        assert!(
            info.unavailable_reason
                .unwrap()
                .contains("accept the USB debugging prompt")
        );
    }

    #[tokio::test]
    async fn rooted_devices_trust_the_ca_as_a_system_ca() {
        let (_dir, runner, i, c) = setup();
        runner.on_prefix("adb -s emulator-5554 shell id -u", CommandOutput::ok("0\n"));
        runner.on_prefix(
            "adb -s emulator-5554 shell sh /data/local/tmp/akev-ca/akev-system-ca.sh",
            CommandOutput::ok("AKEV_SYSTEM_CA_OK\n"),
        );
        let res = i
            .activate(&c, json!({"deviceId": "emulator-5554"}))
            .await
            .unwrap();
        assert_eq!(res.metadata["systemTrust"], json!(true));
        assert!(res.message.unwrap().contains("system-wide"));
        let hash = &c.ca_subject_hash_old;
        let lines = runner.call_lines();
        assert!(
            lines
                .iter()
                .any(|l| l.ends_with(&format!("/data/local/tmp/akev-ca/{hash}.0"))),
            "{lines:?}"
        );
        let script = std::fs::read_to_string(c.data_dir.join("android/akev-system-ca.sh")).unwrap();
        assert!(script.contains(&format!("CERT=\"$DIR/{hash}.0\"")));
        assert!(!script.contains("@HASH@"));
        assert_eq!(
            std::fs::read_to_string(c.data_dir.join(format!("android/{hash}.0"))).unwrap(),
            c.ca_pem
        );
        i.deactivate_all(&c).await.unwrap();
        assert!(
            runner
                .call_lines()
                .iter()
                .any(|l| l.ends_with("shell sh /data/local/tmp/akev-ca/akev-system-ca-remove.sh")),
            "removed on deactivation"
        );
    }

    #[tokio::test]
    async fn su_is_used_when_adb_isnt_root_and_can_be_turned_off() {
        let (_dir, runner, i, c) = setup();
        runner.on_prefix(
            "adb -s emulator-5554 shell id -u",
            CommandOutput::ok("2000\n"),
        );
        runner.on_prefix(
            "adb -s emulator-5554 shell su -c id -u",
            CommandOutput::ok("0\n"),
        );
        runner.on_prefix(
            "adb -s emulator-5554 shell su -c sh /data/local/tmp/akev-ca/akev-system-ca.sh",
            CommandOutput::ok("AKEV_SYSTEM_CA_OK\n"),
        );
        let res = i
            .activate(&c, json!({"deviceId": "emulator-5554"}))
            .await
            .unwrap();
        assert_eq!(res.metadata["systemTrust"], json!(true));
        i.deactivate_all(&c).await.unwrap();
        // `systemTrust: false` keeps it a user CA only.
        let res = i
            .activate(
                &c,
                json!({"deviceId": "emulator-5554", "systemTrust": false}),
            )
            .await
            .unwrap();
        assert_eq!(res.metadata["systemTrust"], json!(false));
        assert!(res.message.unwrap().contains("Install a certificate"));
    }

    #[tokio::test]
    async fn activate_and_deactivate_device() {
        let (_dir, runner, i, c) = setup();
        assert!(
            matches!(
                i.activate(&c, json!({})).await,
                Err(InterceptError::Failed(_))
            ),
            "two devices: id required"
        );
        assert!(matches!(
            i.activate(&c, json!({"deviceId": "R58M123456A"})).await,
            Err(InterceptError::Unavailable(_))
        ));
        assert!(matches!(
            i.activate(&c, json!({"deviceId": "nope"})).await,
            Err(InterceptError::Unavailable(_))
        ));

        let res = i
            .activate(&c, json!({"deviceId": "emulator-5554"}))
            .await
            .unwrap();
        assert!(res.success);
        assert_eq!(res.metadata["proxy"], json!("127.0.0.1:8000"));
        let lines = runner.call_lines();
        assert!(lines.iter().any(|l| l.starts_with("adb -s emulator-5554 push ") && l.ends_with(DEVICE_CA_PATH)));
        assert!(lines.contains(&"adb -s emulator-5554 reverse tcp:8000 tcp:8000".to_string()));
        assert!(lines.contains(
            &"adb -s emulator-5554 shell settings put global http_proxy 127.0.0.1:8000".to_string()
        ));
        assert_eq!(
            std::fs::read(c.data_dir.join("android/akev-ca.crt")).unwrap(),
            c.ca_der
        );
        let entries = read_journal(&journal(&i, &c));
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_ours() && entries[0].process_name == "akev-test");
        assert!(i.info(&c).await.active);

        i.deactivate_all(&c).await.unwrap();
        let lines = runner.call_lines();
        assert!(
            lines.contains(
                &"adb -s emulator-5554 shell settings put global http_proxy :0".to_string()
            )
        );
        assert!(lines.contains(&"adb -s emulator-5554 reverse --remove tcp:8000".to_string()));
        assert!(read_journal(&journal(&i, &c)).is_empty());
        assert!(!journal(&i, &c).exists());
        assert!(!i.info(&c).await.active);
    }

    #[tokio::test]
    async fn falls_back_when_reverse_fails() {
        let (_dir, runner, i, c) = setup();
        runner.on_prefix(
            "adb -s HT7A1B234567 reverse",
            CommandOutput::failed("error: closed"),
        );
        let res = i
            .activate(&c, json!({"deviceId": "HT7A1B234567"}))
            .await
            .unwrap();
        assert_eq!(res.metadata["proxy"], json!("192.168.1.20:8000"));
        runner.on_prefix(
            "adb -s emulator-5554 reverse",
            CommandOutput::failed("error: closed"),
        );
        let res = i
            .activate(&c, json!({"deviceId": "emulator-5554"}))
            .await
            .unwrap();
        assert_eq!(res.metadata["proxy"], json!("10.0.2.2:8000"));
    }

    #[tokio::test]
    async fn stale_journal_entries_are_cleared() {
        let (_dir, runner, i, c) = setup();
        let path = journal(&i, &c);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_journal(
            &path,
            &[
                dead_entry("emulator-5554", 8123),
                dead_entry("disconnected", 8123),
            ],
        );
        i.info(&c).await;
        let lines = runner.call_lines();
        assert!(
            lines.contains(
                &"adb -s emulator-5554 shell settings put global http_proxy :0".to_string()
            )
        );
        assert!(lines.contains(&"adb -s emulator-5554 reverse --remove tcp:8123".to_string()));
        // The disconnected device is remembered for later.
        assert_eq!(
            read_journal(&path)
                .iter()
                .map(|e| e.serial.as_str())
                .collect::<Vec<_>>(),
            vec!["disconnected"]
        );
    }

    #[tokio::test]
    async fn listing_during_activation_keeps_the_new_journal_entry() {
        let (_dir, runner, i, c) = setup();
        // The device takes a while to apply the proxy setting...
        runner.delays.lock().push((
            "adb -s emulator-5554 shell settings put global http_proxy 127.0.0.1".into(),
            Duration::from_millis(300),
        ));
        let i = Arc::new(i);
        let activation = {
            let (i, c) = (i.clone(), c.clone());
            tokio::spawn(async move { i.activate(&c, json!({"deviceId": "emulator-5554"})).await })
        };
        let put = "adb -s emulator-5554 shell settings put global http_proxy 127.0.0.1:8000";
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !runner.call_lines().iter().any(|l| l == put) {
            assert!(std::time::Instant::now() < deadline, "activation stalled");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // ...while the UI refreshes the interceptor list.
        let info = i.info(&c).await;
        assert!(info.available, "listing isn't blocked by the activation");
        activation.await.unwrap().unwrap();

        assert!(
            !proxy_cleared(&runner, "emulator-5554"),
            "{:?}",
            runner.call_lines()
        );
        let entries = read_journal(&journal(&i, &c));
        assert_eq!(
            entries
                .iter()
                .map(|e| e.serial.as_str())
                .collect::<Vec<_>>(),
            vec!["emulator-5554"],
            "the crash-safety record must survive"
        );
        assert!(i.info(&c).await.active);
        assert!(!proxy_cleared(&runner, "emulator-5554"));
    }

    #[tokio::test]
    async fn ephemeral_run_journal_is_recovered_by_the_next_instance() {
        let (dir, runner, i, c) = setup();
        let env = fake_env(Os::Linux, dir.path());
        // An `--ephemeral` run proxied the emulator, then died; its temp dir is gone.
        let ephemeral = tempfile::tempdir().unwrap();
        let eph = ctx(&ephemeral.path().join("akev-XyZ"));
        let path = journal_file(&env, &eph.data_dir, JOURNAL_FILE);
        assert!(
            path.starts_with(dir.path().join("home/.local/share/akev/recovery")),
            "{}",
            path.display()
        );
        assert_eq!(path, i.journal_path(&eph.data_dir));
        write_journal(&path, &[dead_entry("emulator-5554", 8123)]);
        drop(ephemeral);

        // Another instance (its own data dir) lists interceptors.
        i.info(&c).await;
        assert!(proxy_cleared(&runner, "emulator-5554"));
        assert!(
            runner
                .call_lines()
                .contains(&"adb -s emulator-5554 reverse --remove tcp:8123".to_string())
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn devices_of_a_running_instance_are_left_alone() {
        let (dir, runner, i, c) = setup();
        let env = fake_env(Os::Linux, dir.path());
        runner.processes.lock().push(ProcessEntry {
            pid: 4242,
            name: "akev".into(),
            ..Default::default()
        });
        let other = journal_file(&env, Path::new("/tmp/akev-running"), JOURNAL_FILE);
        let live = JournalEntry {
            pid: 4242,
            instance: "running-instance".into(),
            ..dead_entry("emulator-5554", 8000)
        };
        write_journal(&other, std::slice::from_ref(&live));
        i.info(&c).await;
        assert!(!proxy_cleared(&runner, "emulator-5554"));
        assert_eq!(read_journal(&other), vec![live.clone()]);

        // Once that instance is gone (pid reused by something else), it's stale.
        runner.processes.lock().retain(|p| p.pid != 4242);
        runner.processes.lock().push(ProcessEntry {
            pid: 4242,
            name: "vim".into(),
            ..Default::default()
        });
        i.info(&c).await;
        assert!(proxy_cleared(&runner, "emulator-5554"));
        assert!(!other.exists());
    }

    #[tokio::test]
    async fn stale_entry_never_clears_a_device_proxied_since() {
        let (dir, runner, i, c) = setup();
        let env = fake_env(Os::Linux, dir.path());
        i.activate(&c, json!({"deviceId": "emulator-5554"}))
            .await
            .unwrap();
        // A dead run's record for the same device (same port as ours).
        let dead = journal_file(&env, Path::new("/tmp/akev-dead"), JOURNAL_FILE);
        write_journal(&dead, &[dead_entry("emulator-5554", 8000)]);
        i.info(&c).await;
        assert!(!proxy_cleared(&runner, "emulator-5554"));
        assert!(
            !runner
                .call_lines()
                .iter()
                .any(|l| l.contains("reverse --remove")),
            "our tunnel stays"
        );
        assert!(!dead.exists(), "the stale record is dropped");
        assert_eq!(read_journal(&journal(&i, &c)).len(), 1);
    }

    #[test]
    fn finds_adb_in_sdk() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        assert_eq!(find_adb(&env), None);
        let sdk_adb = env.home_path("Android/Sdk/platform-tools/adb").unwrap();
        make_exe(&sdk_adb, "");
        assert_eq!(find_adb(&env), Some(sdk_adb));
    }
}
