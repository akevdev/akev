//! The curl options the importer understands, and which unknown ones take a value.

/// How a data option treats its argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DataKind {
    /// `-d`, `--data`, `--data-ascii`: `@file` reads a file (CR/LF stripped).
    Ascii,
    /// `--data-raw`: literal.
    Raw,
    /// `--data-binary`: `@file` reads a file verbatim.
    Binary,
    /// `--data-urlencode`.
    UrlEncode,
    /// `--json`: like `--data-binary`, plus JSON headers; pieces concatenate without `&`.
    Json,
}

/// A recognised option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Opt {
    Request,
    Header,
    Data(DataKind),
    /// `-F` (`false`) / `--form-string` (`true`).
    Form {
        literal: bool,
    },
    User,
    Oauth2Bearer,
    /// `--digest`, `--ntlm`, `--negotiate`, `--anyauth`, `--aws-sigv4`: no Basic header.
    NonBasicAuth {
        takes_value: bool,
    },
    Basic,
    Cookie,
    UserAgent,
    Referer,
    Head,
    Get,
    Compressed,
    Url,
    UrlQuery,
    UploadFile,
    /// `-:`/`--next`: the rest of the command line is another request.
    Next,
    /// Recognised but irrelevant to the request.
    Ignored {
        takes_value: bool,
    },
}

impl Opt {
    pub(super) fn takes_value(self) -> bool {
        match self {
            Opt::Request
            | Opt::Header
            | Opt::Data(_)
            | Opt::Form { .. }
            | Opt::User
            | Opt::Oauth2Bearer
            | Opt::Cookie
            | Opt::UserAgent
            | Opt::Referer
            | Opt::Url
            | Opt::UrlQuery
            | Opt::UploadFile => true,
            Opt::NonBasicAuth { takes_value } | Opt::Ignored { takes_value } => takes_value,
            Opt::Basic | Opt::Head | Opt::Get | Opt::Compressed | Opt::Next => false,
        }
    }

    /// Booleans that `--no-<name>` turns off.
    pub(super) fn is_boolean(self) -> bool {
        !self.takes_value()
    }
}

/// Look up a short option letter.
pub(super) fn short(c: char) -> Opt {
    match c {
        'X' => Opt::Request,
        'H' => Opt::Header,
        'd' => Opt::Data(DataKind::Ascii),
        'F' => Opt::Form { literal: false },
        'u' => Opt::User,
        'b' => Opt::Cookie,
        'A' => Opt::UserAgent,
        'e' => Opt::Referer,
        'I' => Opt::Head,
        'G' => Opt::Get,
        'T' => Opt::UploadFile,
        ':' => Opt::Next,
        // Other options that take a value.
        'c' | 'C' | 'D' | 'E' | 'K' | 'm' | 'o' | 'P' | 'Q' | 'r' | 't' | 'U' | 'w' | 'x' | 'y'
        | 'Y' | 'z' => Opt::Ignored { takes_value: true },
        _ => Opt::Ignored { takes_value: false },
    }
}

/// Look up a long option (without the leading `--`). Returns `None` for unknown options.
pub(super) fn long(name: &str) -> Option<Opt> {
    let name = name.strip_prefix("expand-").unwrap_or(name);
    Some(match name {
        "request" => Opt::Request,
        "header" => Opt::Header,
        "data" | "data-ascii" => Opt::Data(DataKind::Ascii),
        "data-raw" => Opt::Data(DataKind::Raw),
        "data-binary" => Opt::Data(DataKind::Binary),
        "data-urlencode" => Opt::Data(DataKind::UrlEncode),
        "json" => Opt::Data(DataKind::Json),
        "form" => Opt::Form { literal: false },
        "form-string" => Opt::Form { literal: true },
        "user" => Opt::User,
        "oauth2-bearer" => Opt::Oauth2Bearer,
        "digest" | "ntlm" | "ntlm-wb" | "negotiate" | "anyauth" => {
            Opt::NonBasicAuth { takes_value: false }
        }
        "aws-sigv4" => Opt::NonBasicAuth { takes_value: true },
        "basic" => Opt::Basic,
        "cookie" => Opt::Cookie,
        "user-agent" => Opt::UserAgent,
        "referer" => Opt::Referer,
        "head" => Opt::Head,
        "get" => Opt::Get,
        "compressed" => Opt::Compressed,
        "url" => Opt::Url,
        "url-query" => Opt::UrlQuery,
        "upload-file" => Opt::UploadFile,
        "next" => Opt::Next,
        name if VALUE_OPTIONS.contains(&name) => Opt::Ignored { takes_value: true },
        name if FLAG_OPTIONS.contains(&name) => Opt::Ignored { takes_value: false },
        _ => return None,
    })
}

