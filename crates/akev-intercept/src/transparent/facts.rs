//! What the transparent-capture commands depend on: who Akev runs as, where the user's
//! systemd manager lives in the cgroup v2 tree, and which firewall / launcher tools exist.
//! Detection only reads files (`/proc/self/cgroup`, `/etc/passwd`, `/sys/fs/cgroup`) and
//! scans `PATH`; it never runs anything.

use crate::host::{HostEnv, Os};
use serde::{Deserialize, Serialize};

/// Facts about this machine (`GET /api/transparent` → `system`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemFacts {
    /// `linux`, `macos`, `windows` or `other`.
    pub platform: String,
    /// User id Akev runs as (Unix).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    /// Its user name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// A cgroup v2 (unified) hierarchy is mounted at `/sys/fs/cgroup`.
    pub cgroup_v2: bool,
    /// Akev's own cgroup (`/user.slice/.../app.slice/foo.scope`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own_cgroup: Option<String>,
    /// The cgroup of the user's systemd manager, without a leading slash
    /// (`user.slice/user-1000.slice/user@1000.service`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_manager_cgroup: Option<String>,
    /// `nft` is installed.
    pub nft: bool,
    /// `iptables` is installed.
    pub iptables: bool,
    /// `systemd-run` is installed.
    pub systemd_run: bool,
    /// Akev's executable (for the `setcap` command of mark-based exclusion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
}

/// Directories searched for firewall tools besides `PATH` (they often live in `sbin`).
const SBIN_DIRS: &[&str] = &["/usr/sbin", "/sbin", "/usr/bin", "/bin"];

impl SystemFacts {
    /// Detect facts for the current process on `env`.
    pub fn detect(env: &HostEnv) -> SystemFacts {
        let uid = current_uid();
        let read = |p: &str| std::fs::read_to_string(env.sys_path(p)).ok();
        let own_cgroup = read("/proc/self/cgroup").and_then(|s| parse_cgroup_v2(&s));
        let user = uid.and_then(|uid| {
            read("/etc/passwd")
                .and_then(|p| user_name(&p, uid))
                .or_else(|| env.var("USER").map(str::to_string))
        });
        let user_manager_cgroup = uid.map(|uid| {
            own_cgroup
                .as_deref()
                .and_then(|cg| user_manager_from(cg, uid))
                .unwrap_or_else(|| default_user_manager(uid))
        });
        let tool = |name: &str| {
            env.find_in_path(name).is_some()
                || SBIN_DIRS
                    .iter()
                    .any(|d| crate::host::is_executable(&env.sys_path(&format!("{d}/{name}"))))
        };
        SystemFacts {
            platform: env.os.as_str().to_string(),
            uid,
            user,
            cgroup_v2: env.os == Os::Linux
                && env.sys_path("/sys/fs/cgroup/cgroup.controllers").exists(),
            own_cgroup,
            user_manager_cgroup,
            nft: tool("nft"),
            iptables: tool("iptables"),
            systemd_run: tool("systemd-run"),
            executable: std::env::current_exe()
                .ok()
                .map(|p| p.to_string_lossy().into_owned()),
        }
    }
}

#[cfg(unix)]
fn current_uid() -> Option<u32> {
    // SAFETY: getuid() has no preconditions and cannot fail.
    Some(unsafe { libc::getuid() })
}

#[cfg(not(unix))]
fn current_uid() -> Option<u32> {
    None
}

/// The cgroup v2 path in `/proc/<pid>/cgroup` (the `0::<path>` line).
pub fn parse_cgroup_v2(proc_cgroup: &str) -> Option<String> {
    proc_cgroup
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .map(str::trim)
        .filter(|p| p.starts_with('/'))
        .map(str::to_string)
}

/// `user.slice/user-1000.slice/user@1000.service` out of a cgroup path inside it.
pub fn user_manager_from(cgroup: &str, uid: u32) -> Option<String> {
    let needle = format!("user@{uid}.service");
    let parts: Vec<&str> = cgroup.trim_matches('/').split('/').collect();
    let at = parts.iter().position(|p| *p == needle)?;
    Some(parts[..=at].join("/"))
}

/// Where systemd puts the manager of `uid` by default.
pub fn default_user_manager(uid: u32) -> String {
    format!("user.slice/user-{uid}.slice/user@{uid}.service")
}

/// The name of `uid` in an `/etc/passwd` file.
pub fn user_name(passwd: &str, uid: u32) -> Option<String> {
    passwd.lines().find_map(|line| {
        let mut f = line.split(':');
        let name = f.next()?;
        let _pw = f.next()?;
        let id: u32 = f.next()?.trim().parse().ok()?;
        (id == uid && !name.is_empty()).then(|| name.to_string())
    })
}

