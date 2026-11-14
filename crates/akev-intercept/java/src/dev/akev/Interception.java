package dev.akev;

import java.lang.instrument.Instrumentation;
import java.net.ProxySelector;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.security.NoSuchAlgorithmException;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Properties;
import javax.net.ssl.HttpsURLConnection;
import javax.net.ssl.KeyManager;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocketFactory;
import javax.net.ssl.TrustManager;
import javax.net.ssl.X509ExtendedTrustManager;
import javax.net.ssl.X509TrustManager;

/**
 * Applies and reverts the JVM-wide interception.
 *
 * <p>Applying:
 * <ol>
 *   <li>{@link ProxySelector#setDefault} to an {@link AkevProxySelector} (HTTP, HTTPS and
 *       WebSocket URIs go through Akev, loopback stays direct unless asked otherwise);</li>
 *   <li>a new default {@link SSLContext} (and {@link HttpsURLConnection}'s default socket
 *       factory) whose {@link AkevTrustManager} trusts the Akev CA <em>in addition to</em>
 *       the CAs trusted before, keeping the default context's client certificates;</li>
 *   <li>the same trust manager swapped into the previous default SSLContext object, for
 *       clients that already captured it (best effort, uses JSSE internals);</li>
 *   <li>{@code javax.net.ssl.trustStore*} pointing at a temporary store with the previous
 *       trust store's certificates plus the Akev CA, for libraries that build their own
 *       default trust manager later (OkHttp, Apache HttpClient, Netty...);</li>
 *   <li>the {@code http(s).proxyHost/Port} and {@code http.nonProxyHosts} system properties.
 *       They also re-route clients created <em>before</em> the attach that kept the JDK's
 *       default proxy selector — which is only safe if those clients trust Akev too. In
 *       {@code auto} mode they're therefore set only when step 3 worked and no OkHttp client
 *       (private trust manager) has been loaded.</li>
 * </ol>
 *
 * <p>Every original value is saved; {@code action=restore} (a later attach) puts back each
 * setting that still has the value Akev set — anything the application changed since is left
 * alone. Applying twice first reverts the previous attach.
 */
final class Interception {
    static final String[] OKHTTP_CLASSES = {"okhttp3.OkHttpClient", "com.squareup.okhttp.OkHttpClient"};
    static final String LOOPBACK_PATTERNS = "localhost|*.localhost|127.*|[::1]|0.0.0.0|[::0]";

    private static final Object LOCK = new Object();
    /** Non-null while interception is applied. */
    private static Applied applied;

    private Interception() {}

    static void run(String rawArgs, Instrumentation inst, String entry) {
        Status status = new Status();
        String statusFile = null;
        boolean quiet = false;
        try {
            Properties raw = AgentConfig.load(rawArgs);
            statusFile = AgentConfig.blankToNull(raw.getProperty("statusFile"));
            AgentConfig cfg = AgentConfig.from(raw);
            quiet = cfg.quiet;
            synchronized (LOCK) {
                if (cfg.restore) {
                    restore(status);
                } else {
                    apply(cfg, inst, status);
                }
            }
            status.put("result", "ok");
        } catch (Throwable t) {
            status.put("result", "error");
            status.put("error", Status.describe(t));
            status.put("message", "Agent failed: " + Status.describe(t));
        }
        status.put("entry", entry);
        status.put("agentVersion", AkevAgent.VERSION);
        status.put("javaVersion", System.getProperty("java.version"));
        status.put("javaVendor", System.getProperty("java.vendor"));
        status.put("javaHome", System.getProperty("java.home"));
        if (statusFile != null) {
            try {
                status.writeTo(statusFile);
            } catch (Throwable t) {
                System.err.println("[Akev] couldn't write status file " + statusFile + ": " + Status.describe(t));
            }
        }
        String message = status.get("message");
        if (!quiet && message != null) {
            System.err.println("[Akev] " + message);
        }
    }

    /** Whether interception is currently applied (tests). */
    static boolean isActive() {
        synchronized (LOCK) {
            return applied != null;
        }
    }

