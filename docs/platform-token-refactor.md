# Platform personal-API-token refactor — removing SDK login from `craftsman-rs`

> **Transport retirement (November 11):** references to `serve.rs`,
> `desktop_serve.rs` and their command/protocol surfaces below are historical.
> The owner removed those servers; ACP stdio and its WebSocket bridge remain.
> This does not change the retained token, provider, trace or attribution contracts.

**Status:** IMPLEMENTED (Phases 1 and 2 landed; bearer-semantics verification
pending a minted key, §9.2)
Date: 2026-10-16 · Owner: `craftsman-harness` · Target release: next minor (breaking)
**Verified against:** the platform's aggregate OpenAPI spec
(`web.int.craftsman.ai/platform-docs` → `/openapi.json`, pulled 2026-10-16).

---

## 1. Summary

The platform now issues **personal API keys** (`auth` app: `api-keys/mint`,
`rotate`, `revoke`, `list`; `expires_in_days` nullable → long-lived/non-
expiring keys supported, rotation handled on the platform side). `craftsman-rs`
currently reaches the platform through `craftsman-sdk`-mediated login
(email + password → rotating JWT in `~/.craftsman/config.json`, with refresh logic
in every serve mode). This refactor replaces that with a **headless,
token-configured** model:

- **LLM access** — the platform's token endpoint is **OpenAI-compatible**
  (`POST /apps/llm/api/v1/chat/completions`, `GET /apps/llm/api/v1/models`),
  so craftsman-harness treats it exactly like any OpenAI-compatible provider: existing
  direct route, personal token as `api_key`, **no platform detection anywhere
  on the LLM path**. The SDK, the platform route branch in `route.rs`, and
  the JWT lifecycle are deleted.
- **Platform MCP** — `/mcp` is the entire platform served as a single MCP
  server (27 `<app>__<action>` tools, RBAC-filtered to the caller). It is an
  ordinary `mcp_servers` config entry (URL + static bearer header) — no
  special-casing, no `for_platform()`. The MCP client itself is rewritten to
  the modern **stateless** `2026-07-28` protocol (§4.6); the legacy
  initialize/session era is dropped.
- **Skills** — the only harness-facing platform feature in scope — move to a
  thin, hand-rolled REST client hitting the published paths
  (`GET /apps/skills/api/skill/`, `GET /apps/skills/api/skill/{id}/download`)
  with the static token. No manifest indirection is needed.
- **Out of scope, deleted** (see §4.7): platform delegated agents
  (`agents` app), platform pricing metadata (`llm` app `get_pricing`),
  `craftsman apps|app|invoke` CLI. The inbox poller is untouched.
- The **`craftsman-sdk` dependency is removed entirely**, which also removes the
  private-git-credential build requirement (`install.sh`, CI,
  `.cargo/config.toml`).

Token expiry/rotation is handled **on the platform side** (`api-keys/rotate`):
the CLI treats the token as a static credential. When it rotates, the user
updates one config value (or one environment variable). No refresh code ships
in the CLI.

## 2. Goals / non-goals

**Goals**

1. `craftsman-rs` works as a headless coding agent with only config-file (or env)
   credentials; no interactive login anywhere.
2. Delete the JWT lifecycle (login, refresh, `ensure_fresh`, per-turn
   session-rebuild-on-rotation) across all execution modes.
3. Platform MCP behaves exactly like any third-party MCP server.
4. Keep platform skills browsing/install over plain REST with the static
   token.
5. Drop the `craftsman-sdk` git dependency.

**Non-goals**

- No new platform features or MCP transports (stdio MCP is separate work).
- No changes to `craftsman-harness` (framework is untouched; `dep_guard` green).
- No desktop/Protocol v4 *new* frames; v4 stays frozen (§6.4).
- No token-refresh logic in the CLI — rotation is the platform's/user's
   responsibility via `api-keys/rotate`.
- No legacy-era MCP fallback — the client speaks the modern `2026-07-28`
  protocol only (§4.6).
- Delegated agents / pricing / apps-CLI are *deleted*, not migrated.

## 3. Current state (what is being removed and why it exists)

