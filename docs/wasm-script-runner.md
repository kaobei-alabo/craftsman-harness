# Reusable WASM script runner

On November 19, 2026, the owner resumed the gadget enhancement with **Python,
JavaScript and TypeScript** as the first language targets. Go is deferred.
This page records the implementation scope; the installed CLI still has the
existing Rust gadget authoring/runtime. Script execution through reusable
interpreters and the new file-change review layer are not shipped.

## Language coverage

| Target | Initial compatibility goal | Separate support required |
| --- | --- | --- |
| Python | CPython in WASM; selected standard library and explicitly supplied pure-Python modules | Native extensions, subprocesses, OS integrations and packages needing unsupported APIs |
| JavaScript | QuickJS in WASM; standard JavaScript, supplied modules and bounded file adapters | Node built-ins, native npm addons, browser/DOM APIs and process execution |
| TypeScript | Transform to JavaScript, then use the same WASM JS runtime | Full Node compatibility; type checking is separate from syntax transformation |

These languages cover useful file/data processing, validation and reporting
scripts. Language support alone does not prove that most skills or arbitrary
plugins are compatible. A skill can depend on shell commands, native packages,
external applications, network services or package-install hooks. The initial
compatibility audit must name the scripts and dependencies actually checked.
Existing built-in tools and configured MCP servers retain their execution paths.

### Dependencies on demand

The desired experience includes uv-style inline script dependency declarations
and Bun-style package imports. A trusted host resolver should read metadata,
resolve exact versions from explicitly allowed registries, and fetch bounded
compatible package artifacts as data. It must not execute package build/install
hooks or the skill script on the host. Version/content hashes and transitive
dependencies become part of the separately approved executable bundle.

Supply those packages read-only to the WASM module loader, with a per-bundle
dependency view rather than ambient user site-packages or node_modules. Package
fetching and guest network access are separate permissions. A pure-language
package can still require unsupported filesystem, network or runtime APIs;
binary extensions need compatible WASM builds. Dynamic package loading does not
imply that native Bun/uv or arbitrary package-install commands run inside WASM.
The first proofs should include dependencies introduced after interpreter build,
one declared-dependency example, and an incompatible dependency counterexample.

## Initial feasibility evidence

The November 19 JS/TS worker delivered a fixture-only QuickJS component through
the existing gadget host: different scripts, post-build modules and in-guest
TypeScript syntax transformation use the same 1,375,123-byte image. One separately
reviewed `defu` npm dependency executed successfully and produced a virtual CREATE
preview with no network calls or real-file application. The record includes
28 core and four dependency observations, including retained failed candidates
and unsupported-runtime counterexamples. The Bun-specific `Bun.file` call fails
because that API is absent; this is not general Bun or npm auto-install support.

Independent source review confirmed this synthetic proof and identified two
VFS defects: a zero-byte positioned write enlarges a file, and unlinked buffers
held by open descriptors escape the private VFS byte accounting. Both require
repair and actual rebuilt-image regressions. Neither finding demonstrated host
escape or real-file application; the original host memory limit remained intact.

The JS author delivered a separately rebuilt 1,381,447-byte image with passing
zero-write and live-buffer regressions plus the final 140 existing gadget tests.
Independent review verified these functional corrections. Positive build-input
checks exist; the interrupted negative build-input/incremental experiment remains
incomplete. The original image/dependency evidence is preserved separately.

The Python worker also delivered a CPython 3.14.0 fixture. Its v2 image is
19,697,470 bytes with one memory; the existing gadget host executed different
scripts and post-build dependencies, including a pinned pure wheel plus a
required stdlib wrapper. A separate host prototype routed ordinary Python file
APIs through virtual WASIp2 descriptors and derived CRUD previews. That descriptor
host is not integrated into the current app runtime. One large print attempted
70,001 bytes but retained only 8,193 with no truncation flag. This original
silent-loss failure is preserved.

The Python author subsequently delivered a guest-local buffering repair in a
new 19,662,089-byte image. Actual-host evidence shows a complete 60,001-byte
print and the correct retained prefix/truncation flag for 70,001 bytes, without
changing host capture. Independent review confirmed the healthy-output correction
and found four additional edges: typed-error flushing, byte counting for multi-byte
views, stale descriptors retargeting replacement files, and zero-byte writes
enlarging files. Those edges are assigned for repair. The old failed images and
results remain separate evidence. The author delivered those four corrections
in a separately sealed 19,741,769-byte v4 image and revised virtual host; final
independent review verified the four repaired predicates. Python retains explicit stale-handle refusal,
rather than claiming full POSIX unlinked-file behavior.

