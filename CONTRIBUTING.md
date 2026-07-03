# Contributing to Busytok

Thanks for your interest in contributing. This document covers the practical setup.

## Project context

Busytok is a **local-first agent token usage audit dashboard**. Before contributing, read:
- `README.md` — what the project is and is not

The most load-bearing principle: Busytok **never proxies traffic, never stores credentials in config files, never modifies client config, never handles OAuth/session tokens.** Provider API keys are stored exclusively in the OS keychain (macOS Keychain / Windows Credential Manager) via `keyring-rs`. Keys are never written to config files, never logged, never transmitted over network except to the configured provider endpoint during an explicit user-initiated connection test. Keys are injected into the sidecar subprocess via environment variables at spawn time only. A PR that violates any of these will be rejected.

## Dev setup

Requirements:
- macOS 14+ (for the Tauri GUI + service)
- Rust stable (see `rust-toolchain.toml`)
- Node 22 + pnpm 10
- Xcode command-line tools (for `codesign`, `xcrun`)

```bash
git clone https://github.com/BalianWang/busytok.git
cd busytok
pnpm install --frozen-lockfile
```

Verify the toolchain:

```bash
cargo --version
pnpm --version
```

## Local verification

Run the acceptance gate before every PR:

```bash
./scripts/verify_acceptance.sh
```

This runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`, `pnpm typecheck`, `pnpm -r test`, plus smoke tests.

For a full local release rehearsal (slow; only on macOS):

```bash
./scripts/verify_release.sh
```

## Branch model

- `main` — the only long-lived branch; always buildable. Never commit directly; update via PR.

PRs:
- Open against `main`
- Title: Conventional Commits (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`)
- Description: link any related issue or discussion
- All CI checks green (`verify.yml`)
- Linear history required (`git merge --ff-only` or `git rebase`)

## CI gates (enforced on every PR)

`verify.yml` must pass. Currently enforced:

- `cargo fmt --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `cargo audit` (ubuntu)
- `pnpm typecheck`

Deferred (tracked, pending CI runner fixes):
- `pnpm -r test` / `pnpm -r test:coverage` (vitest hangs on CI runners)
- `cargo llvm-cov` (instrumented tests crash on CI)
- Run `./scripts/verify_acceptance.sh` locally for the full gate including these.

New code paths must include tests in the existing `_tests.rs` companion-module pattern (`#[cfg(test)] mod xxx_tests`).

For packaging-script changes, also add or extend a smoke test under `tests/packaging/macos/` or `tests/scripts/`.

## Commit messages

Conventional Commits, short subject + body when rationale is non-obvious:

```
feat(prompt-palette): add keyboard shortcut overlay

Bindings: cmd+k opens palette, esc closes, arrows navigate, enter applies.
Existing mouse interactions preserved.
```

## Licensing

By contributing, you agree your contributions are licensed under the project's Apache-2.0 license (see `LICENSE`).
