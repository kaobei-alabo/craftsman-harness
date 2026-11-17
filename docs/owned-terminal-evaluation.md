# Owned-terminal synthetic evaluation v3 — November 18, 2026

The isolated driver runs the production owned TUI at frozen source 33a80d9,
using a synthetic provider, real manual one-call approval keys and ordinary
quit/reopen. Three bounded scenarios pass their unchanged original held-out
graders. The synthetic provider supplies fixed reference edits, so these passes
validate the fixture/capture boundary; they are not a full live coding score.

The [v3 contract](../evals/coding/owned-terminal-v3/scenarios.json) is separate
from the unchanged [v1 preparation](../evals/coding/latest-runtime-v1/long-session.json)
and [v2 contract](../evals/coding/owned-terminal-v2/scenarios.json).
Preparation 0ad1e1a, v2 implementation 96b6f68, the original v5 corpus, historical
17/30 and 21/30 grades, seven v2 captures and every failed attempt remain intact.
Independent v2 reviews found unverified executed-buffer rereads, live SQLite
path reopening, task-witness producer identity, post-collection stream limits,
negative-condition issuance and host-write/cleanup gaps. Those historical
captures are not retrospectively authenticated by v3.

## Source, settings and isolation

Runtime source is `33a80d99431fd707b148485cb714f365b1946803`, tree
`ddee5694698835c48c9c7bbf37b4140b64d721dc`. The actual tracked-archive build used
`cargo build --locked --release --no-default-features -p craftsman-rs --bin craftsman`;
its artifact target ends `cargo-targets/evaluation`. Archive SHA-256 is
`366970a77173141c6c8317764e85c703e1882e780d472efb552a845722c4e594`.

The immutable image is
`sha256:6123321bf37c5641bc6ff4b4f79848f8b8e752fbc78a2f4361406162943cb38b`;
Craftsman binary SHA-256 is
`08542be11e7d207b43c4e0622a6f24180eaa2de9d65b247599f7f70660b56585`.
The unchanged v5 corpus SHA-256 is
`5d45038dca136245f090d2e2ed83a3e99f7a7c18eff19f2c4ba2d750d71d89f7`.
A future combined runtime needs a separately reviewed source/image binding.

The agent mounts only its disposable task and a read-only client bundle.
The separately owned collector mounts that task and its own read-only bundle;
its private capture directory and stdin/stdout authority are outside candidate
access. Provider, gateway, copied-query directories, source snapshots, original
graders/references and signing keys remain outside the agent mount. Actual
Docker inspection checks image, mount destinations **and Source paths**,
read-only root/client, dropped capabilities, no-new-privileges, 128 processes,
768 MiB, two CPUs, and the actual network ID/Internal property. No host config,
real key or Docker socket is mounted. Public Craftsman exec and headless gadget
callables remain absent.

Actual settings and received requests are checked: Sol 6.1 Responses, `xhigh`,
8,192 output tokens, 20 gateway requests, 900 seconds per scenario, 14 user
turns, ten 8,192-byte updates, 12,288-byte context, repeat limit five, review
signals off, manual permissions and `auto_approve=false`. Voice/gadgets/MCP/LSP
are disabled. Only fixed synthetic routes/fake keys are used. A separate
zero-price fixture ledger retains the existing $25 and 20-request limits;
it is not another real allowance. The real ledger is read before/after,
never mounted/written.

Every run uses fresh create-only output, retains exact scenario bytes, copies
the evaluation implementation before starting producers, and binds its source
manifest and actual mounted bundle hashes into the receipt. Changed source
copies, bundles or observation files refuse authentication.

## Captured facts and scope

| Channel | v3 evidence |
| --- | --- |
| Input | Actual saved context pin/stamp, submitted digest, appended index, event/revision, session/scope and first observed row; witnesses precede lawful pin eviction |
| Conversation | Typed messages/tool-error booleans, ordered digests and group-local call/result/content digests; first-seen stored positions retain `dispatch_observed=false` |
| Lifecycle | Actual fixed OSC states, manual one-call key sends, owned PTY/keeper waits and terminal-mode restoration |
| SQLite | Whole retained main/WAL/SHM acquisition, membership/WAL-generation epoch, ordinary private-copy recovery and held current-step/thread/workspace/input/sequence expectations |
| Gateway | Actual attempts/admissions/refusals, headers, genuine provider EOF and settlement/unknown facts; no invented historical 21st attempt |
| Files | Finite no-follow observations, retained-leaf operator writes and stopped-task/source/effect/current-input hashes |
| Native fixture | Separately owned actual Popen, verified executed snapshot and instance/intended-request acknowledgement, bounded wait/genuine EOF/status/stream digests and one local forwarding attempt |