The [private session-capture foundation](session-script-capture.md) now has an
actual normal-builder/TUI/ACP owner, retained native reads, revocable binary
snapshots and complete historical CRUD previews. All public selectors remain
absent. Review found an actual-Agent identity-binding gap and late-stop return
races. The integrated repair adds a pure per-built-Agent identity and actual
backend check, plus stop fences through publication and receiver settlement.
Independent corrective review accepted those repairs. Fresh substituted Agents and new work
in a stopped turn are refused, while accepted historical information is retained.
The [bundle-admission implementation](script-bundle-admission.md) is
currently registered only for gadget-feature tests; checked code data is not
execution approval. Both foundations are merged as PR #31 and have scoped
independent source reviews plus full local17 and fresh main
[44567715919](https://github.com/kaobei-alabo/craftsman-harness/actions/runs/44567715919)
proof at `29349c5`/tree `93748936`. All four jobs, every mandatory step and both
native modes passed with original test bodies and warning signatures preserved.
That source is installed locally. These foundations select
no interpreter, execute no script and apply no native changes; source consent,
actual task-root derivation, backend selection and the public runner remain
later work.

These local macOS prototypes have scoped functional reviews and still need
production script/task authority, automatic input selection, actual turn
cancellation, review/apply and full native delivery checks. No interpreter was
added to the installed CLI. Resource timeout evidence does not establish
user-cancellation handling.

## First implementation gate

Before integrating a public script tool, prove that one fixed interpreter image
can run different supplied scripts, including a module supplied **after** the
image was built. Rebuilding one component per script does not prove reusable
interpreter support. Record exact runtime/tool versions, source and image hashes,
imports, memory/table counts, sizes, actual results and failed attempts.

Use the existing `craftsman:gadget@0.1.0` interface and Wasmtime/WASI 48.0.8.
Preserve the current ceilings: 32 MiB component, one memory, 256 MiB linear
memory, 10,000 table elements, four tables, 64 instances, 1 MiB JSON arguments
and results, and the default 30-second guest deadline. A standalone experiment
must distinguish matching these limits from exercising the actual gadget host.
Do not widen limits or use host Python/Node/shell execution to rescue a failed
guest. Source transformation/build tools have their own trust boundary and must
not execute the supplied script on the host.

The same proof must test ordinary file APIs below interpreter-level wrappers,
module loading, escape attempts and unsupported dependencies. Installing or
building an interpreter is separate from executing third-party skill code.
Feasibility evidence is not a shipped CLI, permission implementation, native
platform validation or live model-quality result.

## Shared file and approval path

Craftsman should find relevant files through its normal authorized tools and
capture narrow task-scoped snapshots automatically. Git and mandatory file
pickers are not required. Scripts see a virtual `/work` with immutable originals
and a copy-on-write overlay; ordinary file and descriptor APIs must use that
same boundary. No ambient host directories, environment, sockets or terminal
stdio are inherited. Custom gadget imports cannot bypass the virtual filesystem.

The host derives create/update/delete operations from its own originals and
validated candidates. Unknown files are not absent files. Empty-file creation,
replacement and deletion remain distinct operations; omission from guest output
never means deletion. Binary content and unsupported metadata need explicit
review or refusal. No native file changes occur during the initial preview proof.

The eventual default is one review of the complete related change set, with
optional selection. User-authorized scoped auto rules are enforced by the host;
an agent reviewer can advise but cannot grant permissions. Executable script
approval binds the script and dependencies separately from the interpreter.
Reads, network access and real-file mutations retain their independent gates.

Before applying changes, recheck permissions, cancellation, retained targets and
current content. Report completed, failed/uncertain and unattempted operations
truthfully. Do not promise a multi-file transaction, rollback, exactly-once
execution, portable protection against external writers or host-secret isolation.
Public headless execution and callable headless gadgets remain separate work.

## Rollout

1. Interpreter/module/file compatibility proofs for Python and JS/TS.
2. Source approval, automatic scoped inputs and a validated CRUD preview.
3. Batch/scoped-auto review and guarded create/update/delete host adapters.
4. Representative reusable skill scripts, full local/native CI and independent
   main verification before merge/install of the runtime changes.

Current runtime behavior is documented in [gadgets.md](gadgets.md) and
[runtime-status.md](runtime-status.md). The README keeps a compact comparison of
the current and planned harness paths.
