# Pi SDK Bundle Spike — Result

**Date:** 2026-06-25
**SDK version:** `@earendil-works/pi-coding-agent@0.80.2` (resolved from `latest`)
**Node:** v25.9.0 · **pnpm:** v10.33.2 · **esbuild:** 0.23.1 · **vitest:** 2.1.9

## Outcome

- [x] PASS — `createAgentSession` bundles and is callable.
- [ ] FAIL

The brief's acceptance criteria (Step 5) are both met:
1. `pnpm build` produces `dist/spike.bundle.js` (13.3mb, no errors).
2. `pnpm test` passes (1 test, 52ms) — `createAgentSession` is imported from
   `@earendil-works/pi-coding-agent`, called with `{ model, workingDir }`, and
   returns a session object.

## Install note

The `spike/` directory is intentionally NOT a member of the root pnpm workspace
(`pnpm-workspace.yaml` lists only `apps/gui`, `apps/pi-sidecar`, `packages/*`).
A bare `pnpm install` from the spike dir is intercepted by the workspace and
reports "Already up to date" without installing the SDK. Install must be run
with `--ignore-workspace` so the spike gets its own `node_modules/`:

```bash
cd apps/pi-sidecar/spike
pnpm install --ignore-workspace
```

## Evidence

### `pnpm install --ignore-workspace` (tail)

```
Packages: +173
dependencies:
+ @earendil-works/pi-coding-agent 0.80.2

devDependencies:
+ esbuild 0.23.1 (0.28.1 is available)
+ typescript 5.9.3 (6.0.3 is available)
+ vitest 2.1.9 (4.1.9 is available)
Done in 29.7s using pnpm v10.33.2
```

### `pnpm build` (tail)

```
> @busytok/pi-sidecar-spike@0.0.0 build
> node esbuild.config.mjs

  dist/spike.bundle.js  13.3mb ⚠️

⚡ Done in 636ms
spike bundle written to dist/spike.bundle.js
```

### `pnpm test` (tail)

```
> @busytok/pi-sidecar-spike@0.0.0 test
> vitest run

 RUN  v2.1.9 .../apps/pi-sidecar/spike

 ✓ tests/spike.test.ts (1 test) 52ms

 Test Files  1 passed (1)
      Tests  1 passed (1)
   Start at  00:48:39
   Duration  1.80s
```

## De-risking finding (does NOT fail the spike, but matters for Plan 4)

The vitest test runs the SDK via Node's native ESM loader (vitest transforms
`src/spike.ts` directly), which is why it passes cleanly. Running the esbuild
**bundle** directly, however, fails at startup:

```bash
$ node dist/spike.bundle.js
file:///.../dist/spike.bundle.js:11
  throw Error('Dynamic require of "child_process" is not supported');
        ^
Error: Dynamic require of "child_process" is not supported
    at .../dist/spike.bundle.js:11:9
    at node_modules/.pnpm/cross-spawn@7.0.6/node_modules/cross-spawn/index.js ...
```

Cause: `cross-spawn@7.0.6` (a transitive dependency of the SDK) uses CommonJS
`require('child_process')`. esbuild's `format: 'esm'` output wraps CJS modules
with a shim that throws on dynamic `require` of Node built-ins. The bundle
**builds** fine (esbuild doesn't execute the code), and the SDK is **callable**
in a real ESM context (proven by the test) — but the ESM bundle cannot be
executed directly via `node dist/spike.bundle.js` for code paths that touch
`cross-spawn`.

This is NOT one of the brief's acceptance criteria (build + test), so the spike
PASSES. It is, however, exactly the kind of bundling risk the spike exists to
surface.

## Implications for Plan 4

- **PASS:** Plan 4 can wire `createAgentSession` directly into `session.turn_auto`.
  The SDK's public API is stable and callable in a Node ESM context.

- **Bundling caveat for Plan 4:** When Plan 4 bundles the real sidecar with the
  Pi SDK pulled in, it must NOT rely on directly executing an `format: 'esm'`
  bundle for code paths that reach `cross-spawn`. Recommended mitigations
  (pick one):
  1. Switch the sidecar bundle to `format: 'cjs'` (simplest; `cross-spawn`'s
     `require` works natively).
  2. Keep `format: 'esm'` but add `cross-spawn` (and any other CJS deps that
     `require` built-ins) to esbuild's `external` array, and let Node resolve
     them at runtime from `node_modules`.
  3. Avoid the `cross-spawn` code path at session-init time (only triggered
     when the SDK spawns a subprocess).

- The Task 4 sidecar (mock `turn_auto`) is untouched by this spike and remains
  the source of truth for the JSON-RPC protocol. The Rust-side process
  management (Tasks 1–3, 5–7) is fully validated end-to-end regardless.
