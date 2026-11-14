package dev.akev;

import com.sun.net.httpserver.HttpsConfigurator;
import com.sun.net.httpserver.HttpsServer;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.io.PrintStream;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Proxy;
import java.net.ProxySelector;
import java.net.SocketAddress;
import java.net.URI;
import java.net.URL;
import java.nio.file.Files;
import java.security.KeyStore;
import java.security.cert.Certificate;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.Properties;
import javax.net.ssl.HttpsURLConnection;
import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocketFactory;
import javax.net.ssl.X509ExtendedTrustManager;
import javax.net.ssl.X509TrustManager;

/**
 * Self-test of the agent's classes, run by {@code tests/java_agent.rs} (which generates the
 * certificates with keytool). No test framework: prints {@code PASS <name>} per check and
 * {@code ALL OK <n>} at the end, exits 1 on the first failure.
 *
 * <p>args: {@code <cert dir> patched|unpatched} — whether the JVM was started with
 * {@code --add-opens} for JSSE (so the in-place update of the existing default SSLContext works
 * without an {@code Instrumentation}). Before Java 16 reflection into JSSE works anyway (Java 8:
 * no modules; 9–15: illegal access permitted by default), so it's always patched there.
 */
public final class AgentSelfTest {
    private static int passed;
    private static File dir;
    private static boolean patched;

    public static void main(String[] args) throws Exception {
        dir = new File(args[0]);
        patched = args[1].equals("patched") || javaMajor() < 16;
        // No kept-alive connections between checks (they'd skip the TLS handshake).
        System.setProperty("http.keepAlive", "false");
        // The JVM's default trust: only the "other" certificate (like a corporate root).
        File trust = new File(dir, "default-trust.p12");
        KeyStore ts = KeyStore.getInstance("PKCS12");
        ts.load(null, null);
        ts.setCertificateEntry("other", chain("other.p12", "other")[0]);
        OutputStream os = new FileOutputStream(trust);
        ts.store(os, "changeit".toCharArray());
        os.close();
        System.setProperty("javax.net.ssl.trustStore", trust.getPath());
        System.setProperty("javax.net.ssl.trustStorePassword", "changeit");
        System.setProperty("javax.net.ssl.trustStoreType", "PKCS12");

        config();
        proxySelector();
        trustManager();
        interception();
        attachCli();
        System.out.println("ALL OK " + passed);
    }

    // ------------------------------------------------------------------------------------

    static void config() throws Exception {
        AgentConfig c = AgentConfig.parse("proxyHost=127.0.0.1&proxyPort=8000&caCert=%2Ftmp%2Fmy%20ca.pem&interceptLoopback=true&quiet");
        check("query config", c.proxyHost.equals("127.0.0.1") && c.proxyPort == 8000
                && c.caCert.equals("/tmp/my ca.pem") && c.interceptLoopback && c.quiet && !c.restore
                && c.proxyProperties.equals("auto"));
        File f = new File(dir, "args.properties");
        Properties p = new Properties();
        p.setProperty("proxyHost", "[::1]");
        p.setProperty("proxyPort", "9000");
        p.setProperty("caCert", "C:\\Users\\me\\ca.pem");
        p.setProperty("proxyProperties", "NEVER");
        p.setProperty("statusFile", "/tmp/s.txt");
        OutputStream os = new FileOutputStream(f);
        p.store(os, null);
        os.close();
        c = AgentConfig.parse("@" + f.getPath());
        check("file config", c.proxyHost.equals("::1") && c.proxyPort == 9000 && c.caCert.equals("C:\\Users\\me\\ca.pem")
                && c.proxyProperties.equals("never") && c.statusFile.equals("/tmp/s.txt") && !c.interceptLoopback);
        check("restore config", AgentConfig.parse("action=restore").restore && AgentConfig.parse("restore=true").restore
                && AgentConfig.parse("action=RESTORE&statusFile=x").statusFile.equals("x"));
        for (String bad : new String[] {"", "proxyHost=h&caCert=c", "proxyHost=h&proxyPort=0&caCert=c",
                "proxyHost=h&proxyPort=70000&caCert=c", "proxyHost=h&proxyPort=x&caCert=c", "proxyHost=h&proxyPort=1",
                "proxyHost=h&proxyPort=1&caCert=c&proxyProperties=maybe", "action=detonate"}) {
            boolean threw = false;
            try {
                AgentConfig.parse(bad);
            } catch (IllegalArgumentException e) {
                threw = true;
            }
            check("rejects '" + bad + "'", threw);
        }
        check("malformed escapes kept", AgentConfig.load("a=%zz").getProperty("a").equals("%zz"));
    }

