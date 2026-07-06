# Dynamic API format + base URL per subagent session

**Status:** draft  
**Date:** 2026-07-04  
**Requires:** Phase 1–5 (merged)

## Motivation

Current state (Phase 5):

- `ProviderKind` only has `OpenAiCompatible`
- `inject_provider_env()` hardcodes `OPENAI_API_KEY` + `OPENAI_BASE_URL`
- Sidecar `defaultSessionFactory` passes neither `baseUrl` nor `apiKey` to
  `createAgentSession()`
- Pi SDK resolves endpoint/auth entirely from its built-in catalog +
  env vars

This blocks:

1. Anthropic-format providers (DeepSeek Anthropic endpoint, Claude API, ...)
2. OpenAI-compatible proxies with custom URLs (OpenRouter, LiteLLM, ...)
3. Different API keys / base URLs on successive subagent invocations within
   the same sidecar lifetime

## Pi SDK facts (discovered 2026-07-03)

`CreateAgentSessionOptions` does **not** accept `apiKey` or `baseUrl`:

```typescript
// pi-coding-agent/dist/core/sdk.d.ts
export interface CreateAgentSessionOptions {
    cwd?: string;
    agentDir?: string;
    authStorage?: AuthStorage;       // controls apiKey resolution
    modelRegistry?: ModelRegistry;   // controls model catalog + baseUrl
    model?: Model<any>;              // carries api, provider, baseUrl
    thinkingLevel?: ThinkingLevel;
    tools?: string[];
    // ... (no apiKey, no baseUrl)
}
```

`baseUrl` lives on `Model<TApi>`:

```typescript
// pi-ai/dist/types.d.ts
export interface Model<TApi extends Api> {
    id: string;
    api: TApi;           // "openai-completions" | "anthropic-messages" | ...
    provider: ProviderId; // "deepseek" | "openai" | ...
    baseUrl: string;      // the endpoint URL
    // ...
}
```

`apiKey` is resolved by `AuthStorage` with three layers:

1. `AuthStorage.setRuntimeApiKey(provider, key)` — runtime override
2. `auth.json` — persisted credentials
3. Environment variables (`DEEPSEEK_API_KEY`, `OPENAI_API_KEY`, ...)

`ModelRegistry` loads built-in + custom models. Custom providers defined via
`models.json` (`~/.pi/agent/models.json`) with schema:

```typescript
// ProviderConfigSchema (model-registry.js)
{
    name?: string;
    baseUrl?: string;      // API endpoint
    apiKey?: string;       // supports $VAR / !cmd interpolation
    api?: string;          // "openai-completions" | "anthropic-messages" | ...
    headers?: Record<string, string>;
    compat?: { ... };
    authHeader?: boolean;
    models: Array<{
        id: string;
        name?: string;
        reasoning?: boolean;
        contextWindow?: number;  // required for non-builtin
        maxTokens?: number;      // required for non-builtin
        cost?: { input, output, cacheRead, cacheWrite };
        // ...
    }>;
    modelOverrides?: Record<string, { ... }>;
}
```

`ModelRegistry.create(authStorage, modelsJsonPath)` accepts an explicit
per-call path — not limited to `~/.pi/agent/models.json`.

## Design

### Approach: per-sidecar `models.json` + `AuthStorage.setRuntimeApiKey()`

Rationale over alternatives:

| Approach | Verdict |
|----------|---------|
| Clone builtin Model + override baseUrl | Works for OpenAI-compat but can't switch `api` format (Anthropic path is fundamentally different in Pi SDK) |
| env-var passthrough to `createAgentSession` | SDK doesn't accept `apiKey`/`baseUrl` at session level |
| **per-sidecar `models.json` + runtime auth** | Works for both API formats, uses documented Pi SDK API, no concurrency issues (temp dir per process) ✅ |

Flow per `turn_auto`:

```
Rust service                     Sidecar (TS)
───────────                      ────────────
1. resolve provider entry        4. receive TurnAutoParams
   { api_key, base_url, api }      { provider_id, model, api, base_url, api_key }

2. inject env vars               5. generate /tmp/busytok-sidecar-XXXXX/models.json:
   BUSYTOK_PROVIDER_API_KEY         { "providers": { "<provider_id>": {
   BUSYTOK_PROVIDER_BASE_URL            "baseUrl": "<base_url>",
   BUSYTOK_PROVIDER_API                 "api": "<api>",
                                        "apiKey": "$BUSYTOK_PROVIDER_API_KEY",
                                        "models": [{ "id": "<model>" }]
                                    }}}

3. dispatch turn_auto RPC        6. ModelRegistry.create(authStorage, tmpModelsJsonPath)
   with new fields               7. AuthStorage.inMemory() + setRuntimeApiKey(provider_id, key)
   ↓                             8. createAgentSession({ modelRegistry, authStorage, model })
                                 9. business as usual ...
                                10. cleanup /tmp/busytok-sidecar-XXXXX/ on close
```

