# Repeated-operation budgets and protected-test review

The TUI/ACP implementation supports an explicit exact-operation failure budget
and optional structural notes on the existing protected-content review:

```json
{
  "failure_control": {
    "repeat_limit": 5,
    "test_review_signals": true
  }
}
```

Absence is off. `repeat_limit` is required when configured and is an integer from
3 to 20; review signals default false. Invalid/unknown selected fields refuse
before storage/provider effects without echoing values. Config is not rewritten.
Framework, legacy/private/frozen and delegated defaults remain unchanged.

## Exact failures and actual pauses

The observer compares bounded actual tool-name, canonical argument and returned
error hashes. Success, changed attempts/errors, actual new input and observed
context changes reset pressure. Typed permission/cancellation outcomes retain
their existing owners. A successful nonzero shell result is not reclassified
from internal metadata.

Only actual final-registry identities establish trusted observational tools.
Missing registered operations can be classified as pre-execution failures;
unknown/effectful operations remain advisory. This is an enabled operation budget,
not proof that the whole task has made no progress or that partial effects are
absent. Repeated successful reads/planning do not become a pause verdict.

After a committed tool group finishes, the Agent records accepted input and
evaluates admission before preparation/sampling. Raced input is retained and the
decision is rechecked. A confirmed pause requires ordinary end hooks and a real
successful final checkpoint; save errors remain errors. Input admission closes
without calling user cancellation, losing steering or fabricating model output.
The last already committed group may contain more calls than the threshold.
Background work and native 120 s / 5 s reap / 2 s drain / 30 s shutdown limits
remain independent.

The TUI shows a confirmed pause as chat activity requiring a follow-up, with OSC
question-blocked status. An explicit next turn receives a fresh operation budget.
ACP retains its standard result/error shapes and actual transcript; there is no
automatic model/tool retry, resume or approval bypass.
Accepted steering is retained on selected cancellation after balancing the
committed call/result group; new steering closes immediately. TUI progress saves
and ACP's existing non-durable progress policy remain distinct. Selected ACP
observer refusals and confirmed pauses save their actual retained state, while
other ACP errors do not imply a pre-operation checkpoint.

## Structural review notes

Optional notes analyze borrowed original protected before/after bytes, within
the existing 64 KiB/file and 1 MiB review bounds. Recognisable removal,
skip/ignore/focus directives, reduced assertion/test sites and literal-false
guards are review signals. Strings/comments and ambiguous syntax do not create
false executable findings; limited/unavailable coverage is explicit.

Signals are not a semantic verdict. No-signal results, formatting/additive tests,
unsupported languages and legitimate refactors still use the existing original
content-bound approval and actual verification. No formatted-review delimiter
parsing, path reopening, external parser process, provider call or new scan is
performed. Changing protected contents after approval invalidates it as before.

Source: `src/failure_control/`, `src/agent_controls.rs`, the optional framework
boundary owner and the existing verification/snapshot adapters. Tests use actual
loop, tool, HTTP/file/checkpoint and UI events; native conformance and selected
live checks remain separate from a full performance evaluation.

In a privacy conversation, which nothing selects yet, the same governor runs
behind a private adapter of its controller: its observations, input stamp,
advisory and pause are unchanged, and the advisory is sent as fixed text. See
[privacy-provenance.md](privacy-provenance.md#controller-adapters).
