# Provider connections and model catalogs

Craftsman accepts named provider connections with models nested under each
provider. Model keys are the actual API identifiers; there is no separate alias
or config-version field. The selected `model` is `provider/model-id`. The parser
splits only the first slash, so `custom/org/model` selects API model `org/model`
through the `custom` connection.

The complete placeholder example is [multi-provider.json](../examples/config/multi-provider.json).
Replace the selected model and matching model key, endpoint and API key. The
reasoning levels shown there are illustrative and must match the chosen model.
`${ENV_VAR}` references remain supported in each provider's `api_key`. Session
construction resolves only the selected provider's credential. A missing
credential on another connection does not prevent that session from starting.

| Protocol | Request API | Authentication |
| --- | --- | --- |
| `openai_responses` | `POST /responses`, typed SSE events | Bearer API key |
| `openai_chat_completions` | Existing `POST /chat/completions` transport | Bearer API key |
| `anthropic_messages` | `POST /messages`, content-block SSE events | `x-api-key`, documented `anthropic-version: 2026-10-21` |
| `gemini_generate_content` | `POST /models/MODEL:streamGenerateContent?alt=sse` | `x-goog-api-key` |

These are text/function-call adapters. Select a tool-capable model supported by
the configured API. Gemini Interactions, cloud-specific authentication and
multimodal input adapters are not added by this change. Gemini's documented
OpenAI-compatible route can instead use `openai_chat_completions`. API keys and
protocol choice are explicit; the agent does not connect a consumer subscription
or silently switch to another endpoint/model when a request fails.

## Terminal selection

`/setup` and `craftsman setup` offer a picker for configured connections, then
edit that connection's endpoint/key and actual API model ID. The complete
candidate is validated before saving; failed saves keep the current session.
Other connections and model settings survive, and reconfiguration retains the
conversation without replay. See [provider setup](provider-setup.md). Flat
configurations retain the legacy platform/direct setup.

```text
/model
/model openai/YOUR_GPT_MODEL_ID
/model anthropic/YOUR_CLAUDE_MODEL_ID --reasoning=high
/effort
/effort low
/effort default
/effort 4096
```

The picker combines all configured connections into one menu. Optional discovery
uses each connection's own authentication and response format, preserving
provider-qualified identifiers. A failed discovery leaves configured models
available. Placeholder/unresolvable keys do not generate discovery requests.
The parent task owns bounded concurrent request futures directly; cancellation
drops those futures, and the existing TUI guard refuses late picker delivery.

Selections during a turn wait for its existing completion boundary. The old
session's background/LSP cleanup still precedes conversation transfer. Messages,
tool results, files, todos, pending approval, thread identity, checkpoint sequence
and actual main usage carry forward. No tool/model/hook/checkpoint is replayed by
the transfer. Initialization errors retain the conversation and close sampling
under the selected configuration; the previous provider is not used as fallback.

## Model settings

`models[MODEL].settings.reasoning: null` selects provider defaults. An effort
setting is `{ "type": "effort", "value": "high" }`, with choices declared by
`reasoning_controls: { "type": "effort", "values": ["low", "high"] }`.
Missing effort metadata is labelled provider/model dependent. Unsupported
settings are refused locally when the protocol cannot encode them, or reported
as a provider failure; no automatic downgrade occurs.

Token budgets use `reasoning_controls: { "type": "budget", "min": 1024,
"max": 4096 }` and `settings.reasoning: { "type": "budget", "value": 4096 }`.
The effort picker offers budget values and `/effort TOKENS` accepts a declared
budget. Claude budgets must be at least 1024 and below `max_tokens`; manual
budgets only apply to models that support that API mode. Claude effort settings
select adaptive thinking and `output_config.effort`. Native Gemini effort values
map to uppercase `thinkingLevel` enum values; budget settings use `thinkingBudget`.
Model-dependent support remains the configured provider's contract.

Other request settings belong in `settings.generation`: Responses accepts
`max_output_tokens`, `temperature`, `top_p`, `text`; Claude accepts `max_tokens`,
`temperature`, `top_p`, `top_k`, `stop_sequences`; native Gemini accepts
`maxOutputTokens`, `temperature`, `topP`, `topK`, `stopSequences`. Native transports
refuse unknown generation fields. Compatible endpoints keep their existing
request-option surface. Optional `settings.context_window` controls the existing
compaction trigger. Catalog labels, capabilities and reasoning choices never
become generation parameters.

Named connections use their nested generation settings instead of global legacy
provider/model kwargs. Flat `base_url`, `api_key`, `model_catalog`, `model_settings`
and `model_profiles` remain supported when no named providers are configured;
unknown user fields survive saves. Declared `capabilities.tool_calling: false` or
`capabilities.streaming: false` prevents coding-agent selection. Unknown model
capabilities are not proof of API compatibility.

## Native continuation and limits

Responses reasoning/output items, Claude thinking/signature blocks and Gemini
response parts/signatures are retained with their actual portable assistant
message. App-only middleware checkpoints these under a reserved custom key.
The identity includes the connection, protocol, endpoint, model and credential
fingerprint; signed data is never forwarded to another identity. Restoration
validates the cached text/calls against the portable message before another
request. Changed response calls, malformed cache versions and overflow refuse
further work. Compaction prunes stale records rather than applying them to a
different message. Gemini calls without an API ID receive a local framework ID;
only IDs actually returned by Gemini are echoed for its retained native parts.

Native errors omit credentials and provider error bodies. Native HTTP does not
follow redirects or retry interrupted generations. Existing compatible transport
retry behavior stays in place, with fixed errors for named connections. The
owned terminal's model cancellation drops native request/stream waits. This does
not establish remote billing stop or OS containment.

Bounds: 16 providers, 512 combined catalog entries, 512-byte selection IDs/labels,
1 MiB provider config, 15-second discovery requests, 4 MiB native requests/events,
16 MiB streamed responses, 1 MiB retained text/checkpoint metadata and 256 cached
assistant records. Existing SQLite layout/retention, native shell/cleanup limits,
dependency/toolchain/SDK/WIT pins and frozen protocols/goldens remain unchanged.
Private prepared runtimes refuse named provider configuration.

## Verification and status

Scripted loopback HTTP tests cover the four request protocols, authentication,
tool schemas/results, effort/budget mapping, genuine usage, truncated responses,
cache corruption, provider isolation and cancelled requests followed by another
usable turn. An isolated marked actual-terminal fixture writes a physical file,
switches through all four protocols, checks retained history and provider-owned
signatures, cumulative usage and increasing SQLite sequence in the same thread,
and confirms the write is not replayed. Unmarked helpers return before path or
config access. Discovery tests cover partial failure, namespacing and cancellation.
No user credentials or paid model calls are used.

Full native local modes and all four hosted checks still gate the reviewed
implementation, followed by independent merged-main validation. Scripted tests
do not establish live coding/gadget quality or universal model compatibility.
Remaining native-console, recovery, concurrent-owner and containment work, public
`craftsman exec`, inherited shell issue #138 and publication gates stay separate.

API references reviewed November 10, 2026:
[OpenAI Responses migration](https://developers.openai.com/api/docs/guides/migrate-to-responses),
[OpenAI reasoning](https://developers.openai.com/api/docs/guides/reasoning),
[Claude streaming](https://platform.claude.com/docs/en/build-with-claude/streaming),
[Claude thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking),
[Gemini GenerateContent](https://ai.google.dev/api/generate-content), and
[Gemini function calls](https://ai.google.dev/gemini-api/docs/generate-content/function-calling).
