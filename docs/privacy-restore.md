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
| Derived file operation for `write_file` and `edit_file` (`ReviewedFile`, `DerivedFileOperation`), made in the agent's own event before a call is reviewed, from the plan the agent issued | implemented, private |
| Decision on it by the approval gate's policy or a person at its prompt, for the root agent, recorded where it is taken and carried to the terminal | implemented, private |
| The same for every delegated agent, in its own lane and at its own gate | implemented, private |
| A parked call: the registered reader shows the operation's preview under the digest the agent carries, and the person's sealed input is checked at the file | implemented, private; nothing in the application parks a call yet |
| The file tool's own workspace checkpoint, language diagnostics and result around a derived write | kept; the tool takes only its write from the derived operation |
| The user's rule for file content (`CapturedUserPolicy::restorable_file_data`) | interface only; nothing implements the policy outside owned test fixtures, so no normal session can select any of this |
| Typing a turn into a privacy conversation in the terminal, the secret and bootstrap interface, `/clear` | not implemented here; another owner's |
| Memory files, MCP, gadget, shell, web and every other tool argument | not restored |
| Delete, rename, batch plans | unchanged; no restoration |
| A sealed human input for a decision taken at an inline prompt, without a parked call | local registered producer and opaque UI request/presentation/decision core; A's real receiver/modal/choice consumer and independent validation remain pending |

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
data format for that file (`CapturedUserPolicy::restorable_file_data`). The
policy is asked about the file the operation is bound to, as an absolute
path the host resolved when the operation was derived: the opened file for
an edit, the observed directory and the new name for a create. It is never
asked about the path as the model spelled it, and it is asked again at the
write. A file's name or extension is not a rule. Without a rule the tool
answers with a fixed refusal and nothing is written.

The session that is built for the conversation keeps the user's policy. The
file review, the boundaries and the file tools refer to it weakly, so a tool
or a delegated agent that outlives the session keeps nothing of the policy's
owner alive; without the policy there is no rule.

**Derived at the agent's own event.** Before the agent sends a call through
its review, it issues a plan for that call and hands it to the
conversation's boundary (`before_review`). For a `write_file` or `edit_file`
that the last committed reply asked for and that names a placeholder, the
boundary of that agent's lane derives there, once, what the call will
really do:

- a create binds the absence of the file and holds the full new content;
- an edit opens the file once, reads it through that handle, and holds the
  real old file and the real new file. A file that changed is not edited as
  if it had not.

The result, a `ReviewedFile`, is the source the agent keeps for that review,
and the agent carries its digest beside the plan. The digest covers the call
as the model wrote it, the data format, the bound destination and both
contents. Nothing else makes such an operation: not a call, an id, equal
bytes, a digest or a label. An operation that cannot be derived holds the
refusal, which is the tool's answer before anyone is asked.

**Whether a call names a placeholder** is what the conversation recorded
for the reply that wrote the call, never how a string looks. An alias for a
telephone number is a number and carries no marker; it is found like any
other. No file tool of a privacy conversation decides this by a marker.

### Decisions

A gate that reviews the call takes the operation from the dispatch it is
deciding, shows the real content and decides on it. Restoration grants no
permission: the gate's permission mode, its switches, its allow-lists and
the effect of "always" are the existing ones and apply as to any other
call. A policy that already approves file changes without asking still
does; nobody is asked again merely because the content has values.

Whoever decides, the decision is recorded by the operation itself, where it
is taken, as a `FileDecision`:

- **The gate's automatic policy.** The operation evaluates the gate's own
  policy state on the arguments with the real content. An approval is
  recorded then; a denial ends the operation; otherwise a person is asked.
- **A person at the gate's prompt.** The operation hands the gate's prompt
  its preview, and records the answer when the prompt returns.

An approval is handed to the agent, which carries it to the call's terminal
(`TrackedDispatch::note_decision`). Carrying it approves nothing.

- **The root agent's** gate is the approval gate in the dispatch chain.
- **A delegated agent** has the same two file tools in its own lane of the
  same conversation, behind the gate every delegated tool has. That gate
  decides inside the call's terminal, on the operation the delegated agent's
  own boundary derived, and its decision is carried in the delegated lane. A
  gate that cannot ask anyone refuses, as for any delegated write.
- **A parked call** carries its operation and digest with it. The reader
  registered for a person's input takes the operation's preview under the
  digest the agent hands it and shows that; the person's sealed input then
  carries the same digest. A reader that does not take the preview has
  shown nothing, and its answer is not a decision on the operation. Nothing
  in the application parks a call today.

