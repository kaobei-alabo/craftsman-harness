# Declared verification and protected changes

The terminal and ACP main agent can run declared checks before accepting a
completed change. Failed checks return their actual output to the model for
repair; a passing result belongs to the observed source contents, not a final
answer's claim. This is an app-selected completion policy. Framework, legacy,
private headless, frozen and delegated-agent defaults remain unchanged.

## Declare checks and their scope

Set `verification` in user configuration, or add `.craftsman/verification.json`
to a project. User settings win. The project file proposes only verification
commands/scopes; it cannot set credentials, environment passing, services or
permission mode. The project list needs approval even under ordinary bypass
switches; that approval belongs to the captured list. Commands stay fixed for the session, and project policy changes
refuse further checks until the session is rebuilt.

```json
{
  "commands": ["YOUR_TEST_COMMAND", "YOUR_OTHER_CHECK_COMMAND"],
  "paths": ["src", "tests"],
  "protected_paths": ["tests"],
  "max_repair_rounds": 3
}
```

Use that object as the project file, or nest it under `verification` in
`~/.craftsman/config.json`; [a user-config example](../examples/config/verification.json)
includes placeholders. Missing or empty commands disable the feature. Declare
1–64 relative source paths and at most 16 commands, with 4 KiB per command and
32 KiB combined. Protected paths must be inside the source scopes. No Git
worktree is required, and several project folders or document files can be
declared. Checks run from the selected working folder and use the existing user
`shell.timeout_secs` setting (120 seconds by default). Longer suites need an
explicit user timeout configuration; project policy never increases that limit.

The repository provides [a project-policy example](../.craftsman/verification.example.json)
for local verification. Save it as `.craftsman/verification.json` after selecting
the appropriate command and scopes. It is reviewed through the same permission boundary.
The configured checks and protected paths are explicit; the feature does not
discover arbitrary commands or claim to protect unspecified files.

## Completion uses actual results

Source observation starts lazily before an allowed potentially mutating tool,
not when the user types a prompt. Read-only turns perform no verification scan
or automatic check. At a normal model completion, changed declared sources need
verification. The agent schedules the registered `verify` tool, saves that call
before its effects, and uses the existing approval/UI event path. The tool runs
the exact declared list through the existing owned `ShellRunner`/`ProcessTree`,
user environment scrub and operation limits. It records real exit codes and
output for every completed check in the tool result.

All checks must return a known zero exit without a tool error, and complete
source observations before/after checks must match. Checks that change sources,
incomplete coverage or oversized combined evidence cannot create a passing
result. Later source changes invalidate the result. The model samples the actual
tool results normally before its final answer. At most three repair rounds are
allowed; the next failing verification stops with completed work and results
retained. Denied or unperformed verification stops without another model request.
Cancellation and durable-save failures keep their existing stop/refusal rules.

Verification is one tool operation containing the declared command list. Its
completed per-command results are retained together; it is not a public headless
receipt producer or a promise to recover each internal command after abrupt exit.
Only explicit user continuation/review resumes work; reopening makes no automatic
model or tool call. Restored configured sessions mark the prior verification
baseline unavailable and require new checks on subsequent completion.

## Protected test-file review

Changed files inside `protected_paths` require approval of their current contents
before checks can establish success. The prompt shows full before/after text
within bounded review limits; after restore it shows current text with the prior
baseline explicitly unavailable. Approval binds to those protected contents.
Further changes require review again. This requirement remains independent of
ordinary bypass/always-allow switches. The plan/read-only mode does not execute
verification commands.

The guard withholds accepted completion; it does not undo already approved edits,
prove that arbitrary assertions have equivalent meaning, detect every test by
filename, or constrain native commands at the OS level. Declare the files that
must be reviewed, including mixed source/test files where needed. Binary or large
protected changes require external review rather than a truncated approval view.

## Observation and evidence limits

Observation uses the bounded no-follow file primitives also used for checkpoints,
without a checkpoint store or undo state. Verification walks declared scopes
directly, so changing `.gitignore` cannot hide protected files. Application state
and ordinary build directories remain excluded. Exact file scopes can cover
otherwise generated inputs where permitted. Each observation admits at most
4,096 entries, 1,024 files and 25 MiB of file hashing; incomplete observations
refuse a passing result. Review retains at most 64 KiB per protected file and
1 MiB combined; returned check evidence is also bounded to 1 MiB.

These are data/work bounds, not hard IO deadlines or an OS sandbox. Unix retains
no-follow file walks and nonblocking regular-file checks; Windows retains the
existing best-effort checks. Native blocking work, external writers, namespace
races and shutdown bounds keep their existing limits. Existing user
configuration/data, SQL layout, SDK/WIT, dependency pins, native limits and retained request/ACP frame
goldens are changed.

Agent-loop, local-source and marked actual builder/HTTP/subprocess/approval/SQLite
tests cover scheduling, real failure/repair, protected review/denial/staleness,
source-changing checks, read-only/legacy behavior, save refusal and cancellation.
Normal named Responses, Claude Messages and Gemini continuations retain original
opaque/signed history and actual usage around scheduled checks. Full local,
all four hosted jobs/every mandatory step/both native modes, inventory/warning/tree
review and independent-main validation remain required before delivery. Live
quality measurements and public headless/context/compiler/publication work remain
separate.
