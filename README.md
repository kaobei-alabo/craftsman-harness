<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/craftsman-logo-dark.svg">
    <img src="docs/assets/craftsman-logo-light.svg" alt="匠 — Craftsman" width="560">
  </picture>
</h1>

<p align="center"><strong>A coding agent that builds its own tools.</strong></p>

Craftsman is a Rust terminal coding agent. It reads your code, edits files,
runs commands and carries work across sessions, on the model connection you
configure.

Its signature move: turn a useful routine into a **reusable WebAssembly
gadget**, then call it as a tool in the next model round.

![Craftsman terminal with a block-drawn 匠 mark, small hammer and gray/turquoise model connection status](docs/assets/craftsman-splash.png)

## What you get

- **Gadgets that last.** Ask for a tool. Craftsman scaffolds it with the
  embedded SDK, compiles it to a WASM component and discovers its typed
  functions without a restart. Text statistics, slug generation, checked CSV
  totals and JSON inspection were built and used this way in
  [live model sessions](docs/live-gadget-evaluation.md).
- **A contained place for gadget code.** Gadgets run in Wasmtime with no
  inherited filesystem, sockets or environment. Declared capabilities and
  your approval govern what a gadget may reach.
- **Your models.** Configure your own endpoint, key and model. Named
  connections, a grouped model catalog and per-model reasoning effort are
  described in [models and configuration](docs/provider-models.md).
- **A terminal that keeps you in control.** Streaming replies, file edits,
  shell tools, planning, permission modes with approvals, per-turn file
  checkpoints, and sessions stored locally with no remote replica.
- **Open ends.** Configured MCP servers, standard ACP for editors, and the
  lightweight `craftsman-harness` Rust framework with the gadget SDK for
  your own integrations.

## Get started

Install Rust through rustup, and CMake. Linux also needs ALSA development
headers; see the [build guide](docs/production-readiness.md).

```sh
git clone https://github.com/kaobei-alabo/craftsman-harness.git
cd craftsman-harness
rustup toolchain install --no-self-update
./install.sh
craftsman setup
craftsman
```

`craftsman setup` configures your endpoint, key and model; you can also start
from the [example configuration](examples/config/multi-provider.json). Set
`gadgets.enabled` to use gadget authoring and the gadget catalog. In a
session, `/model` opens the model catalog and `/effort` changes the reasoning
effort.

Start Craftsman in the folder you want to work in; Git is optional. For
editors, `craftsman acp-serve` speaks ACP over stdio and `craftsman
acp-bridge` adds a WebSocket bridge. `./install.sh --upgrade` updates an
older installation.

## How a request runs

Local commands and controls are answered in the terminal. A prompt enters the
agent loop, which can repeat tool rounds before it answers.

<details>
<summary>Current flow, from your input to the response</summary>

```mermaid
%%{init: {"theme":"base","themeVariables":{"primaryColor":"#f3f4f4","primaryTextColor":"#263238","primaryBorderColor":"#82959b","lineColor":"#64767b","fontFamily":"sans-serif","fontSize":"14px"},"flowchart":{"nodeSpacing":18,"rankSpacing":24}}}%%
flowchart TD
    Q["Your input"] --> R{"Local command<br/>or control?"}
    R -->|Yes| C["Handled locally<br/>setup · pickers · status · shell escape"]
    C --> O["Terminal"]
    R -->|Prompt| P["Prepare the round<br/>instructions · history · tools<br/>context, compaction and stop checks"]
    P --> H{"Privacy policy<br/>enabled?"}
    H -->|No| M["Your configured model"]
    H -->|"Yes, before every send"| K["Required privacy hook<br/>mask supported values · guard the request"]
    K -->|Passes| M
    K -->|Guard fails| X["Model work stops<br/>input and completed effects are kept"]
    M -->|Tool calls| G{"Mode, permission<br/>and approval"}
    G -->|"Native or MCP"| N["Host tools and configured MCP<br/>files · shell · web<br/>their own boundaries"]
    G -->|Gadget| W["WASM gadget<br/>Wasmtime · approved capabilities"]
    G -->|Refused| T["Record the actual result"]
    N --> T
    W --> T
    T -->|Next round| P
    M -->|Final answer| D["Show the reply<br/>originals restored for display<br/>when privacy is on"]
    D --> S["Checkpoint attempt<br/>saved, or the failure is reported"]
    X --> S
    P -->|"Cancel, error or limit"| S
    M -->|Request fails| S
    S --> O
    classDef wasm fill:#dcefee,stroke:#408b8b,color:#263238;
    classDef optional stroke-dasharray: 5 5;
    class W wasm;
    class K optional;
```

- **Boundaries.** WASM contains gadget code. Native shell commands and MCP
  servers keep their own execution and permission boundaries.
- **Privacy.** With a policy enabled, the hook is required before every model
  send. If its guard fails, the request is not sent.