| Concern | Location | Depends on login because… |
|---|---|---|
| Login (`auth` app `login_basic`) | `crates/craftsman-rs/src/platform.rs:23-44` | produces a short-lived JWT |
| JWT refresh (`token_refresh`, `ensure_fresh`) | `platform.rs:92-181` | JWTs expire (2h refresh age / 300s margin) |
| Route: platform branch + manifest probe | `crates/craftsman-rs/src/route.rs:26-96` (`App::openai_base_url`, one-slot `ROUTE_CACHE`) | discovers the `llm` base URL via SDK; requires JWT |
| Per-turn `refresh_auth` + session rebuild | `serve.rs:171,196-208,318-334`; `desktop_serve.rs:1031-1055`; `acp_serve.rs:130`; `service_agent.rs:344`; `tui_repl.rs:1615`; `main.rs:340` | the JWT is baked into three `LlmClient`s per session (`agent.rs:823,945-960,1049-1060`) and can rotate mid-session |
| Platform MCP (`for_platform`) | `crates/craftsman-rs/src/mcp.rs:36-44,292-317` | derives `{platform}/mcp` and injects the live JWT |
| Skills list/download (`skills` app) | `platform_skills.rs:156-219` | JWT bearer on manifest/invoke |
| Delegated agents (`agents` app) | `platform_agents.rs:36-44` | JWT bearer — **deleted, not migrated** |
| Model pricing (`llm` app `get_pricing`) | `agent.rs:1254-1281` | JWT bearer — **deleted, not migrated** |
| Inbox claim/settle | `inbox_poller.rs:174-266` | already raw REST, keyed on `auth_token` — **re-pointed only** |
| Login surfaces (CLI/REPL/TUI/wizard/desktop) | `main.rs:94-115,170-191`; `repl.rs:509-571` (file deleted in P1); `tui_repl.rs:4726-5330`; `desktop_serve.rs:835-875,1490-1547` | email/password flow |
| SDK dependency | `crates/craftsman-rs/Cargo.toml:27` (`craftsman-sdk` git tag `rust-v0.1.0`) | supplies `CraftsmanClient`/`App::invoke`/`normalize_platform_url` |

The SDK's *used* surface is tiny: `CraftsmanClient::{new, set_token, apps, app}`,
`CraftsmanClientOptions`, `App::{invoke, openai_base_url, id, version, actions}`,
`Error::Http`, and `normalize_platform_url` (5 call sites). Everything else in
`platform.rs` is local code, not SDK code.

## 4. Target design

### 4.1 Config shape

`~/.craftsman/config.json` (breaking; see §6.5 for migration):

```json
{
  "base_url": "https://platform.example.com/apps/llm/api/v1",
  "api_key": "mpat_xxx",
  "model": "my-model",

  "mcp_servers": [
    {
      "name": "craftsman-platform",
      "url": "https://platform.example.com/mcp",
      "headers": { "Authorization": "Bearer ${CRAFTSMAN_PLATFORM_TOKEN}" }
    }
  ],

  "platform": {
    "base_url": "https://platform.example.com",
    "api_token": "mpat_xxx"
  }
}
```

Field semantics:

| Field | Meaning | Consumers |
|---|---|---|
| `base_url` + `api_key` | The *only* LLM config — existing direct route, **unchanged code path**. The platform is a stock OpenAI-compatible provider here: `api_key` holds the personal token, `base_url` points at `{platform}/apps/llm/api/v1` (verified surface: `v1/chat/completions`, `v1/models`). Nothing distinguishes it from an OpenAI/LiteLLM/NVIDIA entry. | `route.rs`, `llm.rs`, `platform.rs::list_models` direct branch |
| `mcp_servers[]` | Ordinary MCP entries. `/mcp` serves the **whole platform** as one MCP server (verified: 27 `<app>__<action>` tools, RBAC-filtered); `for_platform()` is deleted, the entry is not special-cased. | `mcp.rs::for_config_server` |
| `platform.base_url` | Platform root (normalized by a re-homed `normalize_platform_url`). | skills REST client, `inbox_poller.rs` |
| `platform.api_token` | The personal API key. Static; rotation happens platform-side (`api-keys/rotate`). | skills REST client, `inbox_poller.rs` |

