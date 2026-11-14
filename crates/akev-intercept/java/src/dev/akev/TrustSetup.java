package dev.akev;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.lang.instrument.Instrumentation;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.GeneralSecurityException;
import java.security.KeyStore;
import java.security.cert.Certificate;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Collections;
import java.util.Enumeration;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import javax.net.ssl.KeyManager;
import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.SSLContext;
import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import javax.net.ssl.X509ExtendedTrustManager;
import javax.net.ssl.X509TrustManager;

/** Certificates, key/trust stores and the in-place update of the existing default SSLContext. */
final class TrustSetup {
    /** Password of the combined trust store the agent writes (it only holds public certificates). */
    static final String STORE_PASSWORD = "changeit";

    private TrustSetup() {}

    /** Every certificate in a PEM (or DER) file. */
    static List<X509Certificate> loadCertificates(String path) throws IOException, GeneralSecurityException {
        CertificateFactory cf = CertificateFactory.getInstance("X.509");
        InputStream in = new FileInputStream(path);
        Collection<? extends Certificate> certs;
        try {
            certs = cf.generateCertificates(in);
        } finally {
            in.close();
        }
        List<X509Certificate> out = new ArrayList<X509Certificate>();
        for (Certificate c : certs) {
            if (c instanceof X509Certificate) {
                out.add((X509Certificate) c);
            }
        }
        if (out.isEmpty()) {
            throw new GeneralSecurityException("no certificate in " + path);
        }
        return out;
    }

    /** A trust manager that trusts exactly these CAs (full PKIX validation). */
    static X509ExtendedTrustManager trustManagerFor(List<X509Certificate> cas) throws IOException,
            GeneralSecurityException {
        KeyStore ks = KeyStore.getInstance(KeyStore.getDefaultType());
        ks.load(null, null);
        int i = 0;
        for (X509Certificate ca : cas) {
            ks.setCertificateEntry("akev-ca-" + i++, ca);
        }
        TrustManagerFactory tmf = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
        tmf.init(ks);
        for (TrustManager tm : tmf.getTrustManagers()) {
            if (tm instanceof X509ExtendedTrustManager) {
                return (X509ExtendedTrustManager) tm;
            }
        }
        throw new GeneralSecurityException("no X509ExtendedTrustManager from " + tmf.getAlgorithm());
    }

    /** The trust manager a default-configured {@code TrustManagerFactory} gives right now. */
    static X509TrustManager defaultTrustManager() throws GeneralSecurityException {
        TrustManagerFactory tmf = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
        tmf.init((KeyStore) null);
        for (TrustManager tm : tmf.getTrustManagers()) {
            if (tm instanceof X509TrustManager) {
                return (X509TrustManager) tm;
            }
        }
        throw new GeneralSecurityException("no X509TrustManager from " + tmf.getAlgorithm());
    }