    static void proxySelector() throws Exception {
        final List<URI> delegated = new ArrayList<URI>();
        final List<SocketAddress> failed = new ArrayList<SocketAddress>();
        final Proxy socks = new Proxy(Proxy.Type.SOCKS, InetSocketAddress.createUnresolved("socks", 1080));
        ProxySelector previous = new ProxySelector() {
            @Override
            public List<Proxy> select(URI uri) {
                delegated.add(uri);
                return Collections.singletonList(socks);
            }

            @Override
            public void connectFailed(URI uri, SocketAddress sa, IOException ioe) {
                failed.add(sa);
            }
        };
        AkevProxySelector sel = new AkevProxySelector(previous, "127.0.0.1", 8000, false);
        Proxy akev = sel.proxy();
        for (String u : new String[] {"http://example.com/", "https://api.example.com:8443/x", "ws://example.com/s",
                "WSS://Example.com/", "http://10.0.0.1/", "http://128.0.0.1/", "https://[2001:db8::1]/",
                "http://localhost.example.com/", "http://under_score.example:8080/", "http://my-laptop/"}) {
            check("proxied " + u, sel.select(URI.create(u)).equals(Collections.singletonList(akev)));
        }
        for (String u : new String[] {"http://localhost/", "https://LOCALHOST:8443/", "http://app.localhost/",
                "http://127.0.0.1:3000/", "http://127.1.2.3/", "https://[::1]:8443/", "http://0.0.0.0/",
                "http://[::ffff:127.0.0.1]/", "http://localhost./"}) {
            check("direct " + u, sel.select(URI.create(u)).equals(Collections.singletonList(Proxy.NO_PROXY)));
        }
        AkevProxySelector all = new AkevProxySelector(previous, "127.0.0.1", 8000, true);
        check("interceptLoopback", all.select(URI.create("http://localhost:3000/")).get(0).equals(all.proxy()));
        check("other schemes delegated", sel.select(URI.create("socket://db.example:5432")).get(0) == socks
                && delegated.size() == 1);
        AkevProxySelector orphan = new AkevProxySelector(null, "127.0.0.1", 8000, false);
        check("no previous → direct", orphan.select(URI.create("ftp://x/")).get(0) == Proxy.NO_PROXY);
        sel.connectFailed(URI.create("http://example.com/"), akev.address(), new IOException("x"));
        sel.connectFailed(URI.create("http://example.com/"), new InetSocketAddress("10.0.0.9", 3128), new IOException("x"));
        check("connectFailed only forwarded for other proxies", failed.size() == 1);
        boolean threw = false;
        try {
            sel.select(null);
        } catch (IllegalArgumentException e) {
            threw = true;
        }
        check("null uri rejected", threw);
        check("names are never resolved", !AkevProxySelector.isLoopback("localhost6")
                && !AkevProxySelector.isLoopback("::not-an-ip") && !AkevProxySelector.isLoopback("999.0.0.1")
                && AkevProxySelector.isLoopback("[::]"));
    }

    static void trustManager() throws Exception {
        X509Certificate[] akevLeaf = chain("akev-leaf.p12", "leaf");
        X509Certificate[] other = chain("other.p12", "other");
        X509Certificate[] untrusted = chain("untrusted.p12", "untrusted");
        List<X509Certificate> cas = TrustSetup.loadCertificates(new File(dir, "akev-ca.pem").getPath());
        check("loads the CA PEM", cas.size() == 1 && akevLeaf.length == 2);
        X509TrustManager primary = TrustSetup.defaultTrustManager();
        X509ExtendedTrustManager akevOnly = TrustSetup.trustManagerFor(cas);
        AkevTrustManager tm = new AkevTrustManager(primary, akevOnly, cas);
        String auth = "ECDHE_ECDSA";
        check("Akev-issued server accepted", accepts(tm, akevLeaf, auth, true));
        check("originally trusted server accepted", accepts(tm, other, auth, true));
        check("untrusted server rejected", !accepts(tm, untrusted, auth, true));
        check("Akev leaf without its CA in the chain accepted", accepts(tm, new X509Certificate[] {akevLeaf[0]}, auth, true));
        try {
            tm.checkServerTrusted(untrusted, auth);
        } catch (CertificateException e) {
            check("both failures reported", e.getSuppressed().length == 1);
        }
        check("original alone rejects Akev", !accepts(primary, akevLeaf, auth, true));
        check("client certs: Akev not trusted", !accepts(tm, akevLeaf, auth, false));
        check("client certs: original trust kept", accepts(tm, other, auth, false));
        List<X509Certificate> issuers = Arrays.asList(tm.getAcceptedIssuers());
        check("accepted issuers = original + Akev", issuers.contains(cas.get(0)) && issuers.contains(other[0])
                && issuers.size() == primary.getAcceptedIssuers().length + 1);
    }