The SQLite reader never opens a live candidate pathname with sqlite3.
It opens members relative to retained root authority, records optional absence,
obtains all initial versions before reading, corroborates bytes in a second
bounded pass, and fences every descriptor/path/root after the entire
acquisition. Exact dev/ino/mode/size/mtime_ns/ctime_ns/nlink remain checked in
the supported POSIX driver. Main/page/WAL headers, rolling checksums, frame
salts and final commit markers are validated.
An actual sent approval followed by a stale blocked OSC display is in-flight
work, so it cannot start another acquisition until a later root transition
is observed. A changed stage or whole-file fence still refuses capture.

Each member and combined acquisition are capped at 8 MiB, reads/copy attempts
at 4,096, WAL frames at 16,384, query progress at 4,096 callbacks of 1,000
SQLite steps, and transient transfer at 512 MiB per scenario/48 KiB per
chunk/256 instances. Raw rows/bodies never enter report history. Ordinary
SQLite rebuilds journal/index state inside the private copied namespace;
copied SHM is never committed authority, no committed WAL is omitted, and
`immutable=1` is never used. Unsupported spill/uncommitted/old-salt reset
tails refuse whole-snapshot completion; they are not necessarily corruption.
Valid WAL checksums and integrity alone do not authenticate pairing: a
compatible foreign WAL may recover another row, so current-step expectations
are required.

Manual `/compact` covers the fixed ten-update/ack tail and four kept messages.
A general tool-expanded tail may omit its latest User message and is rejected;
this is not a general compaction grader. Noncompaction cases retain the exact
original ordered input/group instances.

Live writer-hold/save-refusal is **quarantined, unverified and unexecuted**.
The selected evaluation boundary cannot anchor its actual live connection; a
copied-file lock is not a live-store lock. Historical v2 held-writer captures
and original registered-builder SQLite failure tests remain. Selecting
`--negative-case save-refusal` refuses before image/config/model work.

## Native collector and receipt

The [wrapper](../scripts/coding-eval/native_probe_wrapper.py) executes only the
already verified bounded snapshot. The child hashes its exact received buffer
before compile/exec, corroborates the retained source, and acknowledges source
plus instance/intended binding on a private pipe closed before fixture code.
Changed post-verification source/buffer data refuses with an incomplete witness
and no probe execution; restoring the path later cannot erase the observation.

The read-only [collector](../scripts/coding-eval/owned_tui_collector.py) owns
the actual inner Popen/streams/wait. Docker CLI and child return are distinct.
It reserves private create-only request/forwarding slots before effects;
failed/partial/overflowed instances stay closed with no replay or transport
retry/redirect. Both streams are collected concurrently with **pre-retention
64 KiB bounds**, original **20-second execution and five-second cleanup**.
Wait without genuine EOF for both streams is incomplete. Complete issuance
requires source/instance/wait/EOF/status/cleanup and successful bounded local
forwarding/witness writes. Candidate witness JSON and zero/foreign
source/instance claims never authenticate a producer.

Classification is `independently_requested_fixed_fixture_child`. An intended
call/request, stored pending group, OSC modal or Enter key constrains intent
but cannot authenticate an actual outer runtime approval/dispatch. Local
forwarding once is not receiver exactly-once delivery. Saved relay content
is separately matched to an actual subsequent provider request/result digest.
The unchanged probe, intentional exit seven and original effect remain
historical after the operator changes its input.

Duplicate-create and direct-reread negatives acquire their new saved row
**after the actual completed owned TUI return**. Issuance requires that return
before the snapshot, the new specific input/group/result, matching
provider-observed content and command 1→2 or read 0→1 delta, while the original
effect stays unchanged. Missing/wrong/stale/relocated results cannot be inferred
from a variant or sequence increase. Earlier live-window acquisition refusals
remain failed/unclassified evidence, not a diagnosed production failure or
authenticated negative.

