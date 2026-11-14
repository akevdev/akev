package dev.akev;

import java.net.Socket;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;
import javax.net.ssl.SSLEngine;
import javax.net.ssl.X509ExtendedTrustManager;
import javax.net.ssl.X509TrustManager;
import javax.security.auth.x500.X500Principal;

/**
 * Trusts a server if <em>either</em> the JVM's existing trust manager or a trust manager that
 * only knows the Akev CA accepts its chain. Both are full PKIX validators (expiry, signatures,
 * key usage, and hostname checks when the caller asks for them) — nothing is "trust all".
 *
 * <p>Client certificates (the application acting as a TLS server) are only checked by the
 * original trust manager: attaching Akev never widens who may authenticate to the app.
 */
final class AkevTrustManager extends X509ExtendedTrustManager {
    final X509TrustManager primary;
    private final X509ExtendedTrustManager akev;
    private final Set<X500Principal> akevSubjects = new HashSet<X500Principal>();
    private final X509Certificate[] issuers;

    AkevTrustManager(X509TrustManager primary, X509ExtendedTrustManager akev, List<X509Certificate> akevCas) {
        this.primary = primary;
        this.akev = akev;
        Set<X509Certificate> all = new LinkedHashSet<X509Certificate>();
        X509Certificate[] existing = primary.getAcceptedIssuers();
        if (existing != null) {
            for (X509Certificate c : existing) {
                all.add(c);
            }
        }
        for (X509Certificate c : akevCas) {
            akevSubjects.add(c.getSubjectX500Principal());
            all.add(c);
        }
        this.issuers = new ArrayList<X509Certificate>(all).toArray(new X509Certificate[0]);
    }

    @Override
    public void checkServerTrusted(X509Certificate[] chain, String authType) throws CertificateException {
        server(chain, authType, null, null);
    }

    @Override
    public void checkServerTrusted(X509Certificate[] chain, String authType, Socket socket)
            throws CertificateException {
        server(chain, authType, socket, null);
    }

    @Override
    public void checkServerTrusted(X509Certificate[] chain, String authType, SSLEngine engine)
            throws CertificateException {
        server(chain, authType, null, engine);
    }

    @Override
    public void checkClientTrusted(X509Certificate[] chain, String authType) throws CertificateException {
        primary.checkClientTrusted(chain, authType);
    }

    @Override
    public void checkClientTrusted(X509Certificate[] chain, String authType, Socket socket)
            throws CertificateException {
        if (primary instanceof X509ExtendedTrustManager) {
            ((X509ExtendedTrustManager) primary).checkClientTrusted(chain, authType, socket);
        } else {
            primary.checkClientTrusted(chain, authType);
        }
    }

    @Override
    public void checkClientTrusted(X509Certificate[] chain, String authType, SSLEngine engine)
            throws CertificateException {
        if (primary instanceof X509ExtendedTrustManager) {
            ((X509ExtendedTrustManager) primary).checkClientTrusted(chain, authType, engine);
        } else {
            primary.checkClientTrusted(chain, authType);
        }
    }

    @Override
    public X509Certificate[] getAcceptedIssuers() {
        return issuers.clone();
    }

    private void server(X509Certificate[] chain, String authType, Socket socket, SSLEngine engine)
            throws CertificateException {
        // Chains minted by Akev are checked against the Akev CA first, so intercepted
        // handshakes don't pay for a failed path build against every root.
        boolean akevFirst = issuedByAkev(chain);
        X509TrustManager first = akevFirst ? akev : primary;
        X509TrustManager second = akevFirst ? primary : akev;
        try {
            check(first, chain, authType, socket, engine);
        } catch (CertificateException e) {
            try {
                check(second, chain, authType, socket, engine);
            } catch (CertificateException other) {
                e.addSuppressed(other);
                throw e;
            }
        }
    }

    private boolean issuedByAkev(X509Certificate[] chain) {
        if (chain == null || chain.length == 0) {
            return false;
        }
        X509Certificate top = chain[chain.length - 1];
        return akevSubjects.contains(top.getIssuerX500Principal())
                || akevSubjects.contains(top.getSubjectX500Principal());
    }

    private static void check(X509TrustManager tm, X509Certificate[] chain, String authType, Socket socket,
            SSLEngine engine) throws CertificateException {
        if (tm instanceof X509ExtendedTrustManager) {
            X509ExtendedTrustManager x = (X509ExtendedTrustManager) tm;
            if (engine != null) {
                x.checkServerTrusted(chain, authType, engine);
                return;
            }
            if (socket != null) {
                x.checkServerTrusted(chain, authType, socket);
                return;
            }
        }
        tm.checkServerTrusted(chain, authType);
    }
}