    static void interception() throws Exception {
        // Every check of the Akev-issued certificate uses a new server (new port): a cached TLS
        // session or kept-alive connection would otherwise skip certificate validation.
        HttpsServer otherServer = server("other.p12", "other", "other");
        HttpsServer badServer = server("untrusted.p12", "untrusted", "untrusted");
        String otherUrl = "https://127.0.0.1:" + otherServer.getAddress().getPort() + "/";
        String badUrl = "https://127.0.0.1:" + badServer.getAddress().getPort() + "/";

        ProxySelector selector0 = ProxySelector.getDefault();
        // Original proxy properties: some JDKs preset them (macOS: http.nonProxyHosts =
        // "local|*.local|169.254/16|*.169.254/16"), and restore must bring those values back.
        String httpsProxyHost0 = System.getProperty("https.proxyHost");
        String nonProxyHosts0 = System.getProperty("http.nonProxyHosts");
        SSLContext context0 = SSLContext.getDefault();
        SSLSocketFactory factory0 = HttpsURLConnection.getDefaultSSLSocketFactory();
        check("before: Akev-issued rejected", akev(null) == null);
        check("before: default trust works", "other".equals(get(otherUrl, null)));
        check("before: untrusted rejected", get(badUrl, null) == null);

        File status = new File(dir, "status-" + (patched ? "p" : "u") + ".txt");
        String ca = new File(dir, "akev-ca.pem").getPath();
        String args = "proxyHost=127.0.0.1&proxyPort=1&quiet=true&caCert=" + enc(ca) + "&statusFile=" + enc(status.getPath());
        Interception.run(args, null, "test");
        Properties s = read(status);
        check("apply ok", "ok".equals(s.getProperty("result")) && Interception.isActive());
        check("selector installed", ProxySelector.getDefault() instanceof AkevProxySelector
                && ((AkevProxySelector) ProxySelector.getDefault()).previous == selector0);
        check("default context replaced", SSLContext.getDefault() != context0);
        check("HttpsURLConnection factory replaced", HttpsURLConnection.getDefaultSSLSocketFactory() != factory0);
        String store = System.getProperty("javax.net.ssl.trustStore");
        check("combined trust store", store.equals(s.getProperty("trustStore")) && new File(store).isFile()
                && storeHas(store, "akev-ca") && storeHas(store, "other"));
        check("patch status", s.getProperty("existingDefaultContext").startsWith(patched ? "patched" : "unchanged"));
        if (patched) {
            check("proxy properties set (auto)", "set".equals(s.getProperty("proxyProperties"))
                    && "127.0.0.1".equals(System.getProperty("https.proxyHost"))
                    && "1".equals(System.getProperty("http.proxyPort"))
                    && System.getProperty("http.nonProxyHosts").contains("127.*"));
            check("old default context trusts Akev", "akev".equals(akev(context0.getSocketFactory())));
        } else {
            check("proxy properties skipped (auto)", s.getProperty("proxyProperties").startsWith("skipped")
                    && System.getProperty("https.proxyHost") == null && s.getProperty("warning") != null);
            check("old default context unchanged", akev(context0.getSocketFactory()) == null);
        }
        // Loopback stays direct, so these talk to the servers themselves.
        check("attached: Akev-issued accepted", "akev".equals(akev(null)));
        check("attached: default trust still works", "other".equals(get(otherUrl, null)));
        check("attached: untrusted still rejected", get(badUrl, null) == null);
        check("attached: new default trust managers trust Akev",
                accepts(TrustSetup.defaultTrustManager(), chain("akev-leaf.p12", "leaf"), "ECDHE_ECDSA", true));

        // Attaching again re-applies cleanly; one restore undoes everything.
        Interception.run(args, null, "test");
        s = read(status);
        check("re-apply", "ok".equals(s.getProperty("result")) && s.getProperty("message").startsWith("Re-attached")
                && !new File(store).exists());
        String store2 = System.getProperty("javax.net.ssl.trustStore");
        Interception.run("action=restore&quiet=true&statusFile=" + enc(status.getPath()), null, "test");
        s = read(status);
        check("restore ok", "ok".equals(s.getProperty("result")) && "true".equals(s.getProperty("restored"))
                && !Interception.isActive());
        check("selector restored", ProxySelector.getDefault() == selector0);
        check("context restored", SSLContext.getDefault() == context0);
        check("factory restored", HttpsURLConnection.getDefaultSSLSocketFactory() == factory0);
        check("properties restored", same(System.getProperty("https.proxyHost"), httpsProxyHost0)
                && same(System.getProperty("http.nonProxyHosts"), nonProxyHosts0)
                && System.getProperty("javax.net.ssl.trustStore").endsWith("default-trust.p12")
                && "PKCS12".equals(System.getProperty("javax.net.ssl.trustStoreType")));
        check("temporary store deleted", !new File(store2).exists());
        check("after: Akev-issued rejected", akev(null) == null);
        check("after: old default context back to normal", akev(context0.getSocketFactory()) == null);
        check("after: default trust works", "other".equals(get(otherUrl, null)));

        Interception.run("action=restore&quiet=true&statusFile=" + enc(status.getPath()), null, "test");
        check("restore twice is harmless", "false".equals(read(status).getProperty("restored")));

        // Settings the app changes while attached are left alone on restore.
        Interception.run(args, null, "test");
        ProxySelector mine = new AkevProxySelector(null, "10.1.1.1", 3128, true);
        ProxySelector.setDefault(mine);
        System.setProperty("https.proxyHost", "corp-proxy");
        Interception.run("action=restore&quiet=true&statusFile=" + enc(status.getPath()), null, "test");
        s = read(status);
        check("app changes kept", ProxySelector.getDefault() == mine && "true".equals(s.getProperty("restored"))
                && "corp-proxy".equals(System.getProperty("https.proxyHost")) && s.getProperty("warning") != null);
        ProxySelector.setDefault(selector0);
        System.clearProperty("https.proxyHost");

        // A broken configuration changes nothing.
        Interception.run("proxyHost=127.0.0.1&proxyPort=1&quiet=true&caCert=%2Fnonexistent.pem&statusFile="
                + enc(status.getPath()), null, "test");
        s = read(status);
        check("bad CA reported", "error".equals(s.getProperty("result")) && s.getProperty("error").contains("nonexistent")
                && !Interception.isActive() && ProxySelector.getDefault() == selector0
                && SSLContext.getDefault() == context0);
        Interception.run("proxyHost=127.0.0.1&quiet=true&statusFile=" + enc(status.getPath()), null, "test");
        check("bad args reported", "error".equals(read(status).getProperty("result")));

        otherServer.stop(0);
        badServer.stop(0);
    }

