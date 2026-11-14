import com.sun.net.httpserver.HttpsConfigurator;
import com.sun.net.httpserver.HttpsServer;
import java.io.ByteArrayOutputStream;
import java.io.FileInputStream;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.security.KeyStore;
import java.time.Duration;
import java.util.concurrent.Callable;
import java.util.concurrent.Executors;
import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.SSLContext;

/**
 * Target JVM for the attach test: serves HTTPS itself (an "origin" with a certificate the JVM
 * trusts through javax.net.ssl.trustStore, and an "untrusted" one it doesn't) and keeps calling
 * it with the JVM's default clients, printing one line per request:
 * {@code OK|ERR <kind> <seq> <detail>}.
 *
 * <p>args: origin keystore (PKCS12, password changeit), untrusted keystore, origin host name.
 */
public class AkevJvmTarget {
    public static void main(String[] args) throws Exception {
        HttpsServer origin = server(args[0], "origin");
        HttpsServer untrusted = server(args[1], "untrusted");
        String host = args[2];
        int port = origin.getAddress().getPort();
        int badPort = untrusted.getAddress().getPort();
        // Created before any attach: keeps the JDK's default proxy selector and SSLContext.
        HttpClient early = HttpClient.newHttpClient();
        System.out.println("READY " + port + " " + badPort);
        System.out.flush();
        String base = "https://" + host + ":" + port;
        for (int seq = 0; ; seq++) {
            final int s = seq;
            report("huc", seq, () -> huc(base + "/huc?seq=" + s));
            report("jhc-new", seq, () -> jhc(HttpClient.newHttpClient(), base + "/jhc-new?seq=" + s));
            report("jhc-early", seq, () -> jhc(early, base + "/jhc-early?seq=" + s));
            report("loopback", seq, () -> huc("https://127.0.0.1:" + port + "/loopback?seq=" + s));
            report("untrusted", seq, () -> huc("https://127.0.0.1:" + badPort + "/untrusted?seq=" + s));
            Thread.sleep(100);
        }
    }

    static HttpsServer server(String keystore, String name) throws Exception {
        KeyStore ks = KeyStore.getInstance("PKCS12");
        try (InputStream in = new FileInputStream(keystore)) {
            ks.load(in, "changeit".toCharArray());
        }
        KeyManagerFactory kmf = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm());
        kmf.init(ks, "changeit".toCharArray());
        SSLContext ctx = SSLContext.getInstance("TLS");
        ctx.init(kmf.getKeyManagers(), null, null);
        HttpsServer s = HttpsServer.create(new InetSocketAddress(InetAddress.getLoopbackAddress(), 0), 0);
        s.setHttpsConfigurator(new HttpsConfigurator(ctx));
        s.createContext("/", ex -> {
            byte[] body = (name + " ok " + ex.getRequestURI()).getBytes(StandardCharsets.UTF_8);
            ex.getResponseHeaders().add("Content-Type", "text/plain");
            ex.sendResponseHeaders(200, body.length);
            ex.getResponseBody().write(body);
            ex.close();
        });
        s.setExecutor(Executors.newFixedThreadPool(4));
        s.start();
        return s;
    }

    static String huc(String url) throws Exception {
        HttpURLConnection c = (HttpURLConnection) URI.create(url).toURL().openConnection();
        c.setConnectTimeout(5000);
        c.setReadTimeout(5000);
        int code = c.getResponseCode();
        try (InputStream in = c.getInputStream()) {
            ByteArrayOutputStream buf = new ByteArrayOutputStream();
            byte[] chunk = new byte[4096];
            for (int n; (n = in.read(chunk)) > 0;) {
                buf.write(chunk, 0, n);
            }
            return code + " " + buf.toString("UTF-8");
        }
    }

    static String jhc(HttpClient client, String url) throws Exception {
        HttpRequest req = HttpRequest.newBuilder(URI.create(url)).timeout(Duration.ofSeconds(5)).build();
        HttpResponse<String> res = client.send(req, HttpResponse.BodyHandlers.ofString());
        return res.statusCode() + " " + res.body();
    }

    static void report(String kind, int seq, Callable<String> call) {
        String line;
        try {
            line = "OK " + kind + " " + seq + " " + call.call();
        } catch (Exception e) {
            line = "ERR " + kind + " " + seq + " " + e.getClass().getName() + ": " + e.getMessage();
        }
        System.out.println(line.replace('\n', ' '));
        System.out.flush();
    }
}
