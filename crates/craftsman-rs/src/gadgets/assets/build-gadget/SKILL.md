---
name: build-gadget
description: Build or adapt a reusable local WASM gadget when a missing typed function is worth keeping as an agent capability. Prefer existing tools or a shell one-liner for one-off work.
---

# Build a local gadget

A gadget is a Rust `cdylib` with a TOML manifest and a WASM component. The SDK
is embedded in Craftsman; no private registry or platform checkout is needed.

1. Run `craftsman gadgets doctor` through `execute`. If it reports a missing pinned
   Rust toolchain or `wasm32-wasip2`, use the reported rustup command only with
   the user's normal installation approval.
2. Run `craftsman gadgets new <id>`. It creates an independent Cargo package under
   the user config's `gadgets/<id>/`, using the embedded SDK and a lockfile.
   `--project` explicitly selects `./.craftsman/gadgets/` instead. Existing
   directories are never overwritten.
3. Edit its `src/lib.rs`. Use owned `Deserialize + JsonSchema` input types,
   a serializable result and a synchronous `#[gadget_call]` function. A doc
   comment supplies its description. No separate export macro is needed.
4. Declare only the capabilities the implementation actually needs in
   `gadget.toml`. Pure computation needs none. Workspace files go through
   `craftsman_gadget::host`, not `std::fs`; HTTPS goes through `host::http_request`.
5. Run `craftsman gadgets build <id>` (add `--project` for a project package).
   The pinned stable compiler builds the component and static ABI validation
   checks it without running guest code. Failed builds keep the previous
   `gadget.wasm`; successful publication is atomic.
6. The harness discovers the installation/rebuild before the next model
   round and supplies function schemas. Call `gadget_call` directly; do not
   poll `gadget_list` after every build.

## Minimal typed function

```rust
use craftsman_gadget::{gadget_call, GadgetError};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input { text: String }

/// Count whitespace-separated words in a text.
#[gadget_call]
fn count_words(args: Input) -> Result<usize, GadgetError> {
    Ok(args.text.split_whitespace().count())
}
```

The dispatcher arguments are:

```json
{"gadget":"word-count","function":"count_words","args":{"text":"one two three"}}
```

For file work, add `read = ["workspace"]` under `[capabilities]` only when
needed and use `host::read_file(path)?`. Writes additionally need
`write = ["workspace"]`; they are create-only, exclude protected paths and
still pass the ordinary per-write permission gate. HTTP capabilities list
exact lowercase hosts, not URLs or wildcards. Avoid combining read and HTTP
unless sending workspace data is part of the user's actual task.

Building is a native Cargo operation: dependencies, procedural macros and
build scripts do not run inside the WASM sandbox. Use `execute` so shell
approval and environment scrubbing apply. The first-load capability approval
is separate. Never edit `.grants.json` or add `--allow-gadget`,
`--trust-workspace` or bypass flags merely to make an agent-built gadget run.
Use them only when the user has explicitly granted that scope. An unchanged
or narrower rebuild inherits its source-bound grant; wider capabilities ask
again. Declined components remain unexecuted.

If the compiler or dependencies cannot be installed, or the bounded WASM
host surface is a poor fit, report the limitation and use an existing tool
instead of creating a gadget-shaped workaround with broader permissions.