HMAC issuance follows matched actual producer histories, verified snapshots,
owned returns, successful targeted container-absence queries and retained
stopped file data. A nonzero Docker query is an observation failure, not
confirmed absence. Cleanup query/removal/close outcomes are retained. Keys stay
outside the agent. Caller JSON cannot obtain a runtime receipt; standalone
metadata self-check remains explicitly non-runtime.

Direct private callbacks, typed TurnResult/AgentError/checkpoint-failed returns,
live RecoveryInfo/reconciliation, outer invocation/ToolMeta/truncation, whole
activity/quiescence, arbitrary-shell no-reread, external-writer CAS/rollback
and receiver exactly-once remain **unverified**.
`all_stronger_private_criteria_verified=false`. The operator and Docker daemon
are trusted; private issuer tokens/HMAC do not protect against an operator
who rewrites the driver or obtains its key. This does not establish OS
containment, cross-process leases, remote billing termination or release readiness.

## Reproducing the checks

```sh
python3 -I -B scripts/coding-eval/run_owned_tui.py \
  --image-build-record /path/to/owned/image-build-record.json \
  --output /path/to/fresh-v3-output
python3 -I -B scripts/coding-eval/run_owned_tui.py \
  --image-build-record /path/to/owned/image-build-record.json \
  --scenario interruption-reopen-v1 --negative-case duplicate-create \
  --output /path/to/fresh-negative-output
python3 -I -B scripts/coding-eval/test_preflight_latest.py
python3 -I -B scripts/coding-eval/test_long_session.py
python3 -I -B scripts/coding-eval/test_owned_tui.py
```

Other supported negatives are `provider-eof` and `forbidden-reread`. Genuine
no-terminal provider EOF/unknown usage/root error stays a failure observation;
an in-progress row is never saved success. The owned-TUI entrypoint also runs
retained-file, SQLite/current-step, collector, negative-issuance and cleanup
regressions, including real missing EOF and source/buffer races.

The marked loopback gateway fixture retains its10s readiness wait,5s child
cleanup and shared2s pipe-drain bound. It owns concurrent64KiB stdout/stderr
readers and reports real child exit, EOF, parse/overflow failures and bounded
synthetic stderr instead of losing them behind a readiness queue timeout.
Port publication still follows the actual gateway bind and is read from its
stdout; the20 admissions,21st HTTP402 refusal and genuine EOF/unknown ledger
assertions remain exact. Additive owned controls cover diagnostic output before
bind, a held-port bind refusal, invalid bootstrap, malformed stdout and overflow.
PR29/run44195435162's original macOS readiness timeout remains failed evidence:
its missing child diagnostics do not establish a historical scheduling or gateway
cause, and hosted macOS validation of this fixture correction is still required.

There are still **17 discoverable verification checks**: all 14 originals plus
latest preflight, long-session and owned-TUI entrypoints. Every original
context/setup/default-build/both-native-mode condition and publication hold
remains exact. Source-bound synthetic proof does not replace coordinator
fresh full 17/native/hosted/inventory/warning/tree/independent-main gates or
independent driver/isolation review.

The unchanged v1 preflight still refuses the current documentation checkout:
its broad source inventory includes the previously authorised README Explore
edits from e6dc4fd. The explicit frozen 33a80d9 checkout passes with the strict
three-field build-record projection, which is metadata only. Both refusals and
the frozen-source success are retained separately; no source-comparison check
was weakened, and neither result authorises a mutable combined/live source.

The unchanged v5 corpus prerequisite separately passed in this same frozen
image: 30 originals failed independent held-out grades and 30 references
passed, with complete Rust grade/doctest output. This is not a model run or
a newer combined-source/image validation.

No paid calls, cap migration, real-ledger write, global install, PR or
publication occur. The original ledger remains **$24.285934/$25**, 915 rows,
40 unknown reservations, with original 855 rows/bindings unchanged.
The present $25 allowance cannot fund a fresh full run. The proposed $25 extra/
$50 total on that same ledger remains unanswered and unauthorised; its full 30,
six selected repeats and three long sessions still need budget/admission
approval. Offline preflight is not live authorization.
