package dev.akev;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.charset.Charset;
import java.util.ArrayList;
import java.util.List;

/**
 * Outcome of an agent run, written as {@code key=value} lines (one per line, values on one
 * line; keys may repeat, e.g. {@code warning}). Akev parses this format.
 */
final class Status {
    private static final Charset UTF8 = Charset.forName("UTF-8");
    private final List<String[]> entries = new ArrayList<String[]>();

    void put(String key, String value) {
        entries.add(new String[] {key, clean(value)});
    }

    void warn(String message) {
        put("warning", message);
    }

    /** Last value for {@code key}, or null. */
    String get(String key) {
        String found = null;
        for (String[] e : entries) {
            if (e[0].equals(key)) {
                found = e[1];
            }
        }
        return found;
    }

    List<String> all(String key) {
        List<String> out = new ArrayList<String>();
        for (String[] e : entries) {
            if (e[0].equals(key)) {
                out.add(e[1]);
            }
        }
        return out;
    }

    String render() {
        StringBuilder sb = new StringBuilder();
        for (String[] e : entries) {
            sb.append(e[0]).append('=').append(e[1]).append('\n');
        }
        return sb.toString();
    }

    void writeTo(String path) throws IOException {
        File target = new File(path);
        File tmp = new File(target.getPath() + ".tmp");
        OutputStream out = new FileOutputStream(tmp);
        try {
            out.write(render().getBytes(UTF8));
        } finally {
            out.close();
        }
        if (!tmp.renameTo(target)) {
            // Windows can't rename over an existing file.
            if (!target.delete() || !tmp.renameTo(target)) {
                throw new IOException("can't write " + path);
            }
        }
    }

    static String clean(String value) {
        if (value == null) {
            return "";
        }
        StringBuilder sb = new StringBuilder(value.length());
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            sb.append(c == '\n' || c == '\r' || c == '\t' ? ' ' : c);
        }
        return sb.toString().trim();
    }

    /** One-line description of a throwable (class + message, with the root cause). */
    static String describe(Throwable t) {
        Throwable root = t;
        while (root.getCause() != null && root.getCause() != root) {
            root = root.getCause();
        }
        String msg = t.getClass().getName() + (t.getMessage() == null ? "" : ": " + t.getMessage());
        if (root != t) {
            msg += " (caused by " + root.getClass().getName()
                    + (root.getMessage() == null ? "" : ": " + root.getMessage()) + ")";
        }
        return clean(msg);
    }
}
