# Editing provider connections in the terminal

With named providers configured, `/setup` and `craftsman setup` open a connection
picker. Select the connection to edit its endpoint, API key and actual API model
ID. The dialog shows that connection's existing protocol. `Ctrl-U` clears the
current field; `Enter` saves and selects the model, and `Esc` keeps the current
configuration. API keys remain masked. Create additional connections or change
their explicit protocol in the JSON config using the
[four-provider example](../examples/config/multi-provider.json).

Previously, setup always wrote the legacy flat fields and an unqualified model
ID. Those fields do not choose a connection when `providers` exists. The result
could be saved with an invalid selection such as `MODEL` instead of
`provider/MODEL`, leaving sampling closed under that configuration. Setup now
constructs and validates the complete provider-owned candidate before saving.

Only the selected connection's endpoint and credential change. Existing catalog
entries and their settings remain; a newly added actual API model ID starts
with provider defaults. IDs containing slashes remain intact after the first
provider separator. Other connections, flat legacy settings and unknown user
fields survive the save. Credentials for unselected providers are not resolved.
The existing 16-provider, 512-model, selection/endpoint/credential/config bounds
still apply. Oversized pasted values are refused whole, keeping the field draft.

The configuration and active session are replaced only after a successful
save. Invalid candidates and save failures retain the current in-memory config
and session. Errors do not echo provider keys or storage error bodies. Saving
does not sample a model or discover provider catalogs; it confirms local
configuration, not API compatibility. Initialization failure retains the earlier
conversation and closes old sampling, as in the existing model selector.

The existing reconfiguration owner performs background/LSP cleanup, then
transfers committed messages, files, todos, custom state, approvals, thread,
sequence and actual accumulated usage. It replays no model, tool, turn hook or
checkpoint. Native response items remain bound to their original credential,
endpoint, protocol and model identity. Changing a key or model preserves
portable results while keeping old signed/opaque items with their old identity.

Flat configurations offer direct endpoint/key/model setup. Stale legacy
setup actions cannot overwrite a named provider selection; they reopen the
connection picker. Framework/prepared runtimes, SDK/WIT, storage layout and
retention, dependency pins, native cleanup, frozen protocol/schema/goldens and
publication settings remain unchanged.

## Verification

Additive tests cover scoped settings/user-field preservation, actual API IDs
with slashes, new-model defaults, invalid and overbound candidates, unresolved
credentials on another connection, masked key rendering, cancellation retaining
the draft and whole-paste refusal. A marked isolated actual-terminal-builder
fixture performs a physical write through Responses, obstructs the config file
to force an actual failed save, then explicitly restores it and edits the
connection/key/model. It checks saved JSON, same-thread increasing SQLite
sequence, accumulated response usage, retained history and no repeated write or
forwarding of stale native items. Unmarked helpers return before path/config
access; all credentials/providers/files are synthetic.

The first source check's unused import and the first new-test module-path
compile errors are retained separately; only the new import/test paths were
corrected. Fresh full local and all-four/every-step/both-native-mode CI
verification, with exact original/new inventory, warning and tree audits, then
independent merged-main validation, gate delivery. Live model/task/gadget quality,
background/diagnostic recovery, native containment, public headless execution
and binary/container publication remain separate.