    private static void apply(AgentConfig cfg, Instrumentation inst, Status status) throws Exception {
        status.put("action", "apply");
        boolean reapply = applied != null;
        if (reapply) {
            // Start again from the original settings so nothing of the previous attach is
            // mistaken for an original value.
            for (String note : applied.revert()) {
                status.warn(note);
            }
            applied = null;
        }
        String proxy = hostPort(cfg.proxyHost, cfg.proxyPort);

        // ---- Prepare: everything that can fail, before anything changes. ----
        List<X509Certificate> cas = TrustSetup.loadCertificates(cfg.caCert);
        X509ExtendedTrustManager akevTm = TrustSetup.trustManagerFor(cas);
        SSLContext previousContext = SSLContext.getDefault();
        TrustSetup.ContextPatch patch = null;
        String patchError = null;
        try {
            patch = TrustSetup.ContextPatch.prepare(previousContext, inst);
        } catch (Throwable t) {
            patchError = Status.describe(t);
        }
        X509TrustManager primary = patch != null ? patch.original : TrustSetup.defaultTrustManager();
        AkevTrustManager trust = new AkevTrustManager(primary, akevTm, cas);
        KeyManager[] keyManagers = patch != null && patch.keyManager instanceof KeyManager
                ? new KeyManager[] {(KeyManager) patch.keyManager}
                : TrustSetup.defaultKeyManagers(status);
        SSLContext ours = SSLContext.getInstance("TLS");
        ours.init(keyManagers, new TrustManager[] {trust}, null);
        Path trustStore = null;
        String trustStoreError = null;
        try {
            trustStore = TrustSetup.writeCombinedTrustStore(cas);
        } catch (Throwable t) {
            trustStoreError = Status.describe(t);
        }
        List<String> okhttp = TrustSetup.loadedClasses(inst, OKHTTP_CLASSES);
        boolean setProps;
        String propsReason;
        if (cfg.proxyProperties.equals(AgentConfig.PROPS_ALWAYS)) {
            setProps = true;
            propsReason = "always";
        } else if (cfg.proxyProperties.equals(AgentConfig.PROPS_NEVER)) {
            setProps = false;
            propsReason = "disabled (proxyProperties=never)";
        } else if (patch == null) {
            setProps = false;
            propsReason = "the existing default SSLContext couldn't be updated";
        } else if (!okhttp.isEmpty()) {
            setProps = false;
            propsReason = okhttp.get(0) + " is loaded";
        } else {
            setProps = true;
            propsReason = "auto";
        }

        // ---- Commit. ----
        Applied a = new Applied();
        try {
            if (patch != null) {
                patch.install(trust);
                a.patch = patch;
                a.trust = trust;
            }
            a.previousSelector = ProxySelector.getDefault();
            a.selector = new AkevProxySelector(a.previousSelector, cfg.proxyHost, cfg.proxyPort,
                    cfg.interceptLoopback);
            ProxySelector.setDefault(a.selector);
            a.previousContext = previousContext;
            SSLContext.setDefault(ours);
            a.context = ours;
            a.previousFactory = HttpsURLConnection.getDefaultSSLSocketFactory();
            a.factory = ours.getSocketFactory();
            HttpsURLConnection.setDefaultSSLSocketFactory(a.factory);
            if (trustStore != null) {
                a.trustStoreFile = trustStore;
                a.setProperty("javax.net.ssl.trustStore", trustStore.toString());
                a.setProperty("javax.net.ssl.trustStorePassword", TrustSetup.STORE_PASSWORD);
                a.setProperty("javax.net.ssl.trustStoreType", KeyStore.getDefaultType());
                a.setProperty("javax.net.ssl.trustStoreProvider", null);
            }
            if (setProps) {
                String port = String.valueOf(cfg.proxyPort);
                a.setProperty("http.proxyHost", cfg.proxyHost);
                a.setProperty("http.proxyPort", port);
                a.setProperty("https.proxyHost", cfg.proxyHost);
                a.setProperty("https.proxyPort", port);
                a.setProperty("http.nonProxyHosts", cfg.interceptLoopback ? "" : LOOPBACK_PATTERNS);
            }
            applied = a;
        } catch (Throwable t) {
            a.revert();
            throw t;
        }

        status.put("proxy", proxy);
        status.put("interceptLoopback", String.valueOf(cfg.interceptLoopback));
        status.put("proxySelector", "installed");
        status.put("defaultSslContext", "installed");
        status.put("httpsUrlConnection", "installed");
        status.put("existingDefaultContext", patch != null ? "patched" : "unchanged: " + patchError);
        status.put("trustStore", trustStore != null ? trustStore.toString() : "unchanged: " + trustStoreError);
        status.put("proxyProperties", setProps ? "set" : "skipped: " + propsReason);
        if (!okhttp.isEmpty()) {
            status.put("okhttp", join(okhttp));
        }
        if (patch == null) {
            status.warn("Clients created before attaching that use the old default SSLContext don't trust Akev ("
                    + patchError + ").");
        }
        if (!setProps) {
            status.warn("Clients created before attaching that use the JDK's built-in proxy selector keep connecting "
                    + "directly (" + propsReason + "); clients created from now on are intercepted.");
        }
        if (trustStore == null) {
            status.warn("Libraries that build their own trust manager won't trust Akev: " + trustStoreError);
        }
        status.put("message", (reapply ? "Re-attached: " : "") + "HTTP(S) traffic of this JVM now goes through Akev at "
                + proxy + (cfg.interceptLoopback ? " (including localhost)" : ""));
    }