**Removed fields:** `auth_token`, `refresh_token`, `auth_token_expires_at`,
`auth_provider`, `user_id`, `tenant_id`, `user_email`, `platform_base_url`.

**Environment-variable interpolation (new, small):** string values of `api_key`
and MCP server `headers` support `${ENV_VAR}` expansion resolved at load time.
This lets a single `CRAFTSMAN_PLATFORM_TOKEN` environment variable back all three
uses (LLM `api_key`, MCP bearer header, `platform.api_token`) instead of three
copies of a rotating secret in the file. Interpolation is limited to those
fields on purpose — blanket expansion would be surprising.

`platform.api_token` itself also falls back to `$CRAFTSMAN_PLATFORM_TOKEN` when
empty, so the file can stay secret-free:

```json
"platform": { "base_url": "https://platform.example.com", "api_token": "" }
```

### 4.2 Route resolution (`route.rs`)

- The platform branch, `ROUTE_CACHE`, `invalidate_route_cache()`, and
  `uses_platform_llm()` are **deleted**. The direct route is the only route:
  `base_url` + `api_key` (+ `model`).
- `route.rs` shrinks to the direct branch only.
- `platform.rs::list_models` loses its platform branch too: model listing is
  `GET {base}/models` with the `api_key` bearer — the identical request craftsman
  already sends to OpenAI/LiteLLM/NVIDIA, and a verified endpoint
  (`/apps/llm/api/v1/models`). The platform model picker behaves exactly
  like the direct-provider picker.
- Downstream effect: harness/provider profiles lose `uses_platform_llm()` as a
  signal. `harness_profiles::model_identity` already infers the family from the
  model name / `provider:model` spec, so users of platform-hosted models keep
  correct profile inference by naming the model. `examples/config/platform.json`
  is updated to the new shape.

### 4.3 Skills REST client (`platform_rest.rs`, new; skills only)

A small hand-rolled client modeled on the `inbox_poller.rs` pattern (plain
`reqwest` + bearer). Scope: **skills only** — the two operations the SDK
performed today, whose action names map 1:1 onto published REST paths in the
platform OpenAPI spec:

| SDK action (today) | Published REST path (verified) | Used for |
|---|---|---|
| `list_skill` | `GET /apps/skills/api/skill/` | `/skills` browser catalog |
| `skill_download` | `GET /apps/skills/api/skill/{id}/download` | tar.gz download info (url + sha256) |

```rust
pub struct PlatformRest { base_url: String, token: String, http: reqwest::Client }

impl PlatformRest {
    pub fn from_config(cfg: &Config) -> Option<Self>;   // None when unconfigured
    pub async fn list_skills(&self) -> Result<Vec<SkillRecord>>;
    pub async fn skill_download(&self, id: &str) -> Result<DownloadInfo>;
    pub fn normalize_url(url: &str) -> Result<String>;  // re-homed normalize_platform_url
}
```

`platform_skills.rs::SkillsClient` swaps its transport from
`sdk_client(cfg).app("skills")` to this client; the sha256-verified atomic
install pipeline (`install_from_bytes`, `.craftsman-skill.json` marker, remove) is
unchanged. Blob download stays a plain unauthed `GET` of the returned URL.

**No manifest indirection.** The SDK resolved action paths via
`GET /apps/{id}/manifest`; that machinery is dropped. The published OpenAPI
paths are the contract (§6.3).

### 4.4 Deletions across serve modes

Because the token is static, the "rebuild session on rotation" logic — which
exists solely because a JWT was baked into three `LlmClient`s per session —
disappears:

- `serve.rs`: `refresh_auth` per-turn path (197) and rebuild-on-rotation
  deleted. `refresh_agents` (318-334) is **deleted entirely** (no platform
  agents); `Session.shared_agent_revision` removed.
- `desktop_serve.rs`: `refresh_auth` (1031-1055) deleted; `signed_in` snapshot
  fields become `platform_configured: !platform.api_token.is_empty()`.
