# Terminal model catalog and reasoning effort

Named connections now use the [provider-owned nested catalog](provider-models.md):
`providers.NAME.models` is keyed by actual API IDs and `/model NAME/MODEL_ID`
selects its endpoint and saved settings. The legacy single-route example below
remains supported when no named providers are configured. Native Responses,
Claude and Gemini continuation data has its own bounded app-layer checkpoint
middleware; portable conversation transfer retains the contracts below.

The terminal selects models and reasoning effort while retaining the current
conversation. `/model` combines the configured provider's OpenAI-compatible
`GET /models` response, explicit `model_catalog` entries, `model_profiles` keys
and the current model. Configured labels/effort options override discovered
metadata for the same id. If discovery is unavailable, configured models remain
selectable. Lookup is asynchronous; Ctrl-C stops it without cancelling an idle
conversation.

```text
/model
/model MODEL_ID
/model MODEL_ID --reasoning=high
/model --reasoning=medium
/effort
/effort high
/effort default
```

Catalog entries with effort metadata offer combined model/effort choices.
`/effort` opens the current model's effort picker. Choices are saved in the user
config for subsequent terminal launches. Changes during an active turn are
queued until it returns. The footer displays the selected effort.

## Config example

Replace these placeholder model ids, endpoint, key and supported levels with
your provider's actual values.

```json
{
  "base_url": "https://your-provider.example/v1",
  "api_key": "${CRAFTSMAN_MODEL_API_KEY}",
  "model": "model-fast",
  "permission_mode": "manual",
  "gadgets": { "enabled": true },
  "model_catalog": [
    {
      "id": "model-fast",
      "label": "Fast coding model",
      "reasoning_efforts": ["low", "medium", "high"]
    },
    {
      "id": "model-deep",
      "label": "Deep coding model",
      "reasoning_efforts": ["low", "medium", "high", "xhigh"]
    }
  ],
  "model_settings": {
    "model-fast": { "reasoning_effort": "medium" },
    "model-deep": { "reasoning_effort": "high" }
  }
}
```

Catalog metadata never becomes a generation parameter. In the owned terminal,
`model_settings[model].reasoning_effort` wins over provider defaults, the model
profile and global `model_kwargs`. Null means **provider default** and omits the
parameter. The string `"none"` is a distinct explicit effort value. Other model
settings and unknown user config data are retained. Private prepared and
legacy/frozen frontend construction do not select these new terminal controls.

Known values are `none`, `minimal`, `low`, `medium`, `high`, `xhigh` and `max`.
Support/defaults depend on the provider/model. A configured effort list restricts
the picker and rejects undeclared choices. Without metadata, `/effort` labels
choices as provider-dependent: manual values are explicit requests, not proof of
compatibility. The provider can reject them; no automatic downgrade occurs.

On November 9, 2026 the [official reasoning guide](https://developers.openai.com/api/docs/guides/reasoning?api-mode=chat)
documents model-dependent levels and Chat Completions `reasoning_effort` versus
Responses `reasoning.effort`. Craftsman currently uses Chat Completions and needs
a tool-capable compatible route. This change does not add Responses transport or
connect a ChatGPT/Codex subscription. Codex's [model command](https://learn.chatgpt.com/docs/developer-commands?surface=cli)
is the UX reference. API compatibility depends on the configured provider and
client transport.

## Conversation and ownership

The old initializer and entire session remain owned while a new session is
prepared. Old background work is cancelled/joined and LSP resources are closed
before committed state transfers; completed child writes and addressed
notifications are retained. Messages, files, todos, custom state, parked
approval, thread id and the next checkpoint sequence carry forward. Transfer
runs no model, tool, turn hook or checkpoint. Actual main usage totals carry
forward; the old model's compaction usage anchor is cleared.

The replacement keeps its own model, middleware, tools, registry and stop owner.
Stopping the retired owner cannot stop the new agent. A pending approval still
needs an explicit resolution. Initialization failure retains the conversation
and stops sampling instead of silently using the old model under the new
selection. Selecting a working model can continue it. Effects are not rolled
back and no cross-process session lease or OS-containment guarantee is added.

Catalog input is bounded at 1 MiB, 512 models, 512-byte ids/labels and the finite
effort list. Duplicates/malformed metadata refuse. Lookup keeps its 15-second
timeout. New tasks join terminal-owned cleanup. Existing shell/native limits,
SDK/WIT, dependency pins, SQLite layout/retention and frozen protocols/goldens
remain unchanged.

## Verification

Additive cases cover state/sequence transfer, independent cancellation owners,
parked approvals, active-agent refusal, config preservation, default/none,
provider metadata and bounds. A marked native fixture builds actual terminal
sessions against a local scripted HTTP provider, writes a physical file, switches
model/effort, checks retained request history and increasing SQLite sequence, and
tests catalog cancellation and failed-init recovery. Unmarked helpers return
before path/config inspection. No user config or real model account is used.

Full local/all-four/every-step/both-mode hosted checks still gate merge, followed
by independent merged-main validation before local fast-forwards. Tests do not
measure live quality, billing or OS containment. Inherited shell issue #138,
remaining recovery/diagnostic/native-console gates, public `craftsman exec`,
gadget-default and binary/image publication gates remain separate.

The full native local modes pass with stable runtime/test hashes: core
1965/0/14 and gadgets 2121/0/17 (passed/failed/ignored), warning-free, retaining
every original entry plus 18 new cases/one helper. The first complete verification
command exits 1 only for the new comparison wording caught by provenance; its
failed log remains retained. Corrected documentation provenance passes separately,
and JSON examples/fences are checked. This does not label that first command a
green full script or rerun unchanged native suites to replace its outcome.
