# Security Policy

## Reporting a Vulnerability

**Please do not open public GitHub issues for security reports.** Publicly
disclosing a vulnerability before a fix is available puts all users at risk.

### Where to report depends on which code is affected

AiNxt CLI is NPCI's fork of [Grok Build](https://github.com/xai-org/grok-build),
developed by SpaceXAI. Most of this tree is upstream code, and a vulnerability in
it needs to reach the people who can fix it at source — otherwise every other
fork of Grok Build stays exposed.

| Affected code | Report to | Why |
|---|---|---|
| **Upstream Grok Build** — the agent harness, TUI, tool runtime, session handling: most of the tree | SpaceXAI's HackerOne programme: **https://hackerone.com/x** | They maintain it and can fix it for everyone. This is upstream's own stated channel (see their [`SECURITY.md`](https://github.com/xai-org/grok-build/blob/main/SECURITY.md)). |
| **This fork's own changes** — bearer-token sign-in, gateway selection and trust, replaced endpoints, the `AINXT_*` configuration surface | This repository's private advisory: **Security** tab → **Report a vulnerability**, or [`../../security/advisories/new`](../../security/advisories/new) | Only present in this fork; upstream cannot act on it. |


Include as much of the following as possible in your report:

- A description of the vulnerability and its potential impact
- The affected component(s) and version(s)
- Step-by-step reproduction instructions or a proof-of-concept
- Any suggested mitigations you have identified

---
