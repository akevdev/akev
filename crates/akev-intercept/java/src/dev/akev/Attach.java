package dev.akev;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.io.PrintStream;
import java.lang.reflect.InvocationTargetException;
import java.net.URL;
import java.net.URLClassLoader;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Properties;

/**
 * Loads the Akev agent into a running JVM with the JDK Attach API.
 *
 * <pre>
 * java -cp akev-agent.jar dev.akev.Attach &lt;pid&gt; --proxy 127.0.0.1:8000 --ca ca.pem
 *      [--intercept-loopback] [--proxy-properties auto|always|never] [--quiet]
 * java -cp akev-agent.jar dev.akev.Attach &lt;pid&gt; --restore [--quiet]
 * java -cp akev-agent.jar dev.akev.Attach --check
 * </pre>
 *
 * <p>Prints the agent's status ({@code key=value} lines, see {@link Status}) on stdout. Exit
 * codes: 0 success, 2 usage, 3 Attach API unavailable in this Java (a JRE, or Java 8 without
 * tools.jar), 4 couldn't attach (no such JVM, other user, attach disabled), 5 the JVM refused
 * the agent, 6 the agent reported an error, 7 the agent didn't report back.
 *
 * <p>The Attach API is called reflectively so this compiles for Java 8 (where it lives in
 * {@code tools.jar}) and runs on every later version ({@code jdk.attach} module).
 */
public final class Attach {
    static final int OK = 0;
    static final int USAGE = 2;
    static final int NO_ATTACH_API = 3;
    static final int ATTACH_FAILED = 4;
    static final int LOAD_FAILED = 5;
    static final int AGENT_ERROR = 6;
    static final int NO_STATUS = 7;

    private Attach() {}

    public static void main(String[] args) {
        int code;
        try {
            code = run(args, System.out, System.err);
        } catch (Throwable t) {
            System.out.println("result=error");
            System.out.println("stage=internal");
            System.out.println("error=" + Status.describe(t));
            code = 1;
        }
        System.out.flush();
        System.exit(code);
    }

    static int run(String[] args, PrintStream out, PrintStream err) throws Exception {
        if (args.length == 0 || args[0].equals("--help") || args[0].equals("-h")) {
            usage(err);
            return USAGE;
        }
        if (args[0].equals("--check")) {
            try {
                attachApi();
                out.println("attach=available");
                out.println("javaVersion=" + System.getProperty("java.version"));
                out.println("agentVersion=" + AkevAgent.VERSION);
                return OK;
            } catch (Throwable t) {
                out.println("attach=unavailable");
                out.println("error=" + Status.describe(t));
                return NO_ATTACH_API;
            }
        }
        String pid = args[0];
        if (!pid.matches("[0-9]+")) {
            usage(err);
            return USAGE;
        }
        Properties cfg = new Properties();
        boolean restore = false;
        for (int i = 1; i < args.length; i++) {
            String a = args[i];
            if (a.equals("--restore")) {
                restore = true;
            } else if (a.equals("--intercept-loopback")) {
                cfg.setProperty("interceptLoopback", "true");
            } else if (a.equals("--quiet")) {
                cfg.setProperty("quiet", "true");
            } else if ((a.equals("--proxy") || a.equals("--ca") || a.equals("--proxy-properties")) && i + 1 < args.length) {
                String v = args[++i];
                if (a.equals("--proxy")) {
                    int colon = v.lastIndexOf(':');
                    if (colon <= 0 || colon == v.length() - 1) {
                        err.println("--proxy expects host:port, got " + v);
                        return USAGE;
                    }
                    cfg.setProperty("proxyHost", v.substring(0, colon));
                    cfg.setProperty("proxyPort", v.substring(colon + 1));
                } else if (a.equals("--ca")) {
                    cfg.setProperty("caCert", new File(v).getAbsolutePath());
                } else {
                    cfg.setProperty("proxyProperties", v);
                }
            } else {
                err.println("Unknown or incomplete option: " + a);
                usage(err);
                return USAGE;
            }
        }
        if (!restore && (cfg.getProperty("proxyHost") == null || cfg.getProperty("caCert") == null)) {
            err.println("--proxy and --ca are required (or --restore)");
            usage(err);
            return USAGE;
        }
        cfg.setProperty("action", restore ? "restore" : "apply");

        Path dir = Files.createTempDirectory("akev-attach-");
        Path status = dir.resolve("status.txt");
        Path config = dir.resolve("agent.properties");
        try {
            cfg.setProperty("statusFile", status.toAbsolutePath().toString());
            OutputStream os = new FileOutputStream(config.toFile());
            try {
                cfg.store(os, "Akev agent arguments");
            } finally {
                os.close();
            }

            Class<?> vmClass;
            try {
                vmClass = attachApi();
            } catch (Throwable t) {
                fail(out, "attach-api", "The Java Attach API isn't available in " + System.getProperty("java.home")
                        + " (a JRE without the jdk.attach module, or Java 8 without tools.jar): " + Status.describe(t));
                return NO_ATTACH_API;
            }
            String jar = ownJar();
            Object vm;
            try {
                vm = vmClass.getMethod("attach", String.class).invoke(null, pid);
            } catch (InvocationTargetException e) {
                fail(out, "attach", Status.describe(e.getCause()));
                return ATTACH_FAILED;
            }
            try {
                vmClass.getMethod("loadAgent", String.class, String.class).invoke(vm, jar, "@" + config.toAbsolutePath());
            } catch (InvocationTargetException e) {
                Throwable cause = e.getCause();
                fail(out, "load", Status.describe(cause));
                return cause instanceof IOException ? ATTACH_FAILED : LOAD_FAILED;
            } finally {
                try {
                    vmClass.getMethod("detach").invoke(vm);
                } catch (Throwable ignored) {
                    // The target may already be gone.
                }
            }
            if (!Files.isRegularFile(status) || Files.size(status) == 0) {
                fail(out, "status", "The agent was loaded but didn't report back (is " + pid
                        + " a HotSpot/OpenJ9 JVM that can write to " + dir + "?)");
                return NO_STATUS;
            }
            String text = new String(Files.readAllBytes(status), "UTF-8");
            out.print(text);
            return hasLine(text, "result=ok") ? OK : AGENT_ERROR;
        } finally {
            quietDelete(status, dir.resolve("status.txt.tmp"), config, dir);
        }
    }

