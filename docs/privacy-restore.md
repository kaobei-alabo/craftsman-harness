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
| Hold on a streamed reply until it is committed (`HeldStream`) | implemented, private |
| A frontend's display of committed replies (`ReplyDisplay`, `SessionExtras::human_reply_sink`) | implemented, private; the terminal's bridge and application use it |
| Derived file operation for `write_file` and `edit_file` (`DerivedFileOperation`) | implemented, private |
| Review of the derived operation in the approval gate, for the root agent in the shared session builder | implemented, private |
| The same for every delegated agent, in its own lane and behind its own gate | implemented, private |
| The file tool's own workspace checkpoint, language diagnostics and result around a derived write | kept; the tool takes only its write from the derived operation |
| The user's rule for file content (`CapturedUserPolicy::restorable_file_data`) | interface only; nothing implements the policy outside owned test fixtures, so no normal session can select any of this |
| Typing a turn into a privacy conversation in the terminal, the secret and bootstrap interface, `/clear` | not implemented here; another owner's |
| Memory files, MCP, gadget, shell, web and every other tool argument | not restored |
| Delete, rename, batch plans | unchanged; no restoration |
| A framework event for a planned call before its review, and a sealed decision that carries the digest a person was shown | not in this base; see "What the framework still has to supply" |

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
  reply beyond them has no human view; the reply itself is kept, and a
  frontend that shows replies is told that this one is withheld. The masked
  text is not drawn in its place.

Drawing the text is the frontend's work, through the same safe backend as
every other cell.

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

### In a frontend

A frontend that shows replies gives the session builder a sink
(`SessionExtras::human_reply_sink`). For a privacy conversation the builder
then makes a `ReplyDisplay`:

- the root agent's provider streams into the display's hold, and never into
  the frontend's token sink;
- a round that begins drops what the round before it streamed and never
  committed;
- when the agent has retained a reply, the conversation's boundary tells the
  display in its own commit hook, and the frontend is given
  `HumanReply::Text`, once, made from the committed reply. A reply that is
  only calls gives nothing. If no view can be made, the frontend is given
  `HumanReply::Withheld` with the reason and nothing of the reply.

The sink has no effect on any other session, and a privacy conversation
without it streams as before. Delegated agents have no display: a frontend
does not draw their text.

The terminal registers this sink in its bridge. The application draws a
reply as one transcript item of its own, through the same cells and the same
safe backend as a streamed reply, and by the same rule as a stream: nothing
is added once the turn is stopping or over. The text is read while the
conversation is live; if it is not, a fixed line stands in its place. The
status reports of the terminal stay fixed words. The transcript is only
drawn; nothing in the terminal sends it back to a model, and a session that
is reopened is drawn from the record, which holds the placeholders.

What the terminal does not do: it has no way yet to type a turn into a
privacy conversation, and no secret or bootstrap interface. The fixtures
enter the conversation through its own typed entry.

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

**Derived before review.** For the root agent the approval gate asks for the
derived operation before it looks at the call (`FileReview::propose`, from
`ConversationBoundary::proposed_operation`). A person, the policy and the
permission mode then decide on the real content:

- a create binds the absence of the file and shows the full new content;
- an edit opens the file once, reads it through that handle, and shows the
  real old file and the real new file. A file that changed is not edited as
  if it had not.

The decision is bound to exactly what was shown, by a digest over the call
as the model wrote it, the data format, the destination and both contents.
The call's id alone names nothing. A decision about anything else approves
nothing. Restoration grants no permission: plan mode, a denial and every
existing rule apply to the derived operation as to any other call. A policy
that already approves file changes without asking still does; nobody is
asked again merely because the content has values.

**Whether a call names a placeholder** is what the conversation recorded
for the reply that wrote the call, never how a string looks. An alias for a
telephone number is a number and carries no marker; it is found like any
other. No file tool of a privacy conversation decides this by a marker.

**Checked again where it runs.** The file tool of a privacy conversation
(`RestoredFileTool`) runs a call that names a placeholder only through the
operation that was derived and approved for it. It finds the agent the call
runs in by that agent's own boundary, requires the operation the boundary
checked for exactly the invocation the agent issued, and then lets the
wrapped tool run. Before the write, the operation checks that the
conversation is live, that it was approved as shown and has not run, that
the call running now was asked for by the same committed reply with the same
arguments and was not changed by a person, and that the destination is what
it was. A create is made with the host's own refusal of
an existing file, after the directory is checked against what it was. An edit
checks that the path still names the opened file and that the file still
holds what was read, and writes through the same handle. This also holds for
a call that was parked and resumed. An operation runs once.

**The file tool itself runs.** The wrapped tool is the ordinary one. It
notes the file and begins its workspace checkpoint, performs the write,
finishes the checkpoint, asks for language diagnostics and returns its own
result, as for any write. The only difference is the write: where an
operation was derived for the running call, the tool takes it from
`authorized_write` instead of writing its arguments. A session without a
review never has such an operation, and its file tools do exactly what they
always did. What the tool adds to its result after the path, such as
diagnostics that quote the file, is host text: the record keeps it as the
tool returned it and the model is given its view.

**A delegated agent** has the same two tools, in its own lane of the same
conversation and with the same record of placeholders. Its gate is inside
its tools, so the order is: derive, gate, tool.

- The operation is derived from the delegated agent's own committed reply
  and checked call, before its gate is asked.