    private static void restore(Status status) throws Exception {
        status.put("action", "restore");
        if (applied == null) {
            status.put("restored", "false");
            status.put("message", "Interception was not active in this JVM; nothing to restore");
            return;
        }
        List<String> notes = applied.revert();
        applied = null;
        for (String note : notes) {
            status.warn(note);
        }
        status.put("restored", "true");
        status.put("message", "Interception removed: original proxy and TLS settings restored");
    }

    static String hostPort(String host, int port) {
        return (host.indexOf(':') >= 0 ? "[" + host + "]" : host) + ":" + port;
    }

    private static String join(List<String> items) {
        StringBuilder sb = new StringBuilder();
        for (String s : items) {
            if (sb.length() > 0) {
                sb.append(',');
            }
            sb.append(s);
        }
        return sb.toString();
    }

    /** What one attach changed, with the values to put back. */
    static final class Applied {
        final Map<String, String> originalProps = new LinkedHashMap<String, String>();
        final Map<String, String> ourProps = new LinkedHashMap<String, String>();
        AkevProxySelector selector;
        ProxySelector previousSelector;
        SSLContext context;
        SSLContext previousContext;
        SSLSocketFactory factory;
        SSLSocketFactory previousFactory;
        TrustSetup.ContextPatch patch;
        AkevTrustManager trust;
        Path trustStoreFile;

        void setProperty(String key, String value) {
            if (!originalProps.containsKey(key)) {
                originalProps.put(key, System.getProperty(key));
            }
            ourProps.put(key, value);
            if (value == null) {
                System.clearProperty(key);
            } else {
                System.setProperty(key, value);
            }
        }

        /**
         * Undo everything still in the state Akev left it. Never throws; returns notes about
         * settings the application changed in the meantime (left as they are).
         */
        List<String> revert() {
            List<String> notes = new ArrayList<String>();
            for (Map.Entry<String, String> e : ourProps.entrySet()) {
                String key = e.getKey();
                String now = System.getProperty(key);
                if (!same(now, e.getValue())) {
                    notes.add("System property " + key + " was changed by the application; left as is.");
                    continue;
                }
                String original = originalProps.get(key);
                if (original == null) {
                    System.clearProperty(key);
                } else {
                    System.setProperty(key, original);
                }
            }
            if (factory != null) {
                if (HttpsURLConnection.getDefaultSSLSocketFactory() == factory && previousFactory != null) {
                    HttpsURLConnection.setDefaultSSLSocketFactory(previousFactory);
                } else {
                    notes.add("HttpsURLConnection's default SSLSocketFactory was replaced by the application; left as is.");
                }
            }
            if (context != null) {
                try {
                    if (SSLContext.getDefault() == context) {
                        SSLContext.setDefault(previousContext);
                    } else {
                        notes.add("The default SSLContext was replaced by the application; left as is.");
                    }
                } catch (NoSuchAlgorithmException e) {
                    notes.add("Couldn't restore the default SSLContext: " + Status.describe(e));
                }
            }
            if (selector != null) {
                if (ProxySelector.getDefault() == selector) {
                    ProxySelector.setDefault(previousSelector);
                } else {
                    notes.add("The default ProxySelector was replaced by the application; left as is.");
                }
            }
            if (patch != null) {
                try {
                    if (!patch.revert(trust)) {
                        notes.add("The original SSLContext's trust manager was replaced by the application; left as is.");
                    }
                } catch (Throwable t) {
                    notes.add("Couldn't restore the original SSLContext's trust manager: " + Status.describe(t));
                }
            }
            if (trustStoreFile != null) {
                try {
                    Files.deleteIfExists(trustStoreFile);
                } catch (Throwable ignored) {
                    // A temporary file; deleteOnExit is the fallback.
                }
            }
            return notes;
        }

        private static boolean same(String a, String b) {
            return a == null ? b == null : a.equals(b);
        }
    }
}