A decision at an inline prompt is the operation's own record, not a sealed
human input: that input exists only for a parked call.

**Checked again where it runs.** The file tool of a privacy conversation
(`RestoredFileTool`) stands directly on the tool, behind every gate. It
requires the invocation the agent issued for the call, the agent's own
boundary and the operation that boundary checked for exactly that
invocation, and then lets the wrapped tool run. Before the write, the
operation checks that:

- the conversation is live;
- the invocation is the one that was planned, with this operation as its
  review source and this operation's digest;
- the call runs exactly as it was planned, was asked for by the same
  committed reply, and no person changed it;
- the decision the invocation carries is a decision on this operation: one
  this operation issued itself, approving and not used before; or the sealed
  input of a person about the parked call, approving it as it is, with this
  digest, after the preview was taken;
- the user's policy still names the same data format for the bound
  destination;
- the destination is what it was. A create is made with the host's own
  refusal of an existing file, after the directory is checked against what
  it was. An edit checks that the path still names the opened file and that
  the file still holds what was read, and writes through the same handle.

An operation runs once. When it is over, written or refused, it releases the
content and the open file; what the agent keeps of it afterwards holds
neither.

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

**Refusals** are the tool's answer, before any effect, and the call stays as
the model wrote it: no rule, an alias where it is not restored, an alias
nobody validated, content that is not the named format, no decision or a
decision of another operation, a destination that changed. The tool's ordinary
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
- A decision at an inline prompt is recorded by the operation when the
  gate's prompt returns. It is that operation's own record of what the gate
  did, not a sealed human input, which exists only for a parked call.
- Nothing in the application parks a call. The registered reader's part is
  proven with a gate of the fixtures that parks, through the same function
  the application's reader uses.
- The policy is asked about a resolved path. Between that resolution and
  the write a directory above the file can still be replaced by the same
  user; the checks at the write are the ones listed above.
- A person who replaces the arguments at an approval writes their own text;
  nothing is restored into it, what was derived from the model's call is
  over, and the person's arguments run as they are. The conversation does
  not accept a person's text that holds one of its aliases, with a marker
  or without one: it stops before the call runs.
- A delegated agent's text is not drawn by a frontend, so it has no human
  view; its final answer returns to the root agent as a tool result.
- No normal session can reach any of this: the user's policy has no owner
  outside fixtures, and the terminal has no entry for a privacy turn.
  Declared verification, context compilation and failure controls are now
  installed behind the conversation's adapters; see
  [privacy-provenance.md](privacy-provenance.md), "Controller adapters".

## What is still to supply

The agent's event for a planned call (`before_review`,
`ActualPlannedInvocation`), the digest it carries, the parked call's
`shown()` and `TrackedDispatch::note_decision` are in the base now and are
used as described above. What remains:

- **A sealed human input for an inline decision.** `HumanDecisionInput` is
  made from an `ActualParkedCall`. The approval gate and a delegated agent's
  gate ask a person without parking, so the person's decision there is the
  operation's own `FileDecision`. To make it the same sealed input, its
  producer needs an entry for a planned call that was not parked, for
  example
  `HumanDecisionProducer::receive_planned(&ActualPlannedInvocation, Option<&SourceRef>, Option<[u8; 32]>)`,
  and the two gates need that producer.
- **The owner of the user's policy** and its rule for file data
  (`restorable_file_data`), which now takes the bound destination.
