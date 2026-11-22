# Task context and inspection

The maintained TUI/ACP implementation can compile a bounded working record of
actual task inputs, current todos and permitted file-tool observations. It adds
one JSON-escaped system addition before a main model request. Original portable
messages, tool results and signed/opaque native continuation items remain intact.

Compilation is explicitly opt-in:

```json
{
  "context": {
    "enabled": true,
    "max_bytes": 12288
  }
}
```

Missing/false settings preserve existing generation. `max_bytes` is bounded to
2048–32768; invalid selected fields refuse before session storage/provider
construction. Existing user config is not migrated or enabled automatically.
Framework, legacy/private/frozen and delegated policies retain their defaults.

## What is retained

Actual human input is recorded at the Agent's append/checkpoint boundary.
Runtime notifications and generated summaries do not become human instructions.
The owned TUI saves progress before sampling. ACP retains its existing
non-durable progress policy: selected observer refusals save the accepted input,
and completed or controlled-paused turns use actual final saves. ACP errors keep
selected input in live state without rolling it back; this does not add
pre-sampling durability to every ACP model or tool operation.
Whole input text is pinned only within 4 KiB; larger bounded inputs retain a
digest/byte count and an explicit omission. At most four recent whole updates,
32 selectors and a 32 KiB stored record are retained. Input observation/hashing
has a 1 MiB ceiling. Sequences never wrap, and malformed/versioned-key collisions
fail with fixed text rather than rewriting unrelated custom state.

Read observations are historical page/result fingerprints, not full-file hashes
or claims about external writers. A completed owned write invalidates matching
old reads; failures and unknown-effect operations make observations uncertain.
An overriding host tool cannot obtain file-tool authority from its name.
The controller performs no extra filesystem scan, read, model request or tool
dispatch. Actual compaction retains pins and resets derived associations; model,
thread and workspace rebinding preserves conversation without replay.

## `/context`

The read-only live slash command reports bounded local state, pins, todos,
observations, omissions, configured model-window information and the last
prepared request shape. It works during a turn without initializing another
agent, resolving credentials, saving config or making a provider request.
Missing/zero usage is labeled unknown; local counts are not a provider-native
dump or an exact current token count. Normal SafeBackend rendering applies.

The controller lives in `src/context_compiler.rs` and its submodules;
`src/agent_controls.rs` and the optional framework boundary owner connect real
input/save events. TUI command/transfer and ACP support are app-selected. Native
and live quality checks remain separate; this bounded continuity slice does not
implement tool-result eviction, a read cache, full typed context artifacts or a
broad coding-performance claim.

The first full local run caught the new reserved-command entry out of lexical
order. The entry was reordered; the existing sorted/unique assertion and all
other native checks remain intact. The first failed run is retained separately
from corrected-source validation.

In a privacy conversation, which nothing selects yet, the same controller runs
behind a private adapter. Its record is unchanged; the section the model is
given is made of source parts and carries no digest or size of recorded text.
See [privacy-provenance.md](privacy-provenance.md#controller-adapters).