/// Long options (curl 8.x) that take an argument and don't affect the request itself.
const VALUE_OPTIONS: &[&str] = &[
    "abstract-unix-socket",
    "alt-svc",
    "cacert",
    "capath",
    "cert",
    "cert-type",
    "ciphers",
    "config",
    "connect-timeout",
    "connect-to",
    "continue-at",
    "cookie-jar",
    "create-file-mode",
    "crlfile",
    "curves",
    "delegation",
    "dns-interface",
    "dns-ipv4-addr",
    "dns-ipv6-addr",
    "dns-servers",
    "doh-url",
    "dump-header",
    "ech",
    "egd-file",
    "engine",
    "etag-compare",
    "etag-save",
    "expect100-timeout",
    "ftp-account",
    "ftp-alternative-to-user",
    "ftp-method",
    "ftp-port",
    "ftp-ssl-ccc-mode",
    "happy-eyeballs-timeout-ms",
    "haproxy-clientip",
    "help",
    "hostpubmd5",
    "hostpubsha256",
    "hsts",
    "interface",
    "ip-tos",
    "ipfs-gateway",
    "keepalive-cnt",
    "keepalive-time",
    "key",
    "key-type",
    "knownhosts",
    "krb",
    "krb4",
    "libcurl",
    "limit-rate",
    "local-port",
    "login-options",
    "mail-auth",
    "mail-from",
    "mail-rcpt",
    "max-filesize",
    "max-redirs",
    "max-time",
    "netrc-file",
    "noproxy",
    "output",
    "output-dir",
    "parallel-max",
    "pass",
    "pinnedpubkey",
    "preproxy",
    "proto",
    "proto-default",
    "proto-redir",
    "proxy",
    "proxy-cacert",
    "proxy-capath",
    "proxy-cert",
    "proxy-cert-type",
    "proxy-ciphers",
    "proxy-crlfile",
    "proxy-header",
    "proxy-key",
    "proxy-key-type",
    "proxy-pass",
    "proxy-pinnedpubkey",
    "proxy-service-name",
    "proxy-tls13-ciphers",
    "proxy-tlsauthtype",
    "proxy-tlspassword",
    "proxy-tlsuser",
    "proxy-user",
    "proxy1.0",
    "pubkey",
    "quote",
    "random-file",
    "range",
    "rate",
    "request-target",
    "resolve",
    "retry",
    "retry-delay",
    "retry-max-time",
    "sasl-authzid",
    "service-name",
    "sigalgs",
    "socks4",
    "socks4a",
    "socks5",
    "socks5-gssapi-service",
    "socks5-hostname",
    "speed-limit",
    "speed-time",
    "ssl-sessions",
    "stderr",
    "telnet-option",
    "tftp-blksize",
    "time-cond",
    "tls-max",
    "tls13-ciphers",
    "tlsauthtype",
    "tlspassword",
    "tlsuser",
    "trace",
    "trace-ascii",
    "trace-config",
    "unix-socket",
    "upload-flags",
    "variable",
    "vlan-priority",
    "write-out",
];

/// Long boolean options (curl 8.x) that don't affect the request itself.
const FLAG_OPTIONS: &[&str] = &[
    "append",
    "ca-native",
    "cert-status",
    "compressed-ssh",
    "create-dirs",
    "crlf",
    "disable",
    "disable-eprt",
    "disable-epsv",
    "disallow-username-in-url",
    "doh-cert-status",
    "doh-insecure",
    "fail",
    "fail-early",
    "fail-with-body",
    "false-start",
    "follow",
    "form-escape",
    "ftp-create-dirs",
    "ftp-pasv",
    "ftp-pret",
    "ftp-skip-pasv-ip",
    "ftp-ssl-ccc",
    "ftp-ssl-control",
    "globoff",
    "haproxy-protocol",
    "http0.9",
    "http1.0",
    "http1.1",
    "http2",
    "http2-prior-knowledge",
    "http3",
    "http3-only",
    "ignore-content-length",
    "include",
    "insecure",
    "ipv4",
    "ipv6",
    "junk-session-cookies",
    "list-only",
    "location",
    "location-trusted",
    "manual",
    "mptcp",
    "netrc",
    "netrc-optional",
    "no-alpn",
    "no-buffer",
    "no-clobber",
    "no-keepalive",
    "no-npn",
    "no-progress-meter",
    "no-sessionid",
    "out-null",
    "parallel",
    "parallel-immediate",
    "path-as-is",
    "post301",
    "post302",
    "post303",
    "progress-bar",
    "proxy-anyauth",
    "proxy-basic",
    "proxy-ca-native",
    "proxy-digest",
    "proxy-http2",
    "proxy-insecure",
    "proxy-negotiate",
    "proxy-ntlm",
    "proxy-ssl-allow-beast",
    "proxy-ssl-auto-client-cert",
    "proxy-tlsv1",
    "proxytunnel",
    "raw",
    "remote-header-name",
    "remote-name",
    "remote-name-all",
    "remote-time",
    "remove-on-error",
    "retry-all-errors",
    "retry-connrefused",
    "sasl-ir",
    "show-error",
    "show-headers",
    "silent",
    "skip-existing",
    "socks5-basic",
    "socks5-gssapi",
    "socks5-gssapi-nec",
    "ssl",
    "ssl-allow-beast",
    "ssl-auto-client-cert",
    "ssl-no-revoke",
    "ssl-revoke-best-effort",
    "ssl-reqd",
    "sslv2",
    "sslv3",
    "styled-output",
    "suppress-connect-headers",
    "tcp-fastopen",
    "tcp-nodelay",
    "tftp-no-options",
    "tlsv1",
    "tlsv1.0",
    "tlsv1.1",
    "tlsv1.2",
    "tlsv1.3",
    "tr-encoding",
    "trace-ids",
    "trace-time",
    "use-ascii",
    "verbose",
    "version",
    "xattr",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups() {
        assert_eq!(long("data-raw"), Some(Opt::Data(DataKind::Raw)));
        assert_eq!(long("expand-data"), Some(Opt::Data(DataKind::Ascii)));
        assert_eq!(long("proxy"), Some(Opt::Ignored { takes_value: true }));
        assert_eq!(long("insecure"), Some(Opt::Ignored { takes_value: false }));
        assert_eq!(long("made-up"), None);
        assert_eq!(short('x'), Opt::Ignored { takes_value: true });
        assert_eq!(short('k'), Opt::Ignored { takes_value: false });
        assert!(short('d').takes_value());
        assert!(!short('I').takes_value());
    }

    #[test]
    fn tables_have_no_overlap() {
        for name in VALUE_OPTIONS {
            assert!(!FLAG_OPTIONS.contains(name), "{name}");
        }
    }
}