- **A terminal entry for a privacy turn**, and a caller for the registered
  reader: something that parks a call and asks through
  `PrivacyView::request_decision`.

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
}
impl ConversationBoundary {
    // ModelViewBoundary::before_review and reviewed_operation_digest are implemented here.
    pub(crate) fn review_files(self: &Arc<Self>, review: Arc<FileReview>);
    pub fn member(&self, conversation: &SourceRef) -> Option<Arc<ConversationBoundary>>;
    pub fn is_root(&self) -> bool;
    pub(crate) fn show_replies(&self, display: Arc<ReplyDisplay>);
}
pub struct PlannedOperation;   // made only by the boundary, from the agent's plan
pub struct ReviewedFile;       // the review source: the derived operation or its refusal
impl ReviewedFile {
    pub(crate) fn of(tracked: &TrackedDispatch) -> Option<Arc<Self>>;
    pub(crate) fn of_parked(parked: &ActualParkedCall) -> Option<Arc<Self>>;
    pub fn derived(&self) -> Result<&DerivedFileOperation, FileRefusal>;
    pub(crate) fn shown_arguments(&self) -> Result<Value, FileRefusal>;
    pub(crate) fn policy_decides(self: &Arc<Self>, approval: &Mutex<ApprovalState>,
        tracked: &TrackedDispatch) -> ToolPermission;
    pub(crate) fn person_decides<T>(self: &Arc<Self>, tracked: &TrackedDispatch,
        ask: impl FnOnce(FilePreview<'_>) -> (bool, T)) -> Result<T, FileRefusal>;
    pub(crate) fn shown_to_reader<T>(&self, shown: Option<[u8; 32]>,
        read: impl FnOnce(FilePreview<'_>) -> T) -> Result<T, FileRefusal>;
}
pub struct FileDecision;       // made only by the operation it is about
pub(crate) fn call_to_show(parked: &ActualParkedCall, shown: Option<[u8; 32]>) -> Option<ToolCall>;
impl FileReview { pub fn new(policy: &Arc<dyn CapturedUserPolicy>, root: PathBuf) -> Arc<Self>; }
impl ApprovedOperation { pub fn is_for(&self, invocation: &ActualToolInvocation) -> bool; }
impl CommittedModelNode { pub fn is(&self, other: &CommittedModelNode) -> bool; }
trait CapturedUserPolicy { fn restorable_file_data(&self, workspace_sha256: &str,
    destination: &Path) -> Option<FileDataFormat>; /* default: None */ }
impl PrivacyView { pub(crate) fn with_policy(self, policy: Option<Arc<dyn CapturedUserPolicy>>) -> Self; }

pub enum HumanReply { Text(HumanText), Withheld(HumanRefusal) }
pub type HumanReplySink = Arc<dyn Fn(HumanReply) + Send + Sync>;
pub struct ReplyDisplay; // token_sink(), round_started()
pub struct RestoredFileTool; // new(inner, review): directly on the file tool
pub(crate) fn authorized_write() -> Option<Result<usize, FileRefusal>>; // for a file tool's handler

// privacy_entry
impl HumanDecisionInput { pub(crate) fn planned(&self) -> &ActualPlannedInvocation; }

// agent
pub struct SessionExtras { pub human_reply_sink: Option<HumanReplySink>, .. }
```

`PlannedOperation` is what the last committed reply proposes with one plan
the agent issued, before anything reviewed it. It approves nothing and is
distinct from `ApprovedOperation`, the operation the boundary checks when the
call reaches its terminal.


The canonical local core now retains inline decisions in independently
registered input/policy owners. The actual UI request binds its private issuer,
plan, lane, review source, preview and shown digest; only the original recorded
reply can be consumed. The application must still bind the genuine receiver,
modal and choice events—these private data objects alone prove no human input
or file/application permission. Earlier operation-owned decision descriptions
above record the incomplete candidate; no such record grants current authority.
The real final file/source/destination checks and all remaining activation/native
gates stay required.

### What an inline decision keeps

A gate's registered producer decides a plan once, during the agent's review of
it, and keeps nothing of the operation afterwards.

- **A decision that is not an allow ends the operation where it is taken.** A
  denial, no answer, a missing private UI, an answer that cannot be consumed
  or admitted, arguments that are not JSON, a decision that cannot be attached
  and a turn stopped while a person is asked all release what the derived
  operation holds, its content and for an edit the opened file, when the gate
  returns. An allowed operation is released by the file tool after its write,
  as before.
- **The gate remembers by identity.** A plan is known while the agent reviews
  it and forgotten afterwards. An input and a request are known for as long as
  something else refers to them. The gate keeps none of them alive.
- **What was shown is not copied into the decision.** A request holds the call
  it shows, the summary and the details until its answer is consumed, and a
  presentation holds them while it is read. A receiver that keeps a request or
  a decision keeps no preview. When the policy is asked again at the terminal,
  the shown arguments are read from the operation itself.
- **What is kept is unchanged:** the recorded call and result, the person's
  answer as it was typed for as long as the agent's terminal refers to the
  decision, and every check of the plan, the gate, the registered UI, the
  current policy and single use.

The producer of the parked reader is not changed: it keeps every input it
read, as its tests require. All of this is housekeeping of memory in use. It
encrypts nothing, overwrites no released memory and changes nothing that is
stored.
