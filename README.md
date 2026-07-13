# Busytok

[![CI](https://github.com/BalianWang/busytok/actions/workflows/verify.yml/badge.svg?branch=main)](https://github.com/BalianWang/busytok/actions/workflows/verify.yml)
[![Release](https://img.shields.io/github/v/release/BalianWang/busytok?include_prereleases)](https://github.com/BalianWang/busytok/releases)
[![License: Apache--2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)

[简体中文](README.zh-CN.md) | English

**Busytok routes delegated tasks to an explicit provider/model binding through a persistent logical subagent identity.** It is local-first: the desktop app and `busytok` CLI coordinate task execution, queueing, and diagnostics on your machine while the existing audit dashboard remains available for local agent usage metadata.

![Busytok Dashboard](docs/assets/dashboard.png)

## Why task-level routing?

Different tasks need different models, and a stable role should keep using the
same routing decision until you intentionally change it. Busytok lets a caller
choose a provider and model for a logical subagent, delegate one task, then
wait, poll, cancel, or inspect the result without silently rebinding that role.

## Capabilities

- Explicit provider/model binding sourced from the live catalog
- Persistent logical subagent identities for role-oriented workflows
- `create`, `reuse`, and `fail` reuse policies with no silent rebinding
- Synchronous `--wait` completion or asynchronous JSON submission and polling
- Per-subagent task serialization, queueing, and sidecar session reuse
- Queue reasons, cancellation, task history, and structured error diagnostics
- Local SQLite persistence and a desktop dashboard for local agent metadata

## Quick start (macOS)

### 1. Install the app

Download the latest universal DMG from [Releases](https://github.com/BalianWang/busytok/releases/latest), open it, and drag `Busytok.app` to `/Applications`. Apple Silicon and Intel are both supported.

### 2. Check service and catalog readiness

Start the app, then verify that the local service is ready and inspect the
enabled provider/model catalog:

```bash
busytok status
busytok models --json
```

Configure at least one provider and enabled model in the GUI or with the CLI.
Use the `provider_id` and `model_id` returned by `busytok models --json`; do not
assume a fixed provider or catalog order.

### 3. Delegate and wait

Replace `<PROVIDER_ID>` and `<MODEL_ID>` with IDs from the live catalog:

```bash
busytok delegate \
  --subagent "reviewer-001" \
  --profile "pi/review-cheap" \
  --bind-provider "<PROVIDER_ID>" \
  --bind-model "<MODEL_ID>" \
  --reuse-policy create \
  --output json \
  --wait \
  --wait-timeout 120 \
  "Review the repository's open TODOs and return the three highest-impact items."
```

The response is machine-readable JSON on stdout. Keep stderr separate for
diagnostics; do not merge the streams in automation.

## Asynchronous delegation

For longer work, submit without `--wait`, read the returned `task_id`, and poll
the task until its status is terminal:

```bash
busytok delegate \
  --subagent "reviewer-async-001" \
  --profile "pi/review-cheap" \
  --bind-provider "<PROVIDER_ID>" \
  --bind-model "<MODEL_ID>" \
  --reuse-policy create \
  --output json \
  "Review the repository's open TODOs and return the three highest-impact items."

busytok subagent task --task-id "<TASK_ID>" --output json
```

`completed` is success; `queued` and `running` are still in progress; `failed`
and `cancelled` should be surfaced with their structured error or cancellation
context. See the [integration guide](docs/superpowers/guides/busytok-subagent-codex-integration.md) for deterministic catalog selection, prompt channels, and cancellation flows.

## Core concepts

### Provider/model bindings

Each logical subagent can be bound to a provider UUID (`--bind-provider`) and a
model ID (`--bind-model`). The live catalog from `busytok models --json` is the
source of truth. A one-task `--model` override is distinct from a persistent
binding.

### Logical subagents and reuse policies

`--subagent` is a stable, role-oriented identity. Use:

- `--reuse-policy create` to create a new routing identity
- `--reuse-policy reuse` to intentionally use the existing binding
- `--reuse-policy fail` to reject a name collision

Reusing a name never silently rebinds it. To route the role to a different
provider/model, create a new logical subagent name.

### Task lifecycle

The runtime serializes work for one logical subagent, queues tasks when needed,
and can reuse its sidecar session. Use task polling, cancellation, and history
commands to manage work after submission; structured queue reasons and errors
make failures diagnosable.

## Product boundaries

Busytok is local-first and stores application data in local SQLite. Providers
and models are configured by you in the GUI or CLI, and delegation is explicit.
It is not a transparent proxy for all Claude/Codex traffic, does not intercept
TLS, and does not manage external agent OAuth/API sessions. It does not promise
cloud hosting, automatic routing of every external agent request, or a fixed
provider catalog.

## Local desktop context

For imported Claude Code and Codex usage logs, the desktop app keeps
token/usage metadata rather than prompt/response bodies. Delegated task
prompts/results and prompt-palette templates are separate local app data. The
desktop UI exposes Overview, Usage, Prompt Palette, Providers, Subagents, and
Settings views.
Press **`Cmd+Option+K`** to open the optional prompt palette for saving and
reusing local prompt templates.

![Prompt Palette](docs/assets/prompt-palette.png)

## Documentation

| Topic | Guide |
| --- | --- |
| Agent integration and CLI contract | [Subagent delegation guide](docs/superpowers/guides/busytok-subagent-codex-integration.md) |
| Subagent testing and isolation | [Subagent testing guide](docs/subagent-testing-guide.md) |
| Product design | [Design](DESIGN.md) · [Design system](DESIGN-SYSTEM.md) |
| Releases | [Release workflow](docs/release-workflow.md) |
| Development and contribution | [Contributing](CONTRIBUTING.md) |
| Security reporting | [Security policy](SECURITY.md) |
| License | [Apache-2.0](LICENSE) |

## Workspace and verification

- `apps/gui`: React + Tauri desktop application
- `apps/gui/src-tauri`: Tauri Rust host crate and bundle configuration
- `apps/service`: Rust background service
- `apps/cli`: Rust administrative CLI
- `crates/busytok-*`: Rust workspace crates

Run the local acceptance gate before a pull request:

```bash
./scripts/verify_acceptance.sh
```

For a release rehearsal on macOS:

```bash
DEVELOPER_ID_APPLICATION="Developer ID Application: ..." ./scripts/verify_release.sh
```

The naming check is:

```bash
bash scripts/check-busytok-naming.sh
```

## Stability contract

Busytok is `0.x`: real and usable, but **minor releases may break**. macOS
releases use the universal DMG and may auto-update; reinstall manually from
[Releases](https://github.com/BalianWang/busytok/releases/latest) when needed.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the toolchain, branch model, and
required CI checks. Pull requests target `main` and should use Conventional
Commit titles.

## Security

See [`SECURITY.md`](SECURITY.md). Report vulnerabilities through [GitHub Private
Vulnerability Reporting](https://github.com/BalianWang/busytok/security/advisories/new);
do not open a public issue for a security report.

## License

[Apache-2.0](LICENSE)