- **Saving.** A checkpoint is attempted as work completes. A save that fails
  is reported as a failure, not shown as saved.

Code: [submission routing](crates/craftsman-rs/src/tui/commands.rs) ·
[session construction](crates/craftsman-rs/src/agent.rs) ·
[agent loop](crates/craftsman-harness/src/agent.rs)

</details>

## Script sandbox: current and planned

Rust gadgets already run in Wasmtime. A common sandbox for compatible
**Python, JavaScript and TypeScript** scripts, with a review layer for their
file changes, is planned. It is not shipped.

<details>
<summary>What differs between today and the plan</summary>

```mermaid
%%{init: {"theme":"base","themeVariables":{"primaryColor":"#f3f4f4","primaryTextColor":"#263238","primaryBorderColor":"#82959b","lineColor":"#64767b","clusterBkg":"#f8fafb","clusterBorder":"#a9b7bd","fontFamily":"sans-serif","fontSize":"16px"},"flowchart":{"nodeSpacing":24,"rankSpacing":28,"wrappingWidth":230,"subGraphTitleMargin":{"top":14,"bottom":14}}}}%%
flowchart TB
    H["Same agent harness<br/>query · model · context · history"]

    subgraph BEFORE["Current"]
        B1["Inputs<br/>host file tools, or files a gadget was granted"]
        B2["Execution<br/>Rust gadgets in Wasmtime"]
        B3["Changes<br/>the permission checks of each tool"]
        B1 --> B2 --> B3
    end

    subgraph AFTER["Planned, not shipped"]
        A1["Inputs<br/>task-scoped snapshots"]
        A2["Execution<br/>Python · JavaScript · TypeScript<br/>WASM interpreter + virtual /work"]
        A3["Changes<br/>create, update and delete plan<br/>reviewed as a batch"]
        A1 --> A2 --> A3
    end

    H --> B1
    H --> A1
    B3 --> R["Results return to the agent"]
    A3 --> R
    classDef planned fill:#dcefee,stroke:#408b8b,color:#263238;
    class A1,A2,A3 planned;
```

- **Planned, not included today:** script interpreters in WASM, a virtual
  `/work` for the files a script touches, and a create, update and delete
  plan that is reviewed as a batch.
- **Not part of the plan:** arbitrary native runtimes, package installers or
  containers running inside the sandbox.
- **Unchanged either way:** native shell commands and MCP connections keep
  their own execution boundaries.

Scope and progress: [script runner](docs/wasm-script-runner.md).

</details>

## Privacy mode: experimental and opt-in

Privacy mode is off unless your own user configuration enables a policy, and
then it applies to the interactive `craftsman` start only. It keeps your
original records locally and gives the model a separate masked view.

- **Vault password.** Asked at startup, after `/clear`, and when you change
  the model, effort or mode. The vault encrypts the original values and uses
  keyed lookup indexes. Local messages, history and checkpoints keep their
  existing plaintext form.
- **What is masked.** Supported email addresses and phone numbers. Anything
  else is sent as written. A policy that declares further values to mask is
  refused at start for now.
- **What is restored.** The reply you read, and validated Contact data fields
  in a supported file write. Shell commands, paths, source code and other
  tool arguments are not restored.
- **Restrictions.** It runs in the `manual` or `plan` permission mode, with a
  model connection whose items can be inspected; others are refused.
- **Voice.** In a build with voice input, cancelling a dictation retires the
  voice controller, so the next start loads the model again. Finishing a
  dictation normally keeps it.

Privacy mode is experimental. It does not make all data private and is not
supported on every model connection; what has been validated, and on which
platforms, is in the [current status](docs/runtime-status.md). Activation,
commands and limits:
[startup and commands](docs/privacy-startup.md) ·
[hook design](docs/privacy-hooks.md) ·
[send guard](docs/privacy-send.md) ·
[voice and shell](docs/privacy-voice.md).

## Documentation

- **Use:** [gadgets](docs/gadgets.md) ·
  [models and configuration](docs/provider-models.md) ·
  [connections and local state](docs/connections.md) ·
  [ACP](docs/acp.md) · [terminal status](docs/terminal-status.md)
- **Long tasks:** [turn limits](docs/terminal-turn-limits.md) ·
  [recovery](docs/terminal-recovery.md) ·
  [compaction](docs/terminal-compaction.md) ·
  [checkpoints and rewind](docs/workspace-checkpoints.md)
- **Task quality:** [task workflow](docs/task-workflow.md) ·
  [verification](docs/verification.md) · [context](docs/context.md) ·
  [failure controls](docs/failure-controls.md)
- **Project:** [architecture and current limits](docs/runtime-status.md) ·
  [evaluations](docs/coding-evaluation.md) ·
  [build and native validation](docs/production-readiness.md)

MIT licensed. See [LICENSE](LICENSE) and [third-party notices](NOTICE).