    /** GET from a new server presenting the Akev-issued certificate. */
    static String akev(SSLSocketFactory factory) throws Exception {
        HttpsServer s = server("akev-leaf.p12", "leaf", "akev");
        try {
            return get("https://127.0.0.1:" + s.getAddress().getPort() + "/", factory);
        } finally {
            s.stop(0);
        }
    }

    static void attachCli() throws Exception {
        PrintStream quiet = new PrintStream(new ByteArrayOutputStream());
        check("cli: no args", Attach.run(new String[0], quiet, quiet) == Attach.USAGE);
        check("cli: bad pid", Attach.run(new String[] {"abc", "--restore"}, quiet, quiet) == Attach.USAGE);
        check("cli: missing --ca", Attach.run(new String[] {"1", "--proxy", "127.0.0.1:8000"}, quiet, quiet) == Attach.USAGE);
        check("cli: bad --proxy", Attach.run(new String[] {"1", "--proxy", "nohost", "--ca", "x"}, quiet, quiet) == Attach.USAGE);
        check("cli: unknown option", Attach.run(new String[] {"1", "--frobnicate"}, quiet, quiet) == Attach.USAGE);
        ByteArrayOutputStream buf = new ByteArrayOutputStream();
        int code = Attach.run(new String[] {"--check"}, new PrintStream(buf), quiet);
        check("cli: --check", (code == Attach.OK && buf.toString().contains("attach=available"))
                || (code == Attach.NO_ATTACH_API && buf.toString().contains("attach=unavailable")));
    }

