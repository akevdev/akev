package dev.akev;

import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.UnsupportedEncodingException;
import java.net.URLDecoder;
import java.util.Locale;
import java.util.Properties;

/**
 * Agent arguments.
 *
 * <p>Keys: {@code action} ({@code apply} (default) or {@code restore}), {@code proxyHost},
 * {@code proxyPort}, {@code caCert} (PEM file with the Akev CA), {@code interceptLoopback}
 * ({@code true} also proxies {@code localhost}/{@code 127.0.0.1}/{@code ::1}),
 * {@code proxyProperties} ({@code auto}, {@code always} or {@code never}: whether to set the
 * {@code http(s).proxyHost/Port} system properties, see {@link Interception}),
 * {@code statusFile} (where to write the outcome) and {@code quiet} (no console message).
 */
final class AgentConfig {
    static final String PROPS_AUTO = "auto";
    static final String PROPS_ALWAYS = "always";
    static final String PROPS_NEVER = "never";

    boolean restore;
    String proxyHost;
    int proxyPort;
    String caCert;
    boolean interceptLoopback;
    String proxyProperties = PROPS_AUTO;
    String statusFile;
    boolean quiet;

    /** Raw key/values from {@code @file} or a query string (never fails on content). */
    static Properties load(String raw) throws IOException {
        Properties p = new Properties();
        String s = raw == null ? "" : raw.trim();
        if (s.startsWith("@")) {
            InputStream in = new FileInputStream(s.substring(1));
            try {
                p.load(in);
            } finally {
                in.close();
            }
            return p;
        }
        for (String pair : s.split("&")) {
            if (pair.isEmpty()) {
                continue;
            }
            int eq = pair.indexOf('=');
            String key = decode(eq < 0 ? pair : pair.substring(0, eq));
            String value = eq < 0 ? "true" : decode(pair.substring(eq + 1));
            p.setProperty(key.trim(), value);
        }
        return p;
    }

    /** Validate key/values. Throws {@link IllegalArgumentException} with a readable message. */
    static AgentConfig from(Properties p) {
        AgentConfig c = new AgentConfig();
        c.statusFile = blankToNull(p.getProperty("statusFile"));
        c.quiet = bool(p, "quiet", false);
        String action = p.getProperty("action", "apply").trim().toLowerCase(Locale.ROOT);
        if (bool(p, "restore", false)) {
            action = "restore";
        }
        if (action.equals("restore")) {
            c.restore = true;
            return c;
        }
        if (!action.equals("apply")) {
            throw new IllegalArgumentException("unknown action '" + action + "' (expected apply or restore)");
        }
        c.proxyHost = required(p, "proxyHost");
        if (c.proxyHost.startsWith("[") && c.proxyHost.endsWith("]")) {
            c.proxyHost = c.proxyHost.substring(1, c.proxyHost.length() - 1);
        }
        String port = required(p, "proxyPort");
        try {
            c.proxyPort = Integer.parseInt(port);
        } catch (NumberFormatException e) {
            c.proxyPort = -1;
        }
        if (c.proxyPort < 1 || c.proxyPort > 65535) {
            throw new IllegalArgumentException("invalid proxyPort '" + port + "'");
        }
        c.caCert = required(p, "caCert");
        c.interceptLoopback = bool(p, "interceptLoopback", false);
        String props = p.getProperty("proxyProperties", PROPS_AUTO).trim().toLowerCase(Locale.ROOT);
        if (!props.equals(PROPS_AUTO) && !props.equals(PROPS_ALWAYS) && !props.equals(PROPS_NEVER)) {
            throw new IllegalArgumentException("invalid proxyProperties '" + props + "' (expected auto, always or never)");
        }
        c.proxyProperties = props;
        return c;
    }

    static AgentConfig parse(String raw) throws IOException {
        return from(load(raw));
    }

    private static String required(Properties p, String key) {
        String v = blankToNull(p.getProperty(key));
        if (v == null) {
            throw new IllegalArgumentException("missing " + key);
        }
        return v;
    }

    private static boolean bool(Properties p, String key, boolean dflt) {
        String v = blankToNull(p.getProperty(key));
        if (v == null) {
            return dflt;
        }
        v = v.toLowerCase(Locale.ROOT);
        return v.equals("true") || v.equals("1") || v.equals("yes") || v.equals("on");
    }

    static String blankToNull(String s) {
        if (s == null) {
            return null;
        }
        String t = s.trim();
        return t.isEmpty() ? null : t;
    }

    private static String decode(String s) {
        try {
            return URLDecoder.decode(s, "UTF-8");
        } catch (UnsupportedEncodingException e) {
            throw new IllegalStateException(e);
        } catch (IllegalArgumentException e) {
            // Malformed %-escape: take it literally rather than failing the whole attach.
            return s;
        }
    }
}
