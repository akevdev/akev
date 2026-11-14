# Akev JVM agent

Java sources of the agent the `jvm` interceptor attaches to running JVMs
([user guide](https://akev.dev/docs/jvm/)). Java 8 compatible on purpose (it's loaded into any
JVM ≥ 8), no dependencies; Java 9+ APIs are called reflectively.

| Class | Role |
|---|---|
| `AkevAgent` | `premain` / `agentmain` entry points |
| `AgentConfig` | arguments: `@file.properties` or an URL-encoded query string |
| `Interception` | applies / reverts the JVM-wide changes, saving every original value |
| `AkevProxySelector` | HTTP(S)/WS(S) → Akev, loopback direct unless asked, the rest → previous selector |
| `AkevTrustManager` | accepts a server if the original trust manager **or** an Akev-CA-only one does |
| `TrustSetup` | certificate loading, combined trust store, in-place update of the existing default `SSLContext` |
| `Status` | the `key=value` report |
| `Attach` | `java -cp akev-agent.jar dev.akev.Attach <pid> …`: loads the agent with the Attach API |

`build.rs` compiles these with `javac --release 8` into the jar the crate embeds; its manifest
carries `Akev-Source-Hash` (`src/jvm/source_hash.rs`). No binary jar is committed. Without a JDK
the build still succeeds with an empty jar, and JVM attach reports that the build has no agent;
`AKEV_AGENT_REQUIRE_JDK=1` (set by CI and releases) makes that a build error instead.

Tests: `test/dev/akev/AgentSelfTest.java` (run by `tests/java_agent.rs` with and without
JSSE access, plus `-javaagent:` startup and the `Attach` CLI against a live JVM) and the
end-to-end `crates/akev-api/tests/jvm_attach.rs`. All skip without a JDK.