### Key design decisions

**1. `models.json` per session, not per process.** Each `turn_auto` can
receive different `api`, `base_url`, `api_key`. The sidecar regenerates
`models.json` in its private temp dir and creates a fresh `ModelRegistry`
pointing at it. Multiple sessions with different configs coexist safely in
the same sidecar process.

**2. API key via env var indirection, not inline.** `models.json`'s
`apiKey: "$BUSYTOK_PROVIDER_API_KEY"` tells Pi SDK to read `process.env`.
This keeps the key out of filesystem entirely — only in-memory env vars.

**3. `contextWindow` / `maxTokens` from builtin catalog when available.**
If the requested model ID matches a builtin model, copy its capability
fields into `models.json`. This avoids requiring the user to specify
context window and max tokens for well-known models (deepseek-chat,
claude-sonnet-4-5, etc.). For truly unknown models, these fields are
required in `models.json` — we can either:
- Derive reasonable defaults from the model name pattern
- Add optional fields to the provider config

**4. No `ProviderKind` enum expansion needed yet.** The `api` field on the
provider config is a free-form string matching Pi SDK's `Api` type
(`"openai-completions" | "anthropic-messages" | ...`). We don't need a Rust
enum because the sidecar is the consumer — Rust just passes it through.
However, the GUI/settings validation layer should validate against known
values.

### Negative — what this does NOT do

- Does NOT support OAuth providers (subscription-based auth)
- Does NOT support `openai-responses`, `google-generative-ai`, or `mistral-conversations` in MVP (blocked on Pi SDK catalog, not our code)
- Does NOT persist `models.json` across sidecar restarts (stateless by design)

## Implementation tasks

### Task 1 — Rust: extend `ProviderRuntimeEntry` and `inject_provider_env`

**File:** `crates/busytok-subagent/src/sidecar/pool.rs`

```rust
pub struct ProviderRuntimeEntry {
    pub provider_id: String,
    pub api_key: String,
    pub base_url: String,
+   pub api: String,  // "openai_compatible" | "anthropic_messages" | ...
}

pub fn inject_provider_env(env: &mut HashMap<String, String>, entry: &ProviderRuntimeEntry) {
-   env.insert("OPENAI_API_KEY".to_string(), entry.api_key.clone());
-   env.insert("OPENAI_BASE_URL".to_string(), entry.base_url.clone());
+   env.insert("BUSYTOK_PROVIDER_API_KEY".to_string(), entry.api_key.clone());
+   env.insert("BUSYTOK_PROVIDER_BASE_URL".to_string(), entry.base_url.clone());
+   env.insert("BUSYTOK_PROVIDER_API".to_string(), entry.api.clone());
}
```

- Rename env vars to `BUSYTOK_PROVIDER_*` to avoid collision with Pi SDK's
  own env var reading (e.g. `OPENAI_API_KEY` would leak credentials across
  providers).
- Source of `api`: user's provider config (new field, see Task 4).

### Task 2 — Rust: thread `api` through from config to pool

**Files:**

- `crates/busytok-subagent/src/sidecar/pool.rs` — `ProviderRuntimeEntry`
  population sites (currently read from SQL `providers` table)
- `crates/busytok-config/src/providers.rs` / `crates/busytok-domain/src/provider_catalog.rs` —
  add `api` field to relevant structs

The `api` field defaults to `"openai_compatible"` when absent (backward
compat with v0.0.8 configs).

### Task 3 — TypeScript: extend `TurnAutoParams` and `CreateSessionOpts`

**File:** `apps/pi-sidecar/src/types.ts`

```typescript
export interface TurnAutoParams {
    // ... existing fields ...
+   /** API format: "openai_compatible" → "openai-completions", etc. */
+   api?: string;
+   /** Override base URL (provider endpoint). */
+   base_url?: string;
+   /** Override API key (provider credential). */
+   api_key?: string;
}
```

**File:** `apps/pi-sidecar/src/pi_session.ts`