    // ------------------------------------------------------------------------------------

    static int javaMajor() {
        String v = System.getProperty("java.specification.version");
        return Integer.parseInt(v.startsWith("1.") ? v.substring(2) : v);
    }

    static void check(String name, boolean ok) {
        if (!ok) {
            System.out.println("FAIL " + name);
            System.exit(1);
        }
        passed++;
        System.out.println("PASS " + name);
    }

    static boolean accepts(X509TrustManager tm, X509Certificate[] chain, String auth, boolean server) {
        try {
            if (server) {
                tm.checkServerTrusted(chain, auth);
            } else {
                tm.checkClientTrusted(chain, auth);
            }
            return true;
        } catch (CertificateException e) {
            return false;
        }
    }

    static X509Certificate[] chain(String file, String alias) throws Exception {
        KeyStore ks = KeyStore.getInstance("PKCS12");
        InputStream in = new FileInputStream(new File(dir, file));
        ks.load(in, "changeit".toCharArray());
        in.close();
        Certificate[] c = ks.getCertificateChain(alias);
        X509Certificate[] out = new X509Certificate[c.length];
        for (int i = 0; i < c.length; i++) {
            out[i] = (X509Certificate) c[i];
        }
        return out;
    }

    static boolean storeHas(String path, String alias) throws Exception {
        KeyStore ks = KeyStore.getInstance(KeyStore.getDefaultType());
        InputStream in = new FileInputStream(path);
        ks.load(in, TrustSetup.STORE_PASSWORD.toCharArray());
        in.close();
        return ks.isCertificateEntry(alias);
    }

    static HttpsServer server(String file, String alias, final String name) throws Exception {
        KeyStore ks = KeyStore.getInstance("PKCS12");
        InputStream in = new FileInputStream(new File(dir, file));
        ks.load(in, "changeit".toCharArray());
        in.close();
        KeyManagerFactory kmf = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm());
        kmf.init(ks, "changeit".toCharArray());
        SSLContext ctx = SSLContext.getInstance("TLS");
        ctx.init(kmf.getKeyManagers(), null, null);
        HttpsServer s = HttpsServer.create(new InetSocketAddress(InetAddress.getByName("127.0.0.1"), 0), 0);
        s.setHttpsConfigurator(new HttpsConfigurator(ctx));
        s.createContext("/", ex -> {
            byte[] body = name.getBytes("UTF-8");
            ex.sendResponseHeaders(200, body.length);
            ex.getResponseBody().write(body);
            ex.close();
        });
        s.start();
        return s;
    }

    /** GET over HTTPS; the body, or null when the TLS handshake (or anything) fails. */
    static String get(String url, SSLSocketFactory factory) {
        try {
            HttpsURLConnection c = (HttpsURLConnection) new URL(url).openConnection();
            if (factory != null) {
                c.setSSLSocketFactory(factory);
            }
            c.setConnectTimeout(5000);
            c.setReadTimeout(5000);
            InputStream in = c.getInputStream();
            ByteArrayOutputStream buf = new ByteArrayOutputStream();
            byte[] b = new byte[1024];
            for (int n; (n = in.read(b)) > 0;) {
                buf.write(b, 0, n);
            }
            in.close();
            return buf.toString("UTF-8");
        } catch (IOException e) {
            return null;
        }
    }

    static Properties read(File f) throws IOException {
        Properties p = new Properties();
        for (String line : new String(Files.readAllBytes(f.toPath()), "UTF-8").split("\n")) {
            int eq = line.indexOf('=');
            if (eq > 0) {
                p.setProperty(line.substring(0, eq), line.substring(eq + 1));
            }
        }
        return p;
    }

    static String enc(String s) throws Exception {
        return java.net.URLEncoder.encode(s, "UTF-8");
    }

    /** Null-safe equality (the self-test stays Java 8 compatible, so no Objects.equals). */
    static boolean same(String a, String b) {
        return a == null ? b == null : a.equals(b);
    }
}