    private static void quietDelete(Path... paths) {
        for (Path p : paths) {
            try {
                Files.deleteIfExists(p);
            } catch (IOException ignored) {
                // Best effort: a leftover temporary file is harmless.
            }
        }
    }

    private static boolean hasLine(String text, String line) {
        for (String l : text.split("\n")) {
            if (l.trim().equals(line)) {
                return true;
            }
        }
        return false;
    }

    private static void fail(PrintStream out, String stage, String error) {
        out.println("result=error");
        out.println("stage=" + stage);
        out.println("error=" + Status.clean(error));
    }

    /** {@code com.sun.tools.attach.VirtualMachine}: the jdk.attach module, or Java 8's tools.jar. */
    static Class<?> attachApi() throws Exception {
        String name = "com.sun.tools.attach.VirtualMachine";
        try {
            return Class.forName(name);
        } catch (ClassNotFoundException e) {
            File home = new File(System.getProperty("java.home"));
            File[] candidates = {new File(home.getParentFile(), "lib/tools.jar"), new File(home, "lib/tools.jar")};
            for (File tools : candidates) {
                if (tools.isFile()) {
                    URLClassLoader loader = new URLClassLoader(new URL[] {tools.toURI().toURL()},
                            Attach.class.getClassLoader());
                    return Class.forName(name, true, loader);
                }
            }
            throw e;
        }
    }

    private static String ownJar() throws Exception {
        URL location = Attach.class.getProtectionDomain().getCodeSource().getLocation();
        File f = new File(location.toURI());
        if (!f.isFile()) {
            throw new IllegalStateException("dev.akev.Attach must run from akev-agent.jar (found " + f + ")");
        }
        return f.getAbsolutePath();
    }

    private static void usage(PrintStream err) {
        err.println("Usage: java -cp akev-agent.jar dev.akev.Attach <pid> --proxy <host:port> --ca <ca.pem>");
        err.println("           [--intercept-loopback] [--proxy-properties auto|always|never] [--quiet]");
        err.println("       java -cp akev-agent.jar dev.akev.Attach <pid> --restore [--quiet]");
        err.println("       java -cp akev-agent.jar dev.akev.Attach --check");
    }
}
