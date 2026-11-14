/*
 * Akev JVM agent: routes an already-running (or starting) JVM's HTTP(S) clients through the
 * Akev proxy and makes them trust the Akev CA in addition to the CAs they already trust.
 *
 * Java 8 compatible on purpose (compiled with `javac --release 8`): it is loaded into
 * arbitrary target JVMs. No dependencies; Java 9+ only APIs are called reflectively.
 */
package dev.akev;

import java.lang.instrument.Instrumentation;

/**
 * Agent entry points.
 *
 * <p>Dynamic attach ({@code agentmain}) and {@code -javaagent:} ({@code premain}) take the same
 * arguments: either {@code @/path/to/agent.properties} (what {@link Attach} passes) or an
 * URL-encoded query string such as
 * {@code proxyHost=127.0.0.1&proxyPort=8000&caCert=/home/me/.local/share/akev/ca.pem}.
 * {@code action=restore} undoes a previous attach. See {@link AgentConfig} for every key.
 *
 * <p>The agent never throws: failures are reported in the status file (and on the target's
 * standard error) so a broken configuration can't take the application down.
 */
public final class AkevAgent {
    /** Agent version, reported in the status. */
    public static final String VERSION = "1.0.0";

    private AkevAgent() {}

    /** {@code -javaagent:akev-agent.jar=<args>} at JVM startup. */
    public static void premain(String args, Instrumentation inst) {
        Interception.run(args, inst, "premain");
    }

    /** Startup entry point for JVMs that don't pass an {@link Instrumentation}. */
    public static void premain(String args) {
        Interception.run(args, null, "premain");
    }

    /** Dynamic attach ({@code VirtualMachine.loadAgent}). */
    public static void agentmain(String args, Instrumentation inst) {
        Interception.run(args, inst, "agentmain");
    }

    /** Dynamic attach for JVMs that don't pass an {@link Instrumentation}. */
    public static void agentmain(String args) {
        Interception.run(args, null, "agentmain");
    }
}
