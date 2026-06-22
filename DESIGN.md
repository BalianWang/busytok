# Busytok — Design

## What Busytok is

Busytok is a **local-first agent token usage audit dashboard**. It reads
local AI coding agent logs (Claude Code, Codex, Gemini CLI), normalizes
low-sensitive token metadata, stores it in local SQLite, and serves
GUI/CLI views through a local background service.

## What Busytok is not

- ❌ Does not proxy network traffic
- ❌ Does not store provider credentials or OAuth/session tokens
- ❌ Does not modify client agent configurations
- ❌ Does not inspect protocol payloads (only log-parsed token metadata)

These are load-bearing constraints. A contribution that touches any of
them will be rejected.

## System architecture

```
Agent logs (on disk)         Busytok                           User
────────────────────────────────────────────────────────────────────
Claude Code logs ──┐
Codex logs ────────┤   ┌──────────────┐    ┌─────┐    ┌──────────┐
Gemini CLI logs ───┼──→│ busytok-     │───→│SQLite│───→│ GUI      │
                         │ service      │    └─────┘    │ (Tauri)  │
                         │ (background) │               │ CLI      │
                         └──────────────┘               └──────────┘
```

### Data flow

1. **Read** — `busytok-service` tails agent log files on disk.
   Supported agents: Claude Code, Codex, Gemini CLI.
2. **Normalize** — log lines are parsed into token-usage events
   (model, prompt tokens, completion tokens, timestamp). Prompt and
   response bodies are **never** read or stored — only the
   token-count metadata.
3. **Store** — normalized events are written to a local SQLite
   database in WAL mode. The writer actor batches writes; WAL
   mode ensures crash-safety.
4. **Serve** — a local control server exposes the data over a
   Unix-domain socket. The Tauri GUI and `busytok` CLI both
   consume this API.

### Component map

| Crate / App | Role |
|---|---|
| `crates/busytok-config` | Paths, service marker, port allocation |
| `crates/busytok-runtime` | Service lifecycle, supervisor, bootstrap, tailer, writer |
| `crates/busytok-store` | SQLite schema, migrations, queries |
| `crates/busytok-control` | Unix-domain-socket control server + client SDK |
| `crates/busytok-parser` | Log-line parsing for Claude Code, Codex, Gemini CLI |
| `crates/busytok-model` | Shared data types (token usage, agent identity, pricing) |
| `apps/service` | `busytok-service` binary — the background daemon |
| `apps/gui` | React + Tauri desktop app (Dashboard, Agents, Settings) |
| `apps/gui/src-tauri` | Tauri Rust host + bundle config |
| `apps/cli` | `busytok` CLI — scan, export, settings, shim management |
| `packages/busytok-protocol-types` | Shared TypeScript types for GUI ↔ service protocol |

## macOS service lifecycle

The background service (`busytok-service`) runs as a **launchd LaunchAgent**
in the user's Aqua domain. At startup the GUI renders a minimal agent plist
into the current user's `~/Library/LaunchAgents/com.busytok.service.plist`
and registers it with `launchctl bootstrap`.

- **Registration**: the GUI renders the plist at **runtime** from the
  current install location (`ProgramArguments[0]` =
  `<install>/Busytok.app/Contents/MacOS/busytok-service`), then
  `launchctl bootstrap gui/<uid> <plist>`. Production does NOT call
  `SMAppService.register/status/unregister` — ServiceManagement can throw
  Objective-C exceptions that do not safely cross the Rust FFI boundary, so
  the lifecycle is `launchctl`-backed end to end. The plist carries only
  `Label`, `ProgramArguments`, `RunAtLoad`, `KeepAlive` — no log paths or
  env vars; the service resolves its own data/log dirs at runtime via
  `BusytokPaths::new()`.
- **Install-location invariance**: because the plist is rendered from the
  running GUI's own bundle path, moving `Busytok.app` self-heals on next
  launch. The stale-bundle repair (`launchctl print` detects `registered ≠
  desired`) rewrites the plist to the current path, then bootout →
  bootstrap → kickstart.
- **Lifecycle**: `RunAtLoad` + `KeepAlive` — the service starts at login
  and is restarted if it exits. The GUI polls a `service.ready` marker
  file written by the service after bootstrap completes.
- **Shutdown**: On app quit, the GUI sends a shutdown command over the
  control socket. The service drains the tailer, flushes the writer,
  runs a WAL checkpoint, and removes the readiness marker. On Unix
  (macOS), SIGTERM from launchd triggers the same graceful path.
- **Crash recovery**: SQLite WAL mode is crash-safe. On abrupt
  termination (forced quit, power loss), committed data survives;
  uncommitted writes are discarded by WAL recovery. The tailer resumes
  from the last checkpoint offset on next boot. A stale `service.ready`
  marker from a crashed run is cleared at the start of each boot.

## Data minimization & privacy

- **No prompt or response bodies** are ever read, parsed, or stored.
  Only token-count metadata (model name, prompt tokens, completion
  tokens, timestamp, agent identity) is persisted.
- **No provider credentials** — API keys, OAuth tokens, and session
  cookies are never read or stored.
- **No network egress** — the service does not make outbound network
  requests. All data stays on the local machine.
- **Local SQLite** — the database lives at
  `~/Library/Application Support/Busytok/`. No cloud sync, no telemetry.
- **Log parsing is read-only** — agent log files are opened for reading
  only; no agent configuration is modified.

## Key design decisions

- **Metadata-only**. The product contract is defined as much by what
  is excluded as by what is included. This is not a proxy, not a
  key manager, not a config tool.
- **Service-first**. The background service owns the data path
  (read → normalize → store). The GUI and CLI are views — they
  consume the control-server API but never touch the database or
  log files directly.
- **Crash-safe by default**. SQLite WAL + idempotent log parsing +
  checkpoint-offset tailing means the system tolerates abrupt
  termination without data loss.
- **Single binary, single bundle**. `busytok-service` and `busytok`
  CLI are compiled as standalone Rust binaries and bundled inside
  the Tauri `.app`. The LaunchAgent plist is bundled (not installed
  to user directories), keeping the install surface to a single
  drag-to-Applications.
- **Cross-platform core, macOS GUI**. The library crates compile
  on Linux and Windows. The Tauri GUI is macOS-only (relies on
  native windowing, menu-bar app mode, and launchd). CI runs the
  full test suite on ubuntu, macOS, and Windows; the GUI crate is
  excluded on non-macOS.

## Visual design

The visual design system ("Sentri Inspired") is documented in
[`THEME.md`](THEME.md) — a dark violet-and-lime design language
with a proprietary display sans, Rubik for UI copy, and Monaco
for code.