/// The uid of user `name` in an `/etc/passwd` file.
pub fn user_id(passwd: &str, name: &str) -> Option<u32> {
    passwd.lines().find_map(|line| {
        let mut f = line.split(':');
        let n = f.next()?;
        let _pw = f.next()?;
        let id = f.next()?.trim().parse().ok()?;
        (n == name).then_some(id)
    })
}

/// Resolve a user given as a number or a name (via `/etc/passwd` under `env.root`).
pub fn resolve_user(env: &HostEnv, user: &str) -> Option<(u32, Option<String>)> {
    let user = user.trim();
    let passwd = std::fs::read_to_string(env.sys_path("/etc/passwd")).unwrap_or_default();
    if let Ok(uid) = user.parse::<u32>() {
        return Some((uid, user_name(&passwd, uid)));
    }
    user_id(&passwd, user).map(|uid| (uid, Some(user.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testutil::{fake_env, make_exe};

    #[test]
    fn cgroup_parsing() {
        let proc = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/term.scope\n";
        let cg = parse_cgroup_v2(proc).unwrap();
        assert_eq!(
            cg,
            "/user.slice/user-1000.slice/user@1000.service/app.slice/term.scope"
        );
        assert_eq!(
            user_manager_from(&cg, 1000).as_deref(),
            Some("user.slice/user-1000.slice/user@1000.service")
        );
        assert_eq!(user_manager_from(&cg, 1001), None);
        assert_eq!(user_manager_from("/system.slice/akev.service", 0), None);
        // cgroup v1 hybrid lines are ignored.
        assert_eq!(
            parse_cgroup_v2("12:pids:/user.slice\n0::/init.scope\n").as_deref(),
            Some("/init.scope")
        );
        assert_eq!(parse_cgroup_v2("1:name=systemd:/x\n"), None);
        assert_eq!(
            default_user_manager(7),
            "user.slice/user-7.slice/user@7.service"
        );
    }

    #[test]
    fn passwd_lookups() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n\
                      dev:x:1000:1000:Dev:/home/dev:/bin/zsh\n\
                      broken line\n\
                      capture:x:1001:1001::/home/capture:/bin/bash\n";
        assert_eq!(user_name(passwd, 1000).as_deref(), Some("dev"));
        assert_eq!(user_name(passwd, 0).as_deref(), Some("root"));
        assert_eq!(user_name(passwd, 4242), None);
        assert_eq!(user_id(passwd, "capture"), Some(1001));
        assert_eq!(user_id(passwd, "nobody"), None);
    }

    #[test]
    fn detects_from_a_fake_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let env = fake_env(Os::Linux, root);
        std::fs::create_dir_all(root.join("proc/self")).unwrap();
        std::fs::create_dir_all(root.join("etc")).unwrap();
        std::fs::create_dir_all(root.join("sys/fs/cgroup")).unwrap();
        std::fs::write(root.join("sys/fs/cgroup/cgroup.controllers"), "cpu io\n").unwrap();
        let uid = current_uid().unwrap_or(0);
        std::fs::write(
            root.join("proc/self/cgroup"),
            format!("0::/user.slice/user-{uid}.slice/user@{uid}.service/app.slice/x.scope\n"),
        )
        .unwrap();
        std::fs::write(
            root.join("etc/passwd"),
            format!("someone:x:{uid}:{uid}::/home/someone:/bin/sh\n"),
        )
        .unwrap();
        make_exe(&root.join("usr/sbin/nft"), "");
        make_exe(&root.join("usr/bin/systemd-run"), "");
        let facts = SystemFacts::detect(&env);
        assert_eq!(facts.platform, "linux");
        assert!(facts.cgroup_v2 && facts.nft && facts.systemd_run && !facts.iptables);
        assert!(facts.own_cgroup.as_deref().unwrap().ends_with("x.scope"));
        #[cfg(unix)]
        {
            assert_eq!(facts.uid, Some(uid));
            assert_eq!(facts.user.as_deref(), Some("someone"));
            assert_eq!(facts.user_manager_cgroup, Some(default_user_manager(uid)));
        }
        assert!(facts.executable.is_some());

        assert_eq!(
            resolve_user(&env, &uid.to_string()),
            Some((uid, Some("someone".into())))
        );
        assert_eq!(
            resolve_user(&env, "someone"),
            Some((uid, Some("someone".into())))
        );
        assert_eq!(resolve_user(&env, "ghost"), None);
        assert_eq!(resolve_user(&env, "4242"), Some((4242, None)));

        // A bare machine: no cgroup v2, no tools.
        let bare = tempfile::tempdir().unwrap();
        let facts = SystemFacts::detect(&fake_env(Os::Linux, bare.path()));
        assert!(!facts.cgroup_v2 && !facts.nft && !facts.systemd_run);
        assert!(facts.own_cgroup.is_none());
        #[cfg(unix)]
        assert_eq!(
            facts.user_manager_cgroup,
            facts.uid.map(default_user_manager)
        );
    }
}
