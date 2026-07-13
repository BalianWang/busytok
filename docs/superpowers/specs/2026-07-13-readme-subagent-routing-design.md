# Busytok README Rewrite Design

## Goal

Reposition the repository README around Busytok's primary product capability:
task-level model routing through persistent logical subagents. The README must
help a new user understand the value, run a first delegation, and find deeper
integration and development documentation without claiming capabilities that
are not implemented.

## Audience and language

- `README.md` is the canonical English README and the primary entry point for
  GitHub visitors.
- `README.zh-CN.md` mirrors the English structure and examples in Simplified
  Chinese.
- Commands, flags, JSON field names, provider/model identifiers, and code
  blocks remain identical across both files so that the Chinese version is
  operationally equivalent.
- The English README links to the Chinese version near the top; the Chinese
  README links back to English.

## Information architecture

Both files use the same section order:

1. Title, badges, language switch, and one-sentence positioning.
2. Short explanation of the problem and why task-level routing matters.
3. A compact capability list covering explicit provider/model binding,
   persistent subagent identity, reuse policies, async polling, queueing,
   cancellation, history, and structured diagnostics.
4. A three-step quick start: install the macOS DMG, check service/catalog
   readiness, then run a real `delegate --wait` example.
5. An asynchronous JSON example showing `delegate --output json` followed by
   `subagent task --task-id` polling.
6. Core concepts: provider/model bindings, logical subagents, and
   `create`/`reuse`/`fail` semantics.
7. Product boundaries and local-first behavior.
8. Documentation map for integration, testing, design system, releases,
   development, contributing, security, and license.
9. Workspace/development verification commands.

The README stays concise. Detailed testing matrices, troubleshooting, and
architecture remain in `docs/` and are linked rather than duplicated.

## Product facts and boundaries

The rewrite must describe only behavior present in the current repository:

- Busytok routes explicitly delegated tasks to a provider/model bound to a
  logical subagent identity.
- Reusing a logical subagent reuses its existing binding; it does not silently
  rebind to another provider/model.
- Tasks can be waited on synchronously or submitted as JSON and polled,
  cancelled, and inspected through task history.
- The runtime can queue work, serialize tasks for one logical subagent, reuse
  sidecar sessions, and expose queue reasons and structured errors.
- Providers and models are configured through the GUI or CLI; the live catalog
  is discovered with `busytok models --json`.
- The app is local-first and persists application data in local SQLite.
- Busytok is not a transparent proxy for all Claude/Codex traffic, does not
  intercept TLS, and does not claim to manage external agent OAuth/API
  sessions.

The README must not promise cloud hosting, automatic routing of every external
agent request, unsupported package-manager distribution, or a fixed provider
catalog.

## Command examples

Examples use the actual CLI contract from
`docs/superpowers/guides/busytok-subagent-codex-integration.md`:

- `busytok status`
- `busytok models --json`
- `busytok delegate ... --reuse-policy create --wait --output json`
- `busytok subagent task --task-id "<TASK_ID>" --output json`

Provider IDs are shown as placeholders or explicitly sourced from the live
catalog; the README must not imply that a hard-coded example provider is
always installed.

## Visuals and badges

Retain the existing CI, release, and Apache-2.0 badges. Keep one dashboard
screenshot as product context and retain the prompt-palette screenshot only if
it does not distract from the subagent-routing story. Use repository-relative
image links.

## Acceptance criteria

- The first paragraph and quick-start flow clearly communicate task-level
  subagent routing.
- A reader can copy the readiness and delegation commands after configuring a
  provider/model.
- Sync and async orchestration paths are both documented.
- Reuse-policy semantics and the no-silent-rebinding rule are explicit.
- English and Chinese files have matching headings, command blocks, and links
  to the same source documentation.
- No stale statement says Busytok does not route models.
- Claims are traceable to current code or linked project documentation.
- Existing install, stability, verification, contributing, security, and
  license guidance is preserved or improved.

