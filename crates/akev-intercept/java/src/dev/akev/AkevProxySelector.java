package dev.akev;

import java.io.IOException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Proxy;
import java.net.ProxySelector;
import java.net.SocketAddress;
import java.net.URI;
import java.net.UnknownHostException;
import java.util.Collections;
import java.util.List;
import java.util.Locale;

/**
 * Routes {@code http}, {@code https}, {@code ws} and {@code wss} URIs through Akev, except
 * loopback hosts unless {@code interceptLoopback} is set. Everything else (SOCKS
 * {@code socket://} lookups, FTP...) goes to the selector that was the default before.
 */
final class AkevProxySelector extends ProxySelector {
    private static final List<Proxy> DIRECT = Collections.singletonList(Proxy.NO_PROXY);

    /** The default selector before Akev (may be null). */
    final ProxySelector previous;
    private final Proxy proxy;
    private final List<Proxy> proxied;
    private final boolean interceptLoopback;

    AkevProxySelector(ProxySelector previous, String host, int port, boolean interceptLoopback) {
        this.previous = previous;
        this.proxy = new Proxy(Proxy.Type.HTTP, new InetSocketAddress(host, port));
        this.proxied = Collections.singletonList(proxy);
        this.interceptLoopback = interceptLoopback;
    }

    Proxy proxy() {
        return proxy;
    }

    @Override
    public List<Proxy> select(URI uri) {
        if (uri == null) {
            throw new IllegalArgumentException("URI can't be null.");
        }
        String scheme = uri.getScheme();
        if (scheme != null && isWebScheme(scheme)) {
            String host = hostOf(uri);
            if (host != null && !host.isEmpty()) {
                if (!interceptLoopback && isLoopback(host)) {
                    return DIRECT;
                }
                return proxied;
            }
        }
        return delegate(uri);
    }

    private List<Proxy> delegate(URI uri) {
        if (previous != null && previous != this) {
            List<Proxy> chosen = previous.select(uri);
            if (chosen != null && !chosen.isEmpty()) {
                return chosen;
            }
        }
        return DIRECT;
    }

    @Override
    public void connectFailed(URI uri, SocketAddress sa, IOException ioe) {
        if (uri == null || sa == null || ioe == null) {
            throw new IllegalArgumentException("Arguments can't be null.");
        }
        if (previous != null && previous != this && !sa.equals(proxy.address())) {
            previous.connectFailed(uri, sa, ioe);
        }
    }

    static boolean isWebScheme(String scheme) {
        String s = scheme.toLowerCase(Locale.ROOT);
        return s.equals("http") || s.equals("https") || s.equals("ws") || s.equals("wss");
    }

    /** The URI's host, also for hosts {@link URI#getHost()} rejects (e.g. with underscores). */
    static String hostOf(URI uri) {
        String host = uri.getHost();
        if (host != null) {
            return host;
        }
        String authority = uri.getRawAuthority();
        if (authority == null) {
            return null;
        }
        int at = authority.lastIndexOf('@');
        if (at >= 0) {
            authority = authority.substring(at + 1);
        }
        if (authority.startsWith("[")) {
            int end = authority.indexOf(']');
            return end > 0 ? authority.substring(0, end + 1) : authority;
        }
        int colon = authority.indexOf(':');
        if (colon >= 0 && authority.indexOf(':', colon + 1) < 0) {
            authority = authority.substring(0, colon);
        }
        return authority;
    }

    /**
     * {@code localhost}, {@code *.localhost}, {@code 127.0.0.0/8}, {@code ::1}, the
     * unspecified addresses and IPv4-mapped loopback. Decided from the text only: never
     * resolves names.
     */
    static boolean isLoopback(String rawHost) {
        String h = rawHost.trim().toLowerCase(Locale.ROOT);
        if (h.startsWith("[") && h.endsWith("]") && h.length() >= 2) {
            h = h.substring(1, h.length() - 1);
        }
        while (h.endsWith(".")) {
            h = h.substring(0, h.length() - 1);
        }
        if (h.equals("localhost") || h.endsWith(".localhost")) {
            return true;
        }
        if (isIpv4Literal(h)) {
            return h.startsWith("127.") || h.equals("0.0.0.0");
        }
        if (h.indexOf(':') >= 0) {
            int zone = h.indexOf('%');
            if (zone >= 0) {
                h = h.substring(0, zone);
            }
            try {
                // Bracketed, so an invalid literal fails instead of being looked up in DNS.
                InetAddress a = InetAddress.getByName("[" + h + "]");
                return a.isLoopbackAddress() || a.isAnyLocalAddress();
            } catch (UnknownHostException e) {
                return false;
            }
        }
        return false;
    }

    static boolean isIpv4Literal(String h) {
        String[] parts = h.split("\\.", -1);
        if (parts.length != 4) {
            return false;
        }
        for (String part : parts) {
            if (part.isEmpty() || part.length() > 3) {
                return false;
            }
            for (int i = 0; i < part.length(); i++) {
                if (part.charAt(i) < '0' || part.charAt(i) > '9') {
                    return false;
                }
            }
            if (Integer.parseInt(part) > 255) {
                return false;
            }
        }
        return true;
    }
}