- The gate is handed the call's arguments with the real content. Its
  permission rules, its prompt and its activity line are the existing ones
  and are about what will really be written. A gate that cannot ask anyone
  refuses, as for any delegated write.
- Directly behind the gate, `ReachedFileTool` records that the gate let the
  call through with exactly the arguments that were shown. A gate that
  refuses never gets there.
- Then the ordinary file tool runs, as above.

One `FileReview` serves the root agent and every delegated agent. It finds
each agent by its own boundary, and an operation derived for one agent is
never used for another. Holding the review, the session or its record grants
no access to a file and approves nothing.

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
- A create needs its directory to exist. The ordinary tool makes missing
  directories; a derived create binds the directory it goes into and
  refuses when there is none.
- A memory file is kept in the application's store and is never a derived
  operation: a call that names a placeholder for one is refused.
- The workspace checkpoint keeps the before-image of the file in the local
  store, as for any write. Nothing here encrypts or redacts stored data.
- Language diagnostics were proven with a file tool of the fixtures that
  reports what a checker says; no fixture runs a real language server.
- The approval that is bound to a derived operation is the gate's own
  decision: the approval gate's for the root agent, the delegated agent's
  gate letting the call through for a delegated one. It is not yet a sealed
  witness of what a person did.
- A person who replaces the arguments at an approval writes their own text;
  nothing is restored into it, what was derived from the model's call is
  over, and the person's arguments run as they are. The conversation does
  not accept a person's text that holds one of its aliases, with a marker
  or without one: it stops before the call runs.
- A delegated agent's text is not drawn by a frontend, so it has no human
  view; its final answer returns to the root agent as a tool result.
- No normal session can reach any of this: the user's policy has no owner
  outside fixtures, the terminal has no entry for a privacy turn, and the
  builder still refuses a selection together with verification, context
  and failure controls or local gadgets.

## What the framework still has to supply

Two things are not in this base and are not made up here.

**An event for a planned call before its review.** The agent issues its
`ActualToolInvocation` only when a call is about to run, after every review.
`ProposedOperation` is therefore made from the committed reply and the call,
and the binding to the invocation happens at the tool
(`ApprovedOperation::is_for`). What is needed:

```rust
// craftsman_harness::provenance::ModelViewBoundary
/// Called before the dispatch chain reviews a call, with the invocation
/// the agent will run if the review lets it through.
fn before_review<'a>(&'a self, planned: &'a ActualPlannedInvocation)
    -> BoxFuture<'a, Result<Option<SourceRef>, AgentError>>;

// craftsman_harness::provenance_facts
pub struct ActualPlannedInvocation { /* owner, call; made only by the agent */ }
impl ActualPlannedInvocation {
    pub fn owner(&self) -> &ActualAgentOwner;
    pub fn call(&self) -> &ToolCall;
}
impl ActualToolInvocation {
    /// Whether this is the invocation that was planned as `planned`.
    pub fn was_planned_as(&self, planned: &ActualPlannedInvocation) -> bool;
}
```

With it, `ProposedOperation` holds the planned invocation and
`DerivedFileOperation::execute` requires
`running.invocation.was_planned_as(..)` in place of comparing the reply and
the call.

**A sealed decision that carries what was shown.** `approve(digest)` is
called by the gate with the decision it took. What is needed:

```rust
// the sealed human input of the conversation (privacy_entry)
impl HumanInput {
    /// The digest of the derived operation the person was shown when they
    /// decided, as the registered input callback was given it.
    pub fn shown(&self) -> Option<[u8; 32]>;
}
// craftsman_harness::provenance_facts::ActualToolInvocation
/// The sealed decision this invocation runs under, for a call that was
/// reviewed by a person. Already present for a resumed call.
pub fn decision_source(&self) -> Option<&SourceRef>;
```

With it, `execute` requires the decision of the running invocation to carry
this operation's digest, and `approve` goes away. For an approval a policy
gives without asking, the gate needs the same kind of sealed record of the
policy's decision.

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
    /// The boundary of this conversation that `conversation` is the evidence of.
    pub fn member(&self, conversation: &SourceRef) -> Option<Arc<ConversationBoundary>>;
    pub fn is_root(&self) -> bool;
    pub(crate) fn show_replies(&self, display: Arc<ReplyDisplay>);
}
impl ApprovedOperation { pub fn is_for(&self, invocation: &ActualToolInvocation) -> bool; }
impl CommittedModelNode { pub fn is(&self, other: &CommittedModelNode) -> bool; }
trait CapturedUserPolicy { fn restorable_file_data(&self, workspace_sha256: &str, path: &str)
    -> Option<FileDataFormat>; /* default: None */ }

pub enum HumanReply { Text(HumanText), Withheld(HumanRefusal) }
pub type HumanReplySink = Arc<dyn Fn(HumanReply) + Send + Sync>;
pub struct ReplyDisplay; // token_sink(), round_started()
pub struct RestoredFileTool; // new(inner, review): before a gate, or directly on the tool
pub struct ReachedFileTool;  // new(inner): directly behind a delegated agent's gate
pub(crate) fn authorized_write() -> Option<Result<usize, FileRefusal>>; // for a file tool's handler

// agent
pub struct SessionExtras { pub human_reply_sink: Option<HumanReplySink>, .. }
```

`ProposedOperation` is what the last committed reply proposes with a call,
before anything reviewed it. It approves nothing. It exists because the
checked operation of a call is made only when the call is about to run,
which is after the review.
