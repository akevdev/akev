# Security policy

Akev installs a root certificate authority and sits between your applications and the network,
so we treat security problems as the most urgent kind of bug.

## Reporting a vulnerability

Email **security@akev.dev**, or use GitHub's private vulnerability reporting
([Security → Report a vulnerability](https://github.com/akevdev/akev/security/advisories/new)),
with a description, the affected version (`akev --version`) and steps to reproduce. Please
don't open a public issue for anything exploitable.

- We acknowledge reports within 3 working days and keep you updated until a fix ships.
- We aim to release fixes for critical issues within 14 days and credit you in the release
  notes unless you prefer otherwise.
- Please give us reasonable time to fix the problem before you disclose it.

The machine-readable version of this policy is published at
<https://akev.dev/.well-known/security.txt>.

## Supported versions

Security fixes go into the latest release. Akev Pro licences cover every version released during
their year of updates, so updating never costs a licence holder anything within that year.

## Scope

In scope, for example:

- the CA private key being readable by other users, or leaving the data directory;
- the API, UI or MCP endpoint being reachable or usable without the session token;
- the proxy being usable by other machines when it isn't meant to be;
- reading or writing files outside the places Akev documents (Map Local roots, exports);
- licence verification bypasses in official builds.

Out of scope: attacks that need an attacker who already controls your user account, and
findings in third-party dependencies without a working exploit through Akev (report those
upstream; we track advisories with `cargo deny` and Dependabot).