- `acp_serve.rs`, `service_agent.rs`, `tui_repl.rs`, `main.rs` one-shot:
  `ensure_fresh` calls deleted (no-op when unconfigured).

### 4.5 Compaction trigger & cost metadata

`fetch_model_pricing` (agent.rs:1254-1281) is **deleted** (platform pricing
app out of scope). The token-fraction compaction trigger gets a config
fallback; resolution order:

1. `model_profiles.<model>.context_window` (new field), or
2. message-count fallback (current behavior for direct configs).

The USD cost footer reads `n/a` when no rates are configured — acceptable for
a headless CLI. (A `pricing` config field can be added later without a config
break.)

### 4.6 MCP client: modern protocol only (`2026-07-28`)

`mcp.rs` is rewritten to the modern **stateless** protocol; the legacy
initialize/session era is dropped (deprecated platform-side as of
**July 28, 2027**):

- No `initialize` handshake, no `mcp-session-id`; each request is standalone.
- Methods: `server/discover`, `tools/list`, `tools/call`.
- Every request carries headers `MCP-Protocol-Version: 2026-07-28` and
  `Mcp-Method: <method>`; `tools/call` additionally carries
  `Mcp-Name: <tool>`.
- Every request body embeds the required per-request `_meta`
  (`io.modelcontextprotocol/clientCapabilities`,
  `io.modelcontextprotocol/clientInfo`, and
  `io.modelcontextprotocol/protocolVersion: "2026-07-28"`) — the modern
  replacement for session-establishment state.
- Responses remain JSON or SSE; `401` maps to a clear "invalid or missing
  token" error.
- Per-server failure handling is unchanged: errors are collected and
  reported, never fatal to the session.

This applies to **all** streamable-HTTP config servers, not just the
platform entry — the modern era is the latest standard, and a legacy-era
server fails with a clear error rather than a silent skip. A legacy
fallback is deliberately not built. Tool names surfacing in the registry
will be `<app>__<action>` (e.g. `customer-feedback__submit_feedback`),
RBAC-filtered to the token's role — worth a line in user docs.

### 4.7 Scope deletions (breaking)

| Deleted | Today | Consequence |
|---|---|---|
| Platform delegated agents | `platform_agents.rs` + wiring in `agent.rs:957-1038`, `serve.rs::refresh_agents` | Only the built-in `general-purpose` subagent remains; AGENTS.md's delegated-agents note becomes obsolete and must be updated |
| Platform pricing | `agent.rs::fetch_model_pricing` | Cost footer `n/a`; compaction via `context_window` config (§4.5) |
| `craftsman apps` / `app` / `invoke` CLI | `main.rs:105-115,202-240` | Generic SDK app-manifest surface goes with the SDK; `PlatformRest` covers skills only |
| JWT fields + login UI | `platform.rs`, REPL/TUI/desktop login surfaces | Headless token config; desktop `configure` frame keeps writing config (§6.4) |

`inbox_poller.rs` (workspace-pod deployment path) is **unchanged except** for
reading `platform.api_token` instead of `auth_token` — it already re-reads
config per HTTP call.

### 4.8 What is intentionally unchanged

- `llm.rs` — pure OpenAI-compatible client; never touched the SDK.
- `mcp.rs::for_config_server` and the MCP merge path in `agent.rs` — the
  platform is just another config server. MCP tools remain outside
  `GATED_TOOLS`/`NETWORK_TOOLS`, as today.
- `craftsman-harness` — zero changes; `dep_guard` stays green.
- Skills install/verification pipeline — only the transport changes.
- HITL, permissions, subagent gating — untouched; the shared HITL/tool
  filtering boundary is unaffected. Delegated-agent compilation is removed
  wholesale (not partially relaxed), so no new gap opens.

## 5. Build/packaging effects

- `crates/craftsman-rs/Cargo.toml`: remove `craftsman-sdk` (git, tag `rust-v0.1.0`).
- `.cargo/config.toml`: `git-fetch-with-cli` entry becomes unnecessary;
  remove when the last private git dep is gone.
