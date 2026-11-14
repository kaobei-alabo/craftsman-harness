# Long terminal tasks and turn limits

The interactive terminal's main agent allows **200 model rounds per turn** by
default. A round is one model response and the tool calls in that response; it
is not a count of files or shell commands. Longer tasks can therefore proceed
beyond the former inherited 50-round cutoff.

Add this to `~/.craftsman/config.json` to choose a different limit, then restart
Craftsman. The [multi-provider example](../examples/config/multi-provider.json)
includes the setting. Existing configuration, credentials and data are preserved.
`/status` and `/config` show the configured number of model rounds per turn.

```json
{
  "terminal": {
    "max_rounds": 200
  }
}
```

The setting accepts whole numbers from 1 through 1,000. Zero, null, strings,
fractions and out-of-range values refuse initialization before opening storage
or constructing a model connection. Missing settings use 200. This is a turn
budget, not a dollar cap; larger limits allow more model requests and expense.

When the budget is reached, Craftsman waits for every tool call in the last
round to return, then closes that turn and saves its actual results. The chat
shows **“Paused after … model rounds”** as an activity message. The composer
draft and cursor remain intact. No extra model request is made to summarize the
work, and no synthetic assistant answer or usage is inserted in the transcript.

Send a follow-up such as `continue` to start another turn using the retained
conversation. Reopening a saved conversation also makes no model or tool call.
The model decides its next steps from the recorded results; normal approval
checks still apply. There is no automatic retry or replay of completed tools.
Permission prompts reached in the final round must be resolved normally before
the turn can close.

Input accepted while the last round runs is saved before the pause. New steering
and concurrent turns are refused while end hooks and the final save are still
running. Cancellation before limit completion retains the normal stopped-turn
outcome. A failed durable save remains an error and refuses further sampling;
it cannot be displayed as a successfully saved pause.

The framework's default remains 50 rounds. Its additive
`AgentBuilder::finish_on_round_limit` option defaults false; only the owned
terminal main agent selects the configured budget and limit-completion hooks.
ACP keeps its existing standard `max_turn_requests` handling. One-shot, service,
private headless and delegated-agent policies remain unchanged. Shell, gadget,
storage and shutdown limits are independent of this setting.

These changes make a finite work limit recoverable. They do not establish that
a long task is making useful progress or eliminate model/tool failures.

Framework regressions cover original error semantics, exact tool results,
completed checkpoints, approved and denied calls, final-round cancellation/input,
failed saves and concurrent closure. Marked native subprocess fixtures use
isolated configuration, loopback HTTP and local SQLite to check a real 52-round
terminal task, an approved create/edit pause/reopen/follow-up, legacy defaults,
invalid-limit refusal and typed UI error handling. Unmarked/unknown fixture
startup returns before environment/configuration access. No paid model calls or
user configuration changes are made. Full local validation and all four hosted
jobs, every mandatory step, both native modes and independent-main validation
remain required before delivery.
