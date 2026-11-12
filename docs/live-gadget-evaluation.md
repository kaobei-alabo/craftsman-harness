# Live starter gadget evaluation — November 13, 2026

Four small gadgets were authored, compiled and then used by the actual Craftsman
agent against real configured models. All four build sessions and all four use
sessions succeeded. Thirty independent behavior cases passed, including six
expected input-error cases. The installed user copies passed the same cases.

| Gadget | Authoring model | Function | Independent cases |
| --- | --- | --- | --- |
| `starter-text-stats` | `gpt-6.1-sol` | `count_text` — words, lines and Unicode scalar count | 4 |
| `starter-slugify` | `us.anthropic.claude-fable-5-2` | `slugify` — lowercase ASCII URL slug | 6 |
| `starter-csv-total` | `gpt-6.1-sol` | `sum_csv` — checked signed-integer CSV sum/count | 10 |
| `starter-json-shape` | `us.anthropic.claude-fable-5-2` | `inspect_json` — top-level JSON type and entry count | 10 |

This is live model evidence, separate from earlier scripted provider/authoring
fixtures. The user authorized these model calls and config changes. It establishes
these four tasks on this macOS setup; other configured models, general coding
quality, platform/native containment and public headless execution remain
separate verification.

## Configuration

The selected Codex connection was copied from the user's `config.toml`, including
its actual Responses endpoint, environment-supplied key, `gpt-6.1-sol` selection
and `xhigh` effort. Seven GPT IDs/effort choices were imported from its configured
catalog. Model labels contain GPT names only; the user explicitly retained the
working endpoint URL.

Claude Code uses Bedrock in `us-east-1`. The same configured API token and four
actual Claude IDs were mapped to the native Anthropic-compatible
`bedrock-runtime` Messages route. `us.anthropic.claude-fable-5-2` was selected for
the live Claude tasks. Both connections passed an initial real inference request
through Craftsman. No subscription/OAuth credential was substituted and no
different endpoint/model was silently used.

Live configuration was backed up before editing, remains mode 0600, and retains
manual permission mode, disabled blanket approval and the original shell limits.
Only unchanged unused placeholder connections were removed. Test configs kept
credentials in child-process environment references; keys and provider response
headers were not written to logs or repository files. The live secrets remain
in the user's private configuration, not these examples or this report.

## Build and approval proof

Each independent task started with an empty isolated home/workspace. The model
read the built-in `build-gadget` skill, invoked the create-only scaffold command,
edited its own Rust source and optional manifest description, then invoked the
authoring build command. SDK/toolchain/dependencies stayed unchanged.

Native scaffold/edit/build operations received individual approve-once replies
after reviewing their path/command; no "Always" approval was selected. Source
and manifests were reviewed before building. All capability lists are empty,
with no filesystem/network/environment access or extra native build scripts.
The build-only sessions stopped before guest invocation. Fresh use sessions then
received an explicit ephemeral grant for the reviewed ID and invoked
`gadget_call`. No persistent grant file was manufactured.

Build fixtures selected the existing 240-second test-only shell budget. The live
120-second shell setting and native cleanup limits were preserved. Wall times
include model work, native compilation and manual approval waits; they are not
latency benchmarks or billing measurements.

Observed agent calls included:

| Function input | Actual tool result |
| --- | --- |
| `count_text` on `one two\nthree` | `{"chars":13,"lines":2,"words":3}` |
| `slugify` on `Hello, WORLD! 2026` | `{"slug":"hello-world-2026"}` |
| `sum_csv` on `10, -4, 7` | `{"count":3,"sum":13}` |
| `inspect_json` on `{"a":1,"b":[true,false]}` | `{"entries":2,"kind":"object"}` |

Independent checks covered empty text, Unicode, punctuation, negative integers,
the i64 maximum, malformed CSV, positive/negative sum overflow, all JSON root
types and malformed JSON. The expected error cases were refused, rather than
being counted as successful output.

## Installed use

Reviewed packages were installed create-only under the user's gadget directory.
The authoring CLI materialized the matching embedded SDK and correct local Cargo
paths; the promoted Rust source and WASM hashes match the agent-built versions.
The installed CLI was backed up and replaced with the exact release executable
used by this evaluation. Its version remains 0.4.0.

From a normal project directory, start `craftsman` and approve the newly
discovered pure gadgets when prompted. To check one directly without a model:

```sh
craftsman gadgets call starter-text-stats count_text \
  --allow-gadget starter-text-stats --args '{"text":"one two\nthree"}'
```

An ungranted `gadgets list` reports all four as `needs-approval`. The separate
project-trust boundary also remains: when the user's home is used as the project
cwd, its `.craftsman/gadgets` directory is also a project root and a headless call
without project trust is refused. Installed-copy checks used a normal separate
workspace and did not bypass that boundary.

Sanitized build/use transcripts, exact cases, component/source hashes, config
backup records and installation proof are retained locally under
`target/verification/live-gadget-eval/`. Source: merged-main `7fc6174`, Rust 1.99.1,
Wasmtime/WASI 48.0.8; the tested default release executable SHA-256 is
`db38adff3f102d5a92e4fe571783e0f499687c0d38bc738b72be922fdcb58fb6`.

The obsolete platform connection example was removed at the user's request.
Other provider examples, runtime protocols, SDK/WIT/pins, schemas, native limits
and the binary/container publication hold are unchanged.