- `install.sh` / CI / README / `AGENTS.md`: drop the git-credential
  requirement for `github.com/trend-mgai/craftsman-sdk` (AGENTS.md "Gotchas"
  bullets 1-2 become obsolete; `./install.sh --no-locked` semantics simplify —
  no pinned SDK tag to worry about). AGENTS.md's delegated-agents paragraph is
  removed with the feature.
- New `examples/config/platform.json` reflecting the token shape (and updated
  `direct-*.json` if field names change).

## 6. Design decisions and tradeoffs

### 6.1 The platform LLM endpoint is a stock OpenAI-compatible provider

The platform's token endpoint is OpenAI-compatible by definition
(`v1/chat/completions` + `v1/models` in the public spec), so craftsman-harness treats
it as a stock OpenAI-compatible provider: no platform detection, no manifest
probe on the LLM path, the same client and `GET /models` listing as
OpenAI/LiteLLM/NVIDIA. Users set `base_url` explicitly (identical to the other
example configs). If the platform ever changes the URL shape, users change
one config line — the same contract as every other OpenAI-compatible
provider.

### 6.2 Skills stay REST, not MCP

MCP is the *model's* tool channel. The `/skills` browser is *harness-facing*:
it needs structured catalog records before any model runs. Putting skills
behind MCP would mean the agent invoking tools to configure its own harness.
Hence the thin REST client. MCP stays purely model tools. This is also the
*only* harness-facing platform feature left in scope — everything else either
rides MCP or is deleted.

### 6.3 Published OpenAPI paths replace the SDK manifest indirection

The SDK's `GET /apps/{id}/manifest` indirection existed to resolve action
routes at runtime. The platform now publishes an aggregate OpenAPI spec, and
the two skill operations we need are stable, documented REST paths whose
summaries match the old SDK action names exactly (`list_skill`,
`skill_download`). Hardcoding two published paths is simpler and the spec is
the contract. If the platform renames them, it's a spec break — pinned and
versioned like any other REST client dependency.

### 6.4 Desktop protocol v4 stays frozen, semantics only

No new frames. The existing `configure` frame already supports the direct
route (`apply_provider_route` route `"direct"`); the platform route becomes
"write `platform.base_url` + `platform.api_token` + derived `base_url`". The
`login`/`logout` frame handlers become thin wrappers over the same config
writes (or return a deprecation error, coordinated with the desktop app
release). The desktop app must ship a coordinated update since this is a
breaking change for password-login users.

### 6.5 Migration: breaking, with a grace warning

- Old config fields round-trip verbatim today (serde `extra` flatten), so old
  files still load; nothing crashes.
- On startup, if legacy fields (`auth_token`/`refresh_token`/`user_email`) are
  present, print a one-time notice: login was removed; mint an API key
  (`auth` app `api-keys/mint`), set `platform.api_token` (or
  `CRAFTSMAN_PLATFORM_TOKEN`) and `base_url`.
- Do **not** auto-migrate JWTs — they are short-lived and the wrong credential
  type. Users provision a personal key.
- `craftsman setup` (TUI wizard) becomes the onboarding path: platform URL →
  paste token → verify (`list_models` via the OpenAI-compatible endpoint) →
  pick model → write config (including the `craftsman-platform` MCP entry).
  `handle_setup_direct` is the template.

### 6.6 Token storage

- `~/.craftsman/config.json` already has 0700 dir / 0600 file perms (`config.rs`).
- Env-var interpolation (§4.1) is the secret-free option for hardened setups.
- No keychain integration in this refactor (can be added later behind
  `platform.api_token: "keychain:"` without a config break).

## 7. Sequencing

**Phase 1 — token mode + JWT deletion (~1 week)**

1. Config: add `platform{base_url, api_token}` + `${ENV}` interpolation for
   `api_key`/MCP headers; remove legacy fields; legacy warning.
2. `route.rs`: delete platform branch/cache; direct-only.
3. Delete `platform.rs` login/refresh/`ensure_fresh`; delete per-turn
   `refresh_auth`/rebuild logic across serve modes.
4. Delete `for_platform()`; delete `platform_agents.rs` + wiring +
   `refresh_agents`; delete `fetch_model_pricing` platform path; add
   `context_window` fallback; delete `apps|app|invoke` CLI.
