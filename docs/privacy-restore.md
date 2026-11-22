# Privacy restoration: what a person reads and what reaches a file

In a privacy conversation the model sees and writes placeholders. The user
asked for the values to come back in two places: in the replies they read on
their own terminal, and in the files the model writes for them. This page
records what is implemented for that, what is refused, and what is still to
do. Nothing here is selected by a frontend, a configuration or a command, and
normal privacy stays inactive.

The recorded conversation is never changed by any of this. The reply, the
tool call, its result, usage, the journal and the checkpoints stay as the
model wrote and received them, with placeholders. What a person reads and
what is written to disk are separate views derived from that record.

## State

| Part | State |
| --- | --- |
| Human view of a committed reply (`PrivacySession::human_reply`) | implemented, private |
| Hold on a streamed reply until it is committed (`HeldStream`) | implemented, private; no terminal uses it yet |
| Derived file operation for `write_file` and `edit_file` (`DerivedFileOperation`) | implemented, private |
| Review of the derived operation in the approval gate, and the file tools that run it, for the root agent in the shared session builder | implemented, private |
| The user's rule for file content (`CapturedUserPolicy::restorable_file_data`) | interface only; nothing in this branch implements the policy outside tests |
| File writes with placeholders by a delegated agent | refused |
| Memory files, MCP, gadget, shell, web and every other tool argument | not restored |
| Delete, rename, batch plans | unchanged; no restoration |
| Drawing the human view in the terminal, `/clear`, typed input | not implemented |
| Decision witness for the approval that is bound to a derived operation | waits for the sealed human producer of the framework |

## What a person reads

`PrivacySession::human_reply(source)` takes the source the agent keeps beside
a recorded reply and returns a `HumanText`.

- **Authority is the reply itself.** The source must be a reply this live
  conversation received and committed. A string, a spelling of an alias, a
  role, a position or a copy of a field grants nothing.
- **Only recorded occurrences are restored.** When a reply is committed, the
  conversation records where it names placeholders that this conversation
  issued. Each such occurrence is checked again when the view is made: the
  text at that place must be the alias of the live record of the commit that
  issued it, standing there as a whole token. Then the value is put there.
- **Everything else stays.** An alias nobody issued here, an altered one, one
  from another conversation and one inside a longer token stay as they are.
  If two recorded occurrences overlap, nothing in that reply is restored.
- **The view is its own type.** `HumanText` can only be read through a
  callback, while the conversation is live. It has no copy, no encoding and
  no debug text. It is not written to the record, a source, a checkpoint or
  a tool call, and it is not sent to a model. What the callback copies out
  cannot be taken back.
- **Bounds** are the restorer's: 256 KiB of text and 4096 replacements. A
  reply beyond them has no human view; the reply itself is kept.

Drawing the text is the frontend's work, through the same safe backend as
every other cell. No frontend does it yet.

### While a reply streams

A placeholder can arrive in two pieces. A person must see neither a piece of
one nor a value twice, so a streamed reply is not shown while it streams.
`HeldStream` is the sink a privacy conversation gives its provider in place
of the terminal's. It keeps the masked text, up to 256 KiB, and shows
nothing. When the reply is committed, `HeldStream::committed` makes the human
view from the committed reply, never from the stream, and says how the stream
compares with it: the whole text, nothing, something else, or more than the
hold keeps. When the turn is cancelled or fails first, `discard` drops what
was held; nothing of it was shown and nothing is made up in its place.

## What reaches a file

The call stays as the model wrote it. What is written is derived from it,
separately.

**What is restored.** The `content` of `write_file` and the `old_string` and
`new_string` of `edit_file`, as plain text or inside a complete JSON
document. In a JSON document a value goes only into a string value, encoded
as JSON; a key that holds an alias refuses the document. An alias that nobody
validated refuses the whole content: a file never gets an alias in place of a
value.

**What is never restored.** A path, a command, a URL, an MCP or gadget
argument, a credential, a setting, an instruction, a schema. An alias in the
path or in any other argument refuses the operation.

**The rule.** Content is restored only if the user's captured policy names a
data format for that file (`CapturedUserPolicy::restorable_file_data`). A
file's name or extension is not a rule. Without a rule the tool answers with
a fixed refusal and nothing is written.