    /**
     * Key managers like the JDK's default SSLContext builds them from the
     * {@code javax.net.ssl.keyStore*} properties (client certificates), or null for none.
     */
    static KeyManager[] defaultKeyManagers(Status status) {
        String path = System.getProperty("javax.net.ssl.keyStore", "");
        String type = System.getProperty("javax.net.ssl.keyStoreType", KeyStore.getDefaultType());
        String provider = System.getProperty("javax.net.ssl.keyStoreProvider", "");
        String password = System.getProperty("javax.net.ssl.keyStorePassword", "");
        if (path.isEmpty()) {
            return null;
        }
        try {
            KeyStore ks = provider.isEmpty() ? KeyStore.getInstance(type) : KeyStore.getInstance(type, provider);
            char[] pw = password.isEmpty() ? null : password.toCharArray();
            if ("NONE".equals(path)) {
                ks.load(null, pw);
            } else {
                InputStream in = new FileInputStream(path);
                try {
                    ks.load(in, pw);
                } finally {
                    in.close();
                }
            }
            KeyManagerFactory kmf = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm());
            kmf.init(ks, pw);
            return kmf.getKeyManagers();
        } catch (Exception e) {
            status.warn("client certificates from javax.net.ssl.keyStore are not used by the new default SSLContext: "
                    + Status.describe(e));
            return null;
        }
    }

    /**
     * Write a trust store holding everything the JVM's default trust store holds right now plus
     * the Akev CA, for code that builds its own {@code TrustManagerFactory} with default
     * settings later (OkHttp, Apache HttpClient, Netty...). Mirrors how the JDK picks its trust
     * store: {@code javax.net.ssl.trustStore}, else {@code jssecacerts}, else {@code cacerts}.
     */
    static Path writeCombinedTrustStore(List<X509Certificate> extra) throws IOException, GeneralSecurityException {
        KeyStore source = loadEffectiveTrustStore();
        KeyStore out = KeyStore.getInstance(KeyStore.getDefaultType());
        out.load(null, null);
        Set<String> aliases = new HashSet<String>();
        if (source != null) {
            for (Enumeration<String> e = source.aliases(); e.hasMoreElements();) {
                String alias = e.nextElement();
                Certificate cert = null;
                if (source.isCertificateEntry(alias)) {
                    cert = source.getCertificate(alias);
                } else if (source.isKeyEntry(alias)) {
                    // The JDK trusts the end certificate of key entries in a trust store too.
                    Certificate[] chain = source.getCertificateChain(alias);
                    cert = chain != null && chain.length > 0 ? chain[0] : null;
                }
                if (cert != null) {
                    out.setCertificateEntry(alias, cert);
                    aliases.add(alias.toLowerCase(Locale.ROOT));
                }
            }
        }
        int i = 0;
        for (X509Certificate ca : extra) {
            String alias = "akev-ca";
            while (aliases.contains(alias)) {
                alias = "akev-ca-" + ++i;
            }
            aliases.add(alias);
            out.setCertificateEntry(alias, ca);
        }
        String ext = KeyStore.getDefaultType().toLowerCase(Locale.ROOT).equals("pkcs12") ? ".p12" : ".jks";
        Path file = Files.createTempFile("akev-truststore-", ext);
        file.toFile().deleteOnExit();
        OutputStream os = new FileOutputStream(file.toFile());
        try {
            out.store(os, STORE_PASSWORD.toCharArray());
        } finally {
            os.close();
        }
        return file;
    }

    static KeyStore loadEffectiveTrustStore() throws IOException, GeneralSecurityException {
        String home = System.getProperty("java.home", "");
        File security = new File(new File(home, "lib"), "security");
        String configured = System.getProperty("javax.net.ssl.trustStore");
        String type = System.getProperty("javax.net.ssl.trustStoreType", KeyStore.getDefaultType());
        String provider = System.getProperty("javax.net.ssl.trustStoreProvider", "");
        String password = System.getProperty("javax.net.ssl.trustStorePassword", "");
        KeyStore ks = provider.isEmpty() ? KeyStore.getInstance(type) : KeyStore.getInstance(type, provider);
        if ("NONE".equals(configured)) {
            ks.load(null, password.isEmpty() ? null : password.toCharArray());
            return ks;
        }
        String first = configured != null ? configured : new File(security, "jssecacerts").getPath();
        File chosen = null;
        for (String candidate : new String[] {first, new File(security, "cacerts").getPath()}) {
            File f = new File(candidate);
            if (f.isFile() && f.canRead()) {
                chosen = f;
                break;
            }
        }
        if (chosen == null) {
            return null;
        }
        try {
            load(ks, chosen, password.isEmpty() ? null : password.toCharArray());
        } catch (IOException e) {
            // Wrong/absent password: certificates of JKS & password-less PKCS12 stores still load.
            ks = provider.isEmpty() ? KeyStore.getInstance(type) : KeyStore.getInstance(type, provider);
            load(ks, chosen, null);
        }
        return ks;
    }

    private static void load(KeyStore ks, File file, char[] password) throws IOException, GeneralSecurityException {
        InputStream in = new FileInputStream(file);
        try {
            ks.load(in, password);
        } finally {
            in.close();
        }
    }

    /** Whether a class with one of these names has been loaded by any class loader. */
    static List<String> loadedClasses(Instrumentation inst, String... names) {
        List<String> found = new ArrayList<String>();
        if (inst == null) {
            return found;
        }
        Set<String> wanted = new HashSet<String>();
        Collections.addAll(wanted, names);
        for (Class<?> c : inst.getAllLoadedClasses()) {
            String n = c.getName();
            if (wanted.contains(n) && !found.contains(n)) {
                found.add(n);
            }
        }
        return found;
    }

    /**
     * Opens JSSE internals to this agent (Java 9+, through {@link Instrumentation}, so no
     * "illegal reflective access" warnings). A no-op on Java 8.
     */
    @SuppressWarnings({"rawtypes", "unchecked"})
    static void openJsseInternals(Instrumentation inst) throws Exception {
        Class<?> moduleClass;
        try {
            moduleClass = Class.forName("java.lang.Module");
        } catch (ClassNotFoundException e) {
            return; // Java 8: no modules.
        }
        if (inst == null) {
            return;
        }
        Method getModule = Class.class.getMethod("getModule");
        Object javaBase = getModule.invoke(SSLContext.class);
        Object self = getModule.invoke(TrustSetup.class);
        // Always open explicitly, even when Module.isOpen already says true: on Java 9-15 the
        // packages are open only through the legacy --illegal-access=permit mode, and using that
        // path prints an "illegal reflective access" warning on the application's console. An
        // explicit opens (via Instrumentation) makes the access legal, so nothing is printed.
        Map opens = new HashMap();
        for (String pkg : new String[] {"javax.net.ssl", "sun.security.ssl"}) {
            opens.put(pkg, Collections.singleton(self));
        }
        Method redefine = Instrumentation.class.getMethod("redefineModule", moduleClass, Set.class, Map.class,
                Map.class, Set.class, Map.class);
        redefine.invoke(inst, javaBase, Collections.emptySet(), Collections.emptyMap(), opens,
                Collections.emptySet(), Collections.emptyMap());
    }

    /**
     * Swaps the trust manager inside an existing {@link SSLContext} (the JDK's default one), so
     * clients that captured it before the attach (java.net.http.HttpClient instances,
     * HttpsURLConnection's factory, {@code SSLSocketFactory.getDefault()} users) trust Akev
     * too. Uses JSSE internals; {@link #prepare} fails cleanly on JVMs that differ.
     */
    static final class ContextPatch {
        private final Object spi;
        private final Field trustField;
        final X509TrustManager original;
        /** The context's key manager (client certificates), or null if not found. */
        final Object keyManager;

        private ContextPatch(Object spi, Field trustField, X509TrustManager original, Object keyManager) {
            this.spi = spi;
            this.trustField = trustField;
            this.original = original;
            this.keyManager = keyManager;
        }

        static ContextPatch prepare(SSLContext context, Instrumentation inst) throws Exception {
            openJsseInternals(inst);
            Field spiField = SSLContext.class.getDeclaredField("contextSpi");
            spiField.setAccessible(true);
            Object spi = spiField.get(context);
            if (spi == null) {
                throw new IllegalStateException("SSLContext has no SPI");
            }
            Field trust = findField(spi.getClass(), "trustManager", X509TrustManager.class);
            if (trust == null) {
                throw new NoSuchFieldException(spi.getClass().getName() + ".trustManager");
            }
            trust.setAccessible(true);
            Object current = trust.get(spi);
            if (!(current instanceof X509TrustManager)) {
                throw new IllegalStateException("default SSLContext has no X509 trust manager");
            }
            Object km = null;
            Field key = findField(spi.getClass(), "keyManager", null);
            if (key != null) {
                key.setAccessible(true);
                km = key.get(spi);
            }
            return new ContextPatch(spi, trust, (X509TrustManager) current, km);
        }

        void install(X509TrustManager tm) throws IllegalAccessException {
            trustField.set(spi, tm);
        }

        /** Put the original back if {@code ours} is still installed. */
        boolean revert(X509TrustManager ours) throws IllegalAccessException {
            if (trustField.get(spi) != ours) {
                return false;
            }
            trustField.set(spi, original);
            return true;
        }

        private static Field findField(Class<?> cls, String name, Class<?> type) {
            for (Class<?> c = cls; c != null && c != Object.class; c = c.getSuperclass()) {
                try {
                    Field f = c.getDeclaredField(name);
                    if (type == null || f.getType().isAssignableFrom(type)) {
                        return f;
                    }
                } catch (NoSuchFieldException ignored) {
                    // keep looking in the superclass
                }
            }
            return null;
        }
    }
}
