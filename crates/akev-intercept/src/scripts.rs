//! Shell snippets that switch an already-open terminal to the proxy (and back).
//!
//! Every setup script:
//! * saves the previous value of each variable in [`ENV_VAR_NAMES`] once (as exported
//!   `__AKEV_OLD_<NAME>` variables, so it also works from subshells), so `akev-off` restores
//!   the exact previous state — set values come back, unset ones are removed;
//! * is idempotent (running it twice never saves Akev's own values as "previous");
//! * extends rather than replaces [`APPEND_VARS`] (e.g. keeps `-Xmx` in `JAVA_TOOL_OPTIONS`);
//! * leaves the prompt untouched and prints a one-line confirmation.

use crate::envvars::{APPEND_VARS, ENV_VAR_NAMES};
use std::path::{Path, PathBuf};

/// Shell dialects with dedicated scripts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shell {
    /// bash and other POSIX shells (sh, dash, ksh).
    Bash,
    /// zsh (same syntax as bash here).
    Zsh,
    /// fish.
    Fish,
    /// PowerShell (Windows PowerShell 5.1 and PowerShell 7+ on any OS).
    PowerShell,
    /// Windows `cmd.exe`.
    Cmd,
}

impl Shell {
    /// Parse a shell name (case-insensitive; also accepts paths like `/usr/bin/zsh`).
    /// Unknown names fall back to [`Shell::Bash`] (POSIX syntax).
    pub fn parse(name: &str) -> Shell {
        let base = name
            .trim()
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let base = base.strip_suffix(".exe").unwrap_or(&base);
        match base {
            "zsh" => Shell::Zsh,
            "fish" => Shell::Fish,
            "powershell" | "pwsh" | "ps" | "ps1" | "posh" => Shell::PowerShell,
            "cmd" | "bat" | "batch" | "command" => Shell::Cmd,
            _ => Shell::Bash,
        }
    }

    /// Canonical name (`bash`, `zsh`, `fish`, `powershell`, `cmd`).
    pub fn as_str(self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
            Shell::Fish => "fish",
            Shell::PowerShell => "powershell",
            Shell::Cmd => "cmd",
        }
    }

    /// All supported shells.
    pub const ALL: [Shell; 5] = [
        Shell::Bash,
        Shell::Zsh,
        Shell::Fish,
        Shell::PowerShell,
        Shell::Cmd,
    ];
}

/// Single-quote for POSIX shells.
pub(crate) fn posix_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Single-quote for fish (only `\` and `'` are special inside).
pub(crate) fn fish_quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', r"\\").replace('\'', r"\'"))
}