```typescript
export interface CreateSessionOpts {
    cwd: string;
    model?: string;
    provider_id?: string;
    tools?: string[];
+   api?: string;         // Pi SDK api value
+   base_url?: string;    // override base URL
+   api_key?: string;     // override API key
}
```

### Task 4 — TypeScript: implement per-session `models.json` generation

**File:** `apps/pi-sidecar/src/pi_session.ts` — new helper module or
inline in `defaultSessionFactory`

Pseudocode:

```typescript
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

interface DynamicProviderConfig {
  providerId: string;
  baseUrl: string;
  api: string;        // e.g. "openai-completions"
  apiKeyEnv: string;  // env var name the SDK reads from $ interpolation
  modelId: string;
}

function createDynamicModelRegistry(
  config: DynamicProviderConfig,
): { registry: ModelRegistry; cleanup: () => void } {

  // 1. create temp dir (lifetime = sidecar process)
  const dir = mkdtempSync(join(tmpdir(), 'busytok-sidecar-'));

  // 2. write models.json
  const modelsJson = {
    providers: {
      [config.providerId]: {
        baseUrl: config.baseUrl,
        api: config.api,
        apiKey: `$${config.apiKeyEnv}`,  // $BUSYTOK_PROVIDER_API_KEY
        models: [{ id: config.modelId }],
      },
    },
  };
  writeFileSync(join(dir, 'models.json'), JSON.stringify(modelsJson));

  // 3. create authStorage with runtime key override
  const apiKey = process.env[config.apiKeyEnv];
  const authStorage = AuthStorage.inMemory();
  if (apiKey) {
    authStorage.setRuntimeApiKey(config.providerId, apiKey);
  }

  // 4. create registry pointing at temp models.json
  const registry = ModelRegistry.create(authStorage, join(dir, 'models.json'));

  return {
    registry,
    cleanup: () => rmSync(dir, { recursive: true, force: true }),
  };
}
```

### Task 5 — TypeScript: wire into `defaultSessionFactory` and `turnAutoHandlerWithPool`

**File:** `apps/pi-sidecar/src/pi_session.ts`

When `opts.base_url` and/or `opts.api_key` are present:

1. Generate per-call `ModelRegistry` with dynamic config (Task 4)
2. Use `AuthStorage.setRuntimeApiKey()` for the API key
3. Pass both to `createAgentSession({ modelRegistry, authStorage, model })`
4. Track cleanup handles; dispose when session closes

**File:** `apps/pi-sidecar/src/handlers/turn_auto.ts`

Thread `api`, `base_url`, `api_key` from `TurnAutoParams` into
`CreateSessionOpts` when calling `pool.ensure()`.

### Task 6 — Rust: add `api` field to `ProviderConfig` (settings.toml)

**File:** `crates/busytok-config/src/providers.rs`

Add optional `api` field to `ProviderConfig`:

```rust
pub struct ProviderConfig {
    // ... existing fields ...
    /// Pi SDK API format. When None, defaults to "openai_compatible"
    /// which maps to "openai-completions" in Pi SDK vocabulary.
    pub api: Option<String>,
}
```

Serialization: rename user-facing `openai_compatible` → Pi SDK
`openai-completions` (the mapping from our `ProviderKind` naming to Pi
SDK's internal vocabulary).

### Task 7 — protocol: extend `ProviderDto` / `CreateProviderRequest` etc.

**File:** `crates/busytok-protocol/src/dto.rs`

Add `api` field to provider-related DTOs so the GUI can read/write it.

## Migration

- Existing provider configs without `api` → default to `"openai_compatible"`
  (→ Pi SDK `"openai-completions"`)
- Existing env var names (`OPENAI_API_KEY`, `OPENAI_BASE_URL`) → replaced by
  `BUSYTOK_PROVIDER_*` on the next release. Old names were never relied on
  by the sidecar (Pi SDK ignored them for built-in providers).

## Verification

1. Unit: `defaultSessionFactory` with `base_url` + `api_key` → Pi SDK calls
   correct endpoint
2. E2E: `busytok delegate --profile pi/search-cheap` against
   `DEEPSEEK_API_KEY` + `https://api.deepseek.com/v1` + `openai_compatible`
3. E2E: same against `https://api.deepseek.com/anthropic` +
   `anthropic_messages`
4. E2E: two concurrent `delegate` with different providers in same sidecar
   process
5. Smoke: existing DMG flow (no api field) still works (backward compat)