5. `craftsman setup` wizard token flow; remove `/login` `/logout` `/connect`,
   `LoginModal`, `craftsman login` CLI.
6. Update `examples/config/platform.json`; README/AGENTS.md notes.

**Phase 2 — skills REST client + modern MCP + SDK removal (~1 week)**

1. `platform_rest.rs` (skills list/download + `normalize_url`); migrate
   `SkillsClient`; re-point `inbox_poller` token field.
2. Rewrite `mcp.rs` to the modern stateless protocol (§4.6):
   `server/discover` → `tools/list` → `tools/call`, per-request `_meta` +
   headers; delete `initialize`/`mcp-session-id`.
3. Remove `craftsman-sdk` dep; clean `.cargo/config.toml`, `install.sh`, CI.
4. Test suite updates (§8).

Phases land as two PRs; between them platform skills briefly regress (Phase 1
drops the SDK transport before Phase 2 lands REST). If that window must not
exist on `main`, land 2.1 first as a standalone PR.

**Post-landing verification (not blocking):** with a minted API key, curl
`GET /apps/skills/api/skill/` and a `tools/list` against `/mcp` to confirm
bearer semantics and the key's RBAC role (§9.2). If minted keys are *not*
accepted as bearers there, the fix is platform-side (or a documented
mint-endpoint change) — the client design is unaffected either way.

## 8. Testing

- `platform_rest.rs`: mock HTTP server tests for the two published paths
  (`GET /apps/skills/api/skill/`, `GET /apps/skills/api/skill/{id}/download`)
  + 4xx mapping, like the wire-shape test currently in
  `platform.rs:266-322`.
- Env interpolation: unit tests for `${VAR}` resolution, missing-var error,
  no-interpolation passthrough.
- MCP modern protocol: mock server speaking `2026-07-28` — `server/discover`,
  `tools/list`, `tools/call`, required per-request `_meta`/headers, SSE and
  JSON responses, `401` mapping, and a legacy-era server producing a clear
  error (not a silent skip).
- Route: direct-route resolution unchanged; `examples/config/*.json` fixture
  tests.
- Config: legacy-field round-trip + one-time warning; perms unchanged.
- Serve modes: no rotation path to test (deleted); inbox keeps its
  "token read per call from config" tests (`inbox_poller.rs:1190-1239`
  pattern), re-pointed.
- Compaction: `context_window` fallback ordering test (profile → count).
- Delegated agents: session-build tests updated to expect only the built-in
  `general-purpose` spec.
- Full `cargo test --locked` plus `cargo check` for both crates; `dep_guard`
  untouched.

## 9. Open questions

1. **Resolved by spec:** OpenAI-compatible surface (`/apps/llm/api/v1/*`,
   incl. `GET /models`), `/mcp` as whole-platform MCP server, skills REST
   paths, `api-keys/mint|rotate` with nullable `expires_in_days`.
2. **Bearer semantics (NOTE — verify after refactoring, not blocking):** the
   spec's security scheme still says "bearerJwt … obtain via login/basic",
   and `/mcp` says "requires a valid JWT". Confirm that minted API keys are
   accepted as the bearer on REST **and** `/mcp`, and what RBAC role a
   personal key maps to (tool visibility is role-filtered). The design
   assumes minted keys are valid bearers; one authenticated curl against
   `/apps/skills/api/skill/` and `/mcp` with a minted key settles it —
   scheduled as post-landing verification (§7).
3. **Skills response shapes:** the spec types both skill endpoints' 200
   responses as generic objects. Confirm the concrete list/download JSON
   fields (the client needs at least `id`, `name`, `version`, and for
   download `url` + `sha256`) — one call with a real token settles it.
4. **Naming:** token prefix and env var (`mpat_` / `CRAFTSMAN_PLATFORM_TOKEN` are
   placeholders).
5. **Resolved by decision:** MCP client goes modern-only `2026-07-28` in
   this refactor (§4.6); no legacy fallback. Any streamable-HTTP server
   configured in `mcp_servers` must therefore speak the modern protocol —
   confirm that is acceptable for users' existing non-platform MCP servers,
   if any.
