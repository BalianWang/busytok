# Security Policy

## Supported versions

Busytok is pre-1.0. Only the latest minor release receives security fixes. The 0.x line explicitly does not promise backports — see README for the 0.x stability contract.

## Reporting a vulnerability

**Preferred channel: GitHub Private Vulnerability Reporting.**

Open `https://github.com/BalianWang/busytok/security/advisories/new` and submit a private advisory. This routes directly to the maintainer without public disclosure.

If GitHub PVR is unavailable, email the maintainer directly (see the GitHub profile). **Do not open a public GitHub issue for security reports.**

Please include:
- Busytok version (from Settings → About, or `busytok --version`)
- macOS version
- The agent type whose logs were being audited (Claude Code / Codex / Gemini CLI)
- The log path that triggered the issue (if applicable)
- Reproduction steps
- Impact assessment

## Response timelines

- **Acknowledgement:** within 72 hours
- **Initial assessment:** within 7 days
- **Fix or mitigation:** target 30 days for high-severity issues, 90 days for low-severity

These are targets, not SLAs — this is a one-maintainer project.

## Scope

Busytok is a **local-first agent token audit dashboard**. It reads local AI agent log files, persists metadata-only token usage to local SQLite, and exposes a local GUI/CLI. **It does not proxy network traffic, store credentials, modify client configurations, or handle provider OAuth/session tokens.**

Vulnerabilities in any of these properties are in scope:
- Sandbox escape via the GUI (webview → filesystem → user session)
- Privilege escalation via the bundled `busytok-service` (LaunchAgent / SMAppService)
- SQLite injection via crafted log content
- Crash loops triggered by malformed agent logs that prevent the app from starting
- Path traversal in log path discovery

Out of scope:
- Vulnerabilities in third-party agents themselves (report to the agent's maintainer)
- Vulnerabilities in dependencies — these are tracked by `cargo-audit` and `dependabot`; report directly to the upstream project

## Triage scope note

Busytok reads local AI agent logs. A reporter may include log excerpts that contain prompt content. The maintainer will treat all such material as confidential and delete it after the report is resolved.