/// Single-quote for PowerShell (which also treats typographic single quotes as quotes).
pub(crate) fn ps_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// Escape text for a batch-file line so cmd reads it back verbatim.
///
/// `quoted` says whether the text starts inside a `"..."` region (`set "NAME=<here>"`). The
/// value is kept as is — including double quotes, which carry meaning (the JVM needs the ones
/// around a truststore path with spaces in `JAVA_TOOL_OPTIONS`); `set "NAME=value"` takes
/// everything up to the line's last quote. Only what cmd itself would interpret is escaped:
/// `%` everywhere (`%%`), and `^ & | < > ( )` where an embedded quote has ended the quoted
/// region (`^`-escaped). Line breaks can't be represented and are dropped.
fn cmd_escape(s: &str, quoted: bool) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut in_quotes = quoted;
    for c in s.chars() {
        match c {
            '\r' | '\n' => {}
            '%' => out.push_str("%%"),
            '"' => {
                in_quotes = !in_quotes;
                out.push('"');
            }
            '^' | '&' | '|' | '<' | '>' | '(' | ')' if !in_quotes => {
                out.push('^');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// [`cmd_escape`] for text inside `set "NAME=..."` (or another `"..."`).
fn cmd_value(s: &str) -> String {
    cmd_escape(s, true)
}

fn is_append(name: &str) -> bool {
    APPEND_VARS.contains(&name)
}

/// Names saved/restored, deduplicated case-insensitively for Windows-only shells.
fn tracked_names(case_insensitive: bool) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for name in ENV_VAR_NAMES {
        if case_insensitive && out.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            continue;
        }
        out.push(name);
    }
    out
}

/// Setup script for `shell` applying `vars` (variables outside [`ENV_VAR_NAMES`] are set
/// but not saved/restored). `cmd_reset_file` (cmd only) is the batch file the `akev-off`
/// doskey macro calls — cmd has no functions (see [`ensure_cmd_reset_file`]).
pub(crate) fn setup_script(
    shell: Shell,
    vars: &[(String, String)],
    proxy_url: &str,
    cmd_reset_file: Option<&Path>,
) -> String {
    match shell {
        Shell::Bash | Shell::Zsh => posix_setup(shell, vars, proxy_url),
        Shell::Fish => fish_setup(vars, proxy_url),
        Shell::PowerShell => ps_setup(vars, proxy_url),
        Shell::Cmd => cmd_setup(vars, proxy_url, cmd_reset_file),
    }
}

/// Write the cmd reset script to `<data_dir>/terminal/akev-off.cmd` (if it changed) and
/// return its path.
pub(crate) fn ensure_cmd_reset_file(data_dir: &Path) -> Option<PathBuf> {
    let path = data_dir.join("terminal").join("akev-off.cmd");
    match crate::util::write_if_changed(&path, reset_script(Shell::Cmd).as_bytes()) {
        Ok(()) => Some(path),
        Err(e) => {
            tracing::warn!(path = %path.display(), "couldn't write the cmd reset script: {e}");
            None
        }
    }
}

/// Reset script for `shell` (no-op with a message when interception isn't active).
pub(crate) fn reset_script(shell: Shell) -> String {
    match shell {
        Shell::Bash | Shell::Zsh => format!(
            "# Akev: stop intercepting this {} session\n{}",
            shell.as_str(),
            posix_reset_body("")
        ),
        Shell::Fish => format!(
            "# Akev: stop intercepting this fish session\n{}",
            fish_reset_body("")
        ),
        Shell::PowerShell => format!(
            "# Akev: stop intercepting this PowerShell session\n{}",
            ps_reset_body("")
        ),
        Shell::Cmd => cmd_reset(),
    }
}

// ---------------------------------------------------------------- POSIX (bash, zsh, sh)

fn posix_save_block() -> String {
    let mut s = String::from("if [ -z \"${__AKEV_SAVED+x}\" ]; then\n  export __AKEV_SAVED=1\n");
    for name in tracked_names(false) {
        s.push_str(&format!(
            "  if [ -n \"${{{name}+x}}\" ]; then export __AKEV_OLD_{name}=\"${name}\"; fi\n"
        ));
    }
    s.push_str("fi\n");
    s
}

/// `export NAME=value` lines (append-vars keep the saved original in front).
pub(crate) fn posix_exports(vars: &[(String, String)]) -> String {
    let mut s = String::new();
    for (k, v) in vars {
        if is_append(k) {
            s.push_str(&format!(
                "export {k}=\"${{__AKEV_OLD_{k}:+$__AKEV_OLD_{k} }}\"{}\n",
                posix_quote(v)
            ));
        } else {
            s.push_str(&format!("export {k}={}\n", posix_quote(v)));
        }
    }
    s
}

fn posix_reset_body(indent: &str) -> String {
    let mut s = format!("{indent}if [ -n \"${{__AKEV_SAVED+x}}\" ]; then\n");
    for name in tracked_names(false) {
        s.push_str(&format!(
            "{indent}  if [ -n \"${{__AKEV_OLD_{name}+x}}\" ]; then export {name}=\"$__AKEV_OLD_{name}\"; unset __AKEV_OLD_{name}; else unset {name}; fi\n"
        ));
    }
    s.push_str(&format!("{indent}  unset __AKEV_SAVED\n"));
    s.push_str(&format!(
        "{indent}  printf '%s\\n' 'Akev interception disabled'\n"
    ));
    s.push_str(&format!("{indent}else\n"));
    s.push_str(&format!(
        "{indent}  printf '%s\\n' 'Akev interception is not active in this shell'\n"
    ));
    s.push_str(&format!("{indent}fi\n"));
    s
}

fn posix_setup(shell: Shell, vars: &[(String, String)], proxy_url: &str) -> String {
    let mut s = format!(
        "# Akev terminal interception ({}). Run `akev-off` to stop.\n",
        shell.as_str()
    );
    s.push_str(&posix_save_block());
    s.push_str(&posix_exports(vars));
    s.push_str("akev_off() {\n");
    s.push_str(&posix_reset_body("  "));
    s.push_str("}\n");
    s.push_str("alias akev-off='akev_off'\n");
    s.push_str(&format!(
        "printf '\\033[1;32m%s\\033[0m %s\\n' 'Akev interception enabled' {}\n",
        posix_quote(&format!("(proxy {proxy_url}; run akev-off to stop)"))
    ));
    s
}

/// A POSIX `sh` launcher for fresh terminals: export everything, print the banner, delete
/// itself and `exec` the user's shell. With `pid_var`, that variable is set to the shell's pid
/// (`exec` keeps the launcher's pid): processes started from the shell inherit it with a value
/// that isn't their own pid, which tells the terminal's shell apart from its descendants.
pub(crate) fn posix_launcher(
    vars: &[(String, String)],
    proxy_url: &str,
    shell: &str,
    login: bool,
    pid_var: Option<&str>,
) -> String {
    let mut s =
        String::from("#!/bin/sh\n# Generated by Akev: a terminal with all traffic proxied.\n");
    s.push_str(&posix_save_block());
    s.push_str(&posix_exports(vars));
    s.push_str(&format!(
        "printf '\\033[1;32m%s\\033[0m %s\\n' 'Akev interception enabled' {}\n",
        posix_quote(&format!("(proxy {proxy_url}; close this terminal to stop)"))
    ));
    s.push_str("rm -f -- \"$0\" 2>/dev/null\n");
    if let Some(var) = pid_var {
        s.push_str(&format!("export {var}=\"$$\"\n"));
    }
    s.push_str(&format!(
        "exec {} {}\n",
        posix_quote(shell),
        if login { "-l" } else { "-i" }
    ));
    s
}

// ---------------------------------------------------------------- fish

fn fish_reset_body(indent: &str) -> String {
    let mut s = format!("{indent}if set -q __AKEV_SAVED\n");
    for name in tracked_names(false) {
        s.push_str(&format!(
            "{indent}    if set -q __AKEV_OLD_{name}; set -gx {name} $__AKEV_OLD_{name}; set -e __AKEV_OLD_{name}; else; set -e {name}; end\n"
        ));
    }
    s.push_str(&format!("{indent}    set -e __AKEV_SAVED\n"));
    s.push_str(&format!("{indent}    echo 'Akev interception disabled'\n"));
    s.push_str(&format!("{indent}else\n"));
    s.push_str(&format!(
        "{indent}    echo 'Akev interception is not active in this shell'\n"
    ));
    s.push_str(&format!("{indent}end\n"));
    s
}

fn fish_setup(vars: &[(String, String)], proxy_url: &str) -> String {
    let mut s = String::from("# Akev terminal interception (fish). Run `akev-off` to stop.\n");
    s.push_str("if not set -q __AKEV_SAVED\n    set -gx __AKEV_SAVED 1\n");
    for name in tracked_names(false) {
        s.push_str(&format!(
            "    if set -q {name}; set -gx __AKEV_OLD_{name} ${name}; end\n"
        ));
    }
    s.push_str("end\n");
    for (k, v) in vars {
        if is_append(k) {
            s.push_str(&format!(
                "if test -n \"$__AKEV_OLD_{k}\"\n    set -gx {k} \"$__AKEV_OLD_{k} \"{q}\nelse\n    set -gx {k} {q}\nend\n",
                q = fish_quote(v)
            ));
        } else {
            s.push_str(&format!("set -gx {k} {}\n", fish_quote(v)));
        }
    }
    s.push_str("function akev-off --description 'Stop Akev interception in this shell'\n");
    s.push_str(&fish_reset_body("    "));
    s.push_str("end\n");
    s.push_str(&format!(
        "printf '\\033[1;32m%s\\033[0m %s\\n' 'Akev interception enabled' {}\n",
        fish_quote(&format!("(proxy {proxy_url}; run akev-off to stop)"))
    ));
    s
}

// ---------------------------------------------------------------- PowerShell

/// `$PSDefaultParameterValues` keys saved by setup and restored by `akev-off`.
const PS_DEFAULT_KEYS: [&str; 2] = ["Invoke-WebRequest:Proxy", "Invoke-RestMethod:Proxy"];

fn ps_list(items: &[&str]) -> String {
    format!(
        "@({})",
        items
            .iter()
            .map(|i| ps_quote(i))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn ps_reset_body(indent: &str) -> String {
    let names = tracked_names(false);
    let mut s = format!("{indent}if (Test-Path Env:__AKEV_SAVED) {{\n");
    // Two passes: on Windows `http_proxy` and `HTTP_PROXY` are the same variable, so the saved
    // copies must all be read before any is cleared.
    s.push_str(&format!("{indent}    $__akevNames = {}\n", ps_list(&names)));
    s.push_str(&format!(
        "{indent}    foreach ($__akevName in $__akevNames) {{ [Environment]::SetEnvironmentVariable($__akevName, [Environment]::GetEnvironmentVariable(\"__AKEV_OLD_$__akevName\")) }}\n"
    ));
    s.push_str(&format!(
        "{indent}    foreach ($__akevName in $__akevNames) {{ [Environment]::SetEnvironmentVariable(\"__AKEV_OLD_$__akevName\", $null) }}\n"
    ));
    s.push_str(&format!(
        "{indent}    Remove-Item Env:__AKEV_SAVED -ErrorAction SilentlyContinue\n"
    ));
    s.push_str(&format!(
        "{indent}    foreach ($__akevKey in {}) {{ [void]$PSDefaultParameterValues.Remove($__akevKey) }}\n",
        ps_list(&PS_DEFAULT_KEYS)
    ));
    s.push_str(&format!("{indent}    if ($global:__AkevOldDefaults) {{\n"));
    s.push_str(&format!(
        "{indent}        foreach ($__akevKey in @($global:__AkevOldDefaults.Keys)) {{ $PSDefaultParameterValues[$__akevKey] = $global:__AkevOldDefaults[$__akevKey] }}\n"
    ));
    s.push_str(&format!("{indent}        Remove-Variable -Name __AkevOldDefaults -Scope Global -ErrorAction SilentlyContinue\n"));
    s.push_str(&format!("{indent}    }}\n"));
    s.push_str(&format!(
        "{indent}    Write-Host 'Akev interception disabled'\n"
    ));
    s.push_str(&format!("{indent}}} else {{\n"));
    s.push_str(&format!(
        "{indent}    Write-Host 'Akev interception is not active in this session'\n"
    ));
    s.push_str(&format!("{indent}}}\n"));
    s
}

fn ps_setup(vars: &[(String, String)], proxy_url: &str) -> String {
    let names = tracked_names(false);
    let mut s =
        String::from("# Akev terminal interception (PowerShell). Run `akev-off` to stop.\n");
    s.push_str("if (-not (Test-Path Env:__AKEV_SAVED)) {\n    $env:__AKEV_SAVED = '1'\n");
    s.push_str(&format!(
        "    foreach ($__akevName in {}) {{\n",
        ps_list(&names)
    ));
    s.push_str("        $__akevValue = [Environment]::GetEnvironmentVariable($__akevName)\n");
    s.push_str(
        "        if ($null -ne $__akevValue) { [Environment]::SetEnvironmentVariable(\"__AKEV_OLD_$__akevName\", $__akevValue) }\n",
    );
    s.push_str("    }\n");
    s.push_str("    $global:__AkevOldDefaults = @{}\n");
    s.push_str(&format!(
        "    foreach ($__akevKey in {}) {{\n",
        ps_list(&PS_DEFAULT_KEYS)
    ));
    s.push_str(
        "        if ($PSDefaultParameterValues.ContainsKey($__akevKey)) { $global:__AkevOldDefaults[$__akevKey] = $PSDefaultParameterValues[$__akevKey] }\n",
    );
    s.push_str("    }\n}\n");
    for (k, v) in vars {
        if is_append(k) {
            s.push_str(&format!(
                "$__akevValue = [Environment]::GetEnvironmentVariable('__AKEV_OLD_{k}')\nif ($__akevValue) {{ $env:{k} = $__akevValue + ' ' + {q} }} else {{ $env:{k} = {q} }}\n",
                q = ps_quote(v)
            ));
        } else {
            s.push_str(&format!("$env:{k} = {}\n", ps_quote(v)));
        }
    }
    // Certificate checks stay on. Skipping them for the session would also cover hosts Akev
    // tunnels without inspecting (TLS passthrough) and `-NoProxy` calls, i.e. accept any
    // certificate from a network attacker. .NET on Windows/macOS validates against the OS
    // trust store (SSL_CERT_FILE only works on Linux), so say how to trust the Akev CA there.
    s.push_str(
        "if ($PSVersionTable.PSVersion.Major -lt 6) {\n    # Windows PowerShell's web cmdlets ignore proxy env vars.\n    $PSDefaultParameterValues['Invoke-WebRequest:Proxy'] = $env:HTTP_PROXY\n    $PSDefaultParameterValues['Invoke-RestMethod:Proxy'] = $env:HTTP_PROXY\n}\n",
    );
    s.push_str(
        "if (($PSVersionTable.PSVersion.Major -lt 6) -or (-not $IsLinux)) {\n    Write-Host 'HTTPS from PowerShell web cmdlets is checked against the system trust store: trust the Akev CA there to intercept it (Akev > System Proxy shows how).' -ForegroundColor Yellow\n}\n",
    );
    s.push_str("function global:akev-off {\n");
    s.push_str(&ps_reset_body("    "));
    s.push_str("}\n");
    s.push_str(
        "Remove-Variable -Name __akevName, __akevValue, __akevKey -ErrorAction SilentlyContinue\n",
    );
    s.push_str("Write-Host 'Akev interception enabled' -ForegroundColor Green -NoNewline\n");
    s.push_str(&format!(
        "Write-Host {}\n",
        ps_quote(&format!(" (proxy {proxy_url}; run akev-off to stop)"))
    ));
    s
}

// ---------------------------------------------------------------- cmd.exe

fn crlf(s: String) -> String {
    // cmd's label/goto handling misbehaves with LF-only batch files.
    s.replace("\r\n", "\n").replace('\n', "\r\n")
}

fn cmd_setup(vars: &[(String, String)], proxy_url: &str, reset_file: Option<&Path>) -> String {
    let names = tracked_names(true);
    let mut s = String::from("@echo off\nrem Akev terminal interception (cmd)\n");
    s.push_str("if defined __AKEV_SAVED goto akev_apply\nset \"__AKEV_SAVED=1\"\n");
    for name in &names {
        s.push_str(&format!(
            "if defined {name} set \"__AKEV_OLD_{name}=%{name}%\"\n"
        ));
    }
    s.push_str(":akev_apply\n");
    let mut seen: Vec<&str> = Vec::new();
    for (k, v) in vars {
        // Windows env names are case-insensitive: `http_proxy` *is* `HTTP_PROXY`.
        if seen.iter().any(|n| n.eq_ignore_ascii_case(k)) {
            continue;
        }
        seen.push(k);
        let v = cmd_value(v);
        s.push_str(&format!("set \"{k}={v}\"\n"));
        if is_append(k) {
            s.push_str(&format!(
                "if defined __AKEV_OLD_{k} set \"{k}=%__AKEV_OLD_{k}% {v}\"\n"
            ));
        }
    }
    match reset_file {
        Some(path) => {
            s.push_str(&format!(
                "doskey akev-off=call \"{}\"\n",
                cmd_value(&path.to_string_lossy())
            ));
            s.push_str(&format!(
                "echo Akev interception enabled (proxy {}; run akev-off to stop)\n",
                cmd_escape(proxy_url, false)
            ));
        }
        None => s.push_str(&format!(
            "echo Akev interception enabled (proxy {}; close this window to stop)\n",
            cmd_escape(proxy_url, false)
        )),
    }
    crlf(s)
}

fn cmd_reset() -> String {
    let names = tracked_names(true);
    let mut s = String::from("@echo off\nrem Akev: stop intercepting this cmd window\n");
    s.push_str("if not defined __AKEV_SAVED goto akev_inactive\n");
    for name in &names {
        s.push_str(&format!("set \"{name}=\"\n"));
        s.push_str(&format!(
            "if defined __AKEV_OLD_{name} set \"{name}=%__AKEV_OLD_{name}%\"\n"
        ));
        s.push_str(&format!("set \"__AKEV_OLD_{name}=\"\n"));
    }
    s.push_str("set \"__AKEV_SAVED=\"\necho Akev interception disabled\ngoto :eof\n");
    s.push_str(":akev_inactive\necho Akev interception is not active in this window\n");
    crlf(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envvars::{EnvConfig, build_env_vars};
    use std::path::Path;
    use std::process::Command;

    fn vars() -> Vec<(String, String)> {
        build_env_vars(&EnvConfig {
            proxy_host: "127.0.0.1".into(),
            proxy_port: 8000,
            ca_file: "/tmp/it's here/ca.pem".into(),
            bundle_file: "/tmp/dir with spaces/ca-bundle.pem".into(),
            java_truststore: Some("/tmp/dir with spaces/ts.p12".into()),
            proxy_credentials: None,
        })
    }

    fn have(cmd: &str) -> bool {
        Command::new(cmd)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn syntax_ok(shell: &str, script: &str) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("script");
        std::fs::write(&file, script).unwrap();
        let out = Command::new(shell).arg("-n").arg(&file).output().unwrap();
        assert!(
            out.status.success(),
            "{shell} -n failed: {}\n{script}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Run `script` in `shell`, then print selected variables (`<name>=<value|UNSET>`).
    fn run_and_dump(shell: &str, pre: &str, script: &str, post: &str, names: &[&str]) -> String {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("script");
        let mut full = String::from(pre);
        full.push_str(script);
        full.push_str(post);
        for n in names {
            full.push_str(&format!("if [ -n \"${{{n}+x}}\" ]; then printf '%s=%s\\n' {n} \"${n}\"; else echo {n}=UNSET; fi\n"));
        }
        std::fs::write(&file, full).unwrap();
        let out = Command::new(shell)
            .arg(&file)
            .env_remove("HTTP_PROXY")
            .env_remove("http_proxy")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{shell} exited with {}\nstderr: {}\nstdout: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn parse_shell_names() {
        assert_eq!(Shell::parse("bash"), Shell::Bash);
        assert_eq!(Shell::parse("/usr/bin/zsh"), Shell::Zsh);
        assert_eq!(Shell::parse("FISH"), Shell::Fish);
        assert_eq!(Shell::parse("pwsh"), Shell::PowerShell);
        assert_eq!(
            Shell::parse("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
            Shell::PowerShell
        );
        assert_eq!(Shell::parse("cmd.exe"), Shell::Cmd);
        assert_eq!(Shell::parse("sh"), Shell::Bash);
        assert_eq!(Shell::parse("nushell?"), Shell::Bash);
        for s in Shell::ALL {
            assert_eq!(Shell::parse(s.as_str()), s);
        }
    }

    #[test]
    fn quoting() {
        assert_eq!(posix_quote("it's"), r"'it'\''s'");
        assert_eq!(fish_quote(r"a\b'c"), r"'a\\b\'c'");
        assert_eq!(ps_quote("it's ‘x’"), "'it''s ‘‘x’’'");
        // Quotes are kept; what ends up outside them is escaped.
        assert_eq!(cmd_value("100%\"x\"\n"), "100%%\"x\"");
        assert_eq!(cmd_value("a&b\"c&d\"e&f"), "a&b\"c^&d\"e&f");
        assert_eq!(cmd_escape("(x) & y", false), "^(x^) ^& y");
    }

    /// What cmd.exe makes of a `set` line in a batch file: `%%` → `%`, quote tracking,
    /// `^` escapes outside quotes, `& | < >` outside quotes start another command (`None`),
    /// then `set "NAME=value"` keeps everything between the first and the last quote.
    fn cmd_set_result(line: &str) -> Option<(String, String)> {
        let line = line.strip_prefix("set ")?;
        let line = line.replace("%%", "%");
        let mut args = String::new();
        let mut in_quotes = false;
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    in_quotes = !in_quotes;
                    args.push(c);
                }
                '^' if !in_quotes => args.push(chars.next()?),
                '&' | '|' | '<' | '>' if !in_quotes => return None,
                _ => args.push(c),
            }
        }
        let args = args.strip_prefix('"')?;
        let end = args.rfind('"')?;
        let (name, value) = args[..end].split_once('=')?;
        Some((name.to_string(), value.to_string()))
    }

    #[test]
    fn cmd_set_lines_keep_values_verbatim() {
        for value in [
            "-Dx=1 -Djavax.net.ssl.trustStore=\"C:\\Users\\John Doe\\akev\\ts.p12\" -Dy=2",
            "C:\\Users\\Tom & Jerry\\ca.pem",
            "-Dts=\"C:\\Users\\Tom & Jerry (x)\\ts.p12\" -Dz=^1",
            "100% sure",
            "a\"b",
        ] {
            let line = format!("set \"X={}\"", cmd_value(value));
            assert_eq!(
                cmd_set_result(&line),
                Some(("X".to_string(), value.to_string())),
                "{line}"
            );
        }
    }

    #[test]
    fn cmd_keeps_the_quoted_java_truststore_path() {
        let v = vars();
        let jto = &v.iter().find(|(k, _)| k == "JAVA_TOOL_OPTIONS").unwrap().1;
        assert!(jto.contains("-Djavax.net.ssl.trustStore=\"/tmp/dir with spaces/ts.p12\""));
        let setup = setup_script(Shell::Cmd, &v, "http://127.0.0.1:8000", None);
        let lines: Vec<&str> = setup.split("\r\n").collect();
        let set = lines
            .iter()
            .find(|l| l.starts_with("set \"JAVA_TOOL_OPTIONS="))
            .unwrap();
        assert_eq!(
            cmd_set_result(set),
            Some(("JAVA_TOOL_OPTIONS".to_string(), jto.clone())),
            "{set}"
        );
        let append = lines
            .iter()
            .find(|l| l.starts_with("if defined __AKEV_OLD_JAVA_TOOL_OPTIONS set "))
            .unwrap();
        assert!(
            append.contains("-Djavax.net.ssl.trustStore=\"/tmp/dir with spaces/ts.p12\""),
            "{append}"
        );
        let (name, value) = cmd_set_result(
            append
                .strip_prefix("if defined __AKEV_OLD_JAVA_TOOL_OPTIONS ")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(name, "JAVA_TOOL_OPTIONS");
        assert!(value.ends_with(&format!(" {jto}")), "{value}");
    }

    #[test]
    fn posix_scripts_parse_with_bash_and_zsh() {
        let v = vars();
        for shell in ["bash", "sh", "dash", "zsh"] {
            if !have(shell) && !(shell == "sh" || shell == "dash") {
                continue;
            }
            if Command::new(shell).arg("-c").arg("true").output().is_err() {
                continue;
            }
            let script_shell = if shell == "zsh" {
                Shell::Zsh
            } else {
                Shell::Bash
            };
            syntax_ok(
                shell,
                &setup_script(script_shell, &v, "http://127.0.0.1:8000", None),
            );
            syntax_ok(shell, &reset_script(script_shell));
            syntax_ok(
                shell,
                &posix_launcher(
                    &v,
                    "http://127.0.0.1:8000",
                    "/bin/bash",
                    false,
                    Some("AKEV_TERMINAL_PID"),
                ),
            );
        }
    }

    #[test]
    fn posix_setup_and_reset_roundtrip() {
        if Command::new("sh").arg("-c").arg("true").output().is_err() {
            return;
        }
        let v = vars();
        let setup = setup_script(Shell::Bash, &v, "http://127.0.0.1:8000", None);
        let names = [
            "HTTP_PROXY",
            "http_proxy",
            "NO_PROXY",
            "JAVA_TOOL_OPTIONS",
            "SSL_CERT_FILE",
            "NODE_EXTRA_CA_CERTS",
        ];
        let pre =
            "export NO_PROXY=corp.example\nexport JAVA_TOOL_OPTIONS=-Xmx1g\nunset SSL_CERT_FILE\n";

        let after_setup = run_and_dump("sh", pre, &setup, "", &names);
        assert!(
            after_setup.contains("HTTP_PROXY=http://127.0.0.1:8000\n"),
            "{after_setup}"
        );
        assert!(after_setup.contains("http_proxy=http://127.0.0.1:8000\n"));
        assert!(after_setup.contains("NO_PROXY=\n"));
        assert!(after_setup.contains("SSL_CERT_FILE=/tmp/dir with spaces/ca-bundle.pem\n"));
        assert!(after_setup.contains("NODE_EXTRA_CA_CERTS=/tmp/it's here/ca.pem\n"));
        assert!(after_setup.contains("JAVA_TOOL_OPTIONS=-Xmx1g -Dhttp.proxyHost=127.0.0.1"));
        assert!(after_setup.contains("Akev interception enabled"));

        // Running setup twice must not save Akev's own values or duplicate appended flags.
        let twice = format!("{setup}{setup}");
        let after_twice = run_and_dump("sh", pre, &twice, "", &names);
        assert_eq!(
            after_twice.matches("-Dhttp.proxyHost").count(),
            1,
            "{after_twice}"
        );

        let reset = reset_script(Shell::Bash);
        let after_reset = run_and_dump("sh", pre, &twice, &reset, &names);
        assert!(after_reset.contains("HTTP_PROXY=UNSET\n"), "{after_reset}");
        assert!(after_reset.contains("http_proxy=UNSET\n"));
        assert!(after_reset.contains("NO_PROXY=corp.example\n"));
        assert!(after_reset.contains("JAVA_TOOL_OPTIONS=-Xmx1g\n"));
        assert!(after_reset.contains("SSL_CERT_FILE=UNSET\n"));
        assert!(after_reset.contains("Akev interception disabled"));
        assert!(!after_reset.contains("__AKEV_OLD"));

        // The function defined by the setup script does the same. (`have`: on Windows a bare
        // `bash` is System32's WSL launcher, which fails without a Linux distribution.)
        if have("bash") {
            let via_fn = run_and_dump("bash", pre, &setup, "akev_off\n", &names);
            assert!(via_fn.contains("HTTP_PROXY=UNSET\n"));
            assert!(via_fn.contains("NO_PROXY=corp.example\n"));
        }

        // Reset in a fresh shell (never set up) doesn't touch user variables.
        let untouched = run_and_dump(
            "sh",
            "export HTTP_PROXY=http://corp:3128\n",
            "",
            &reset,
            &["HTTP_PROXY"],
        );
        assert!(untouched.contains("HTTP_PROXY=http://corp:3128\n"));
        assert!(untouched.contains("not active"));
    }

    #[test]
    fn launcher_execs_shell_and_self_deletes() {
        if Command::new("sh").arg("-c").arg("true").output().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let fake_shell = dir.path().join("fake shell.sh");
        std::fs::write(&fake_shell, "#!/bin/sh\necho \"SHELL-ARGS:$*\"\necho \"PROXY:$HTTP_PROXY\"\necho \"TID:$AKEV_TERMINAL_ID\"\necho \"PID:$AKEV_TERMINAL_PID SELF:$$\"\n").unwrap();
        crate::util::make_executable(&fake_shell).unwrap();
        let mut v = vars();
        v.push(("AKEV_TERMINAL_ID".into(), "abc".into()));
        let launcher = dir.path().join("launch.sh");
        std::fs::write(
            &launcher,
            posix_launcher(
                &v,
                "http://127.0.0.1:8000",
                &fake_shell.to_string_lossy(),
                true,
                Some("AKEV_TERMINAL_PID"),
            ),
        )
        .unwrap();
        let out = Command::new("sh").arg(&launcher).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("SHELL-ARGS:-l"), "{stdout}");
        assert!(stdout.contains("PROXY:http://127.0.0.1:8000"));
        assert!(stdout.contains("TID:abc"));
        // The exec'd shell's pid is recorded (its children inherit someone else's pid).
        let pid_line = stdout.lines().find(|l| l.starts_with("PID:")).unwrap();
        let (marker, own) = pid_line
            .strip_prefix("PID:")
            .unwrap()
            .split_once(" SELF:")
            .unwrap();
        assert!(!marker.is_empty() && marker == own, "{pid_line}");
        assert!(!launcher.exists(), "launcher should delete itself");
    }

    #[test]
    fn fish_scripts() {
        let v = vars();
        let setup = setup_script(Shell::Fish, &v, "http://127.0.0.1:8000", None);
        assert!(setup.contains("set -gx HTTP_PROXY 'http://127.0.0.1:8000'"));
        assert!(setup.contains(r"set -gx NODE_EXTRA_CA_CERTS '/tmp/it\'s here/ca.pem'"));
        assert!(setup.contains("function akev-off"));
        assert!(
            setup.contains("if set -q HTTP_PROXY; set -gx __AKEV_OLD_HTTP_PROXY $HTTP_PROXY; end")
        );
        let reset = reset_script(Shell::Fish);
        assert!(reset.contains("set -e HTTP_PROXY"));
        if have("fish") {
            syntax_ok("fish", &setup);
            syntax_ok("fish", &reset);
        }
    }

    #[test]
    fn powershell_scripts() {
        let v = vars();
        let setup = setup_script(Shell::PowerShell, &v, "http://127.0.0.1:8000", None);
        assert!(setup.contains("$env:HTTP_PROXY = 'http://127.0.0.1:8000'"));
        assert!(setup.contains("$env:NODE_EXTRA_CA_CERTS = '/tmp/it''s here/ca.pem'"));
        assert!(setup.contains("function global:akev-off"));
        assert!(setup.contains("Invoke-WebRequest:Proxy"));
        // TLS validation is never switched off (passthrough hosts and -NoProxy calls would
        // accept any certificate).
        assert!(!setup.contains("SkipCertificateCheck"), "{setup}");
        assert!(setup.contains("trust the Akev CA"));
        assert!(setup.contains("__AKEV_OLD_JAVA_TOOL_OPTIONS"));
        let reset = reset_script(Shell::PowerShell);
        assert!(reset.contains("Remove-Item Env:__AKEV_SAVED"));
        // Balanced braces (cheap structural check; pwsh isn't usually available in CI).
        for script in [&setup, &reset] {
            assert_eq!(
                script.matches('{').count(),
                script.matches('}').count(),
                "{script}"
            );
            assert_eq!(script.matches('(').count(), script.matches(')').count());
        }
        if have("pwsh") {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("s.ps1");
            std::fs::write(&file, format!("{setup}\nakev-off\n")).unwrap();
            let out = Command::new("pwsh")
                .args(["-NoProfile", "-File"])
                .arg(&file)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn cmd_scripts() {
        let mut v = vars();
        v.push(("WEIRD".into(), "50%\"quoted\"".into()));
        let setup = setup_script(
            Shell::Cmd,
            &v,
            "http://127.0.0.1:8000",
            Some(Path::new("C:\\Users\\me\\akev\\terminal\\akev-off.cmd")),
        );
        assert!(
            setup.contains(
                "doskey akev-off=call \"C:\\Users\\me\\akev\\terminal\\akev-off.cmd\"\r\n"
            ),
            "{setup}"
        );
        assert!(setup.contains("run akev-off to stop"));
        assert!(!setup_script(Shell::Cmd, &v, "http://x", None).contains("doskey"));
        assert!(setup.contains("\r\n"));
        assert!(
            !setup.replace("\r\n", "").contains('\n'),
            "LF-only lines in a batch file"
        );
        assert!(setup.contains("set \"HTTP_PROXY=http://127.0.0.1:8000\"\r\n"));
        // Case-insensitive env: the lowercase duplicates are skipped.
        assert!(!setup.contains("set \"http_proxy="));
        assert!(
            setup.contains("set \"WEIRD=50%%\"quoted\"\"\r\n"),
            "{setup}"
        );
        assert!(setup.contains("if defined __AKEV_OLD_JAVA_TOOL_OPTIONS set \"JAVA_TOOL_OPTIONS=%__AKEV_OLD_JAVA_TOOL_OPTIONS% -D"));
        assert!(setup.contains("goto akev_apply\r\n") && setup.contains(":akev_apply\r\n"));
        let reset = reset_script(Shell::Cmd);
        assert!(reset.contains(
            "if defined __AKEV_OLD_HTTP_PROXY set \"HTTP_PROXY=%__AKEV_OLD_HTTP_PROXY%\"\r\n"
        ));
        assert!(!reset.contains("http_proxy"));
        assert!(reset.contains(":akev_inactive\r\n"));

        let dir = tempfile::tempdir().unwrap();
        let file = ensure_cmd_reset_file(dir.path()).unwrap();
        assert_eq!(file, dir.path().join("terminal/akev-off.cmd"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), reset);
    }

    #[test]
    fn reset_covers_every_tracked_variable() {
        for shell in [Shell::Bash, Shell::Fish, Shell::PowerShell] {
            let reset = reset_script(shell);
            for name in ENV_VAR_NAMES {
                assert!(reset.contains(name), "{shell:?} reset misses {name}");
            }
        }
        let _ = Path::new("/");
    }
}