**Derived before review.** The approval gate asks for the derived operation
before it looks at the call (`FileReview::propose`, from
`ConversationBoundary::proposed_operation`). A person, the policy and the
permission mode then decide on the real content:

- a create binds the absence of the file and shows the full new content;
- an edit opens the file once, reads it through that handle, and shows the
  real old file and the real new file. A file that changed is not edited as
  if it had not.

The decision is bound to exactly what was shown, by a digest over the call,
the destination and both contents. A decision about anything else approves
nothing. Restoration grants no permission: plan mode, a denial and every
existing rule apply to the derived operation as to any other call. A policy
that already approves file changes without asking still does; nobody is
asked again merely because the content has values.

**Checked again where it runs.** The file tool of a privacy conversation
(`RestoredFileTool`) runs a call that names a placeholder only through the
operation that was derived and approved for it. Before it writes, it checks
that the conversation is live, that the operation was approved as shown and
has not run, that the call running now was asked for by the same committed
reply with the same arguments and was not changed by a person, and that the
destination is what it was. A create is made with the host's own refusal of
an existing file, after the directory is checked against what it was. An edit
checks that the path still names the opened file and that the file still
holds what was read, and writes through the same handle. This also holds for
a call that was parked and resumed. An operation runs once.

**Refusals** are the tool's answer, before any effect, and the call stays as
the model wrote it: no rule, an alias where it is not restored, an alias
nobody validated, content that is not the named format, a decision that is
not about what was shown, a destination that changed. The tool's ordinary
answers stay as they are: the file exists, the text to replace is not there.

**The result** is the tool's own success text, with the path as the call
wrote it, and goes back to the model through its ordinary projection.

## Limits

- Every check happens before the write, because written content cannot be
  taken back. A check after the write would not be a rollback.
- Between the last check and the write another process can still act. A
  create is atomic against an existing file, not against a replaced
  directory. An edit writes through the handle that was read; it is not an
  atomic replacement. On Windows there is no file identity here: an edit
  still goes through the one open handle, and a create has only its refusal
  of an existing file.
- An edit is derived for files up to 256 KiB.
- Language diagnostics and workspace checkpoints of the ordinary file tool
  are not run for a derived write.
- A delegated agent's file tools refuse a call that names a placeholder.
  They have no review of their own yet, and they recognize a placeholder
  only by its marker: an alias without one, such as a numeric one, is not
  recognized there and would be written as it is.
- The approval that is bound to a derived operation is the gate's own
  decision. It is not yet a sealed witness of what a person did.
- A person who replaces the arguments at an approval writes their own text;
  nothing is restored into it.

## Interfaces this adds

```rust
// privacy_restore
impl Restorer<'_> { pub fn occurrence(&self, text: &str, range: Range<usize>, record: usize)
    -> Result<Option<Zeroizing<String>>>; }
pub fn display_text(text: &str, restored: &[Restored]) -> Result<Zeroizing<String>>;
pub fn file_candidate(format: FileDataFormat, text: &str, restored: &[Restored])
    -> Result<Zeroizing<String>>;

// privacy_session
impl PrivacySession {
    pub fn human_reply(&self, source: &SourceRef) -> Result<HumanText, HumanRefusal>;
    pub fn derive_file_operation(&self, policy: &dyn CapturedUserPolicy,
        operation: &Arc<ProposedOperation>, root: &Path)
        -> Result<DerivedFileOperation, FileRefusal>;
}
impl ConversationBoundary {
    pub async fn proposed_operation(&self, call: &ToolCall)
        -> Result<Arc<ProposedOperation>, AgentError>;
}
impl CommittedModelNode { pub fn is(&self, other: &CommittedModelNode) -> bool; }
trait CapturedUserPolicy { fn restorable_file_data(&self, workspace_sha256: &str, path: &str)
    -> Option<FileDataFormat>; /* default: None */ }
```

`ProposedOperation` is what the last committed reply proposes with a call,
before anything reviewed it. It approves nothing. It exists because the
checked operation of a call is made only when the call is about to run,
which is after the review.
