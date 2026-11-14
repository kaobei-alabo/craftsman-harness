# Declared-verifier model check — November 14, 2026

This bounded model check compares the prior workflow runtime (`e2c052f`, whose
Rust sources match the retained `cec77e4` image) with the declared-verification
candidate (`5d912c4`). Both immutable local images receive the same clarified
JavaScript ledger task and the same `.craftsman/verification.json` policy:
`node test.cjs`, source scopes `lib.cjs`/`test.cjs`, and protected `test.cjs`.

The model is the configured `gpt-6.1-sol` Responses route at `xhigh` effort, with
an 8,192-token output ceiling and 300-second task wall limit. Original fixtures
fail the grader and reference fixes pass. Only task files and the client script
are mounted in agent containers; held-out grading uses an offline source snapshot
after all agent processes stop. Real keys remain in the separate gateway.

| Recorded outcome | Prior workflow | Verifier candidate |
| --- | ---: | ---: |
| Successful graded turns | 1/1 | 1/1 |
| Actual stop reason | end_turn | end_turn |
| Provider requests | 6 | 6 |
| Tool calls | 11 | 10 |
| Elapsed seconds | 53.778 | 44.644 |
| Accounted cost at confirmed rates | $0.078242 | $0.074534 |

The candidate trace contains the actual verifier tool result:

```text
Verification passed.
Check: node test.cjs
Exit: 0
```

This verifies model/runtime composition with the declared policy. One matched
task does not establish a coding-quality, causal or speed improvement. The native
tests separately prove that attempted completion schedules checks when needed,
failed checks feed actual results into repair, stale/incomplete source evidence
cannot pass, and protected review/denial/cancellation/save failures retain their
respective boundaries. This live case did not weaken or delete tests and did not
exercise every failure mode.

Both attempts share the original conservative $25 ledger and confirmed $2/M
input, $10/M output, no-fee rates. Accounting is now **$23.153274 of $25**, including
the same **40 retained unknown-usage reservations**. This check accounts for
$0.152776; both new runs settled from reported usage. No reservation was reset or
refunded, and these amounts are admission accounting, not a provider invoice.

Sanitized traces, actual tool/check outputs, grades, bounded source snapshots,
corpus/policy/image hashes and ledger records remain under
`target/verification/verification-gate/`; the ledger remains under
`target/verification/coding-quality/live-budget/`. User configuration and keys
were preserved. Local evaluation images are not published binaries/containers.

[Declared verification](verification.md) is an explicit scoped completion policy.
Broader semantic test-integrity, context compilation, public headless receipts,
larger/repeated model comparisons and native containment retain separate work
and validation requirements.
