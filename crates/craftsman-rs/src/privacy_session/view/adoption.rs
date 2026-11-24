//! Carrying the record of a retired privacy conversation into the
//! conversation that succeeds it on the same thread.
//!
//! A conversation is never rebound and its scope is never reused. When the
//! user changes a setting that needs a new session, the old conversation is
//! retired and a new one is opened after a new unlock, with a scope of its
//! own: the same thread, and a namespace that names how many scopes came
//! before it. Placeholders of the two scopes differ.
//!
//! What is carried, and how:
//!
//! - **The retired values.** While the old conversation is still live, its
//!   own record of every mapping is read once: placeholder, original and
//!   category. Nothing else can produce a [`RetiredConversation`]. It holds
//!   the originals in memory, zeroized on drop, until the adoption.
//! - **Seeding.** The new conversation first admits every retired value on
//!   its own, under its recorded category. From then on its ordinary
//!   admission refuses any text in which such a value is not reported, so a
//!   value the old conversation masked is masked in the new one or the
//!   adoption fails.
//! - **Raw text** (what a person typed, what a tool returned, host text) is
//!   admitted anew from the retired capture itself, through the ordinary
//!   admission with the detector. It is never taken from a view.
//! - **Model-origin text** (replies, call arguments, summaries) keeps its
//!   words. Each placeholder of the retired scope in it is replaced by the
//!   new scope's placeholder for the same value, and the result is checked
//!   by the new send guard. Nothing is restored into it.
//! - **The record itself** is not changed: the agent's recorded messages are
//!   the retired ones, with their roles, calls, ids and results.
//!
//! What is refused, before anything is adopted: a source that is not the
//! retired conversation's own or does not fit its recorded message; a reply
//! that was received with provider items, which cannot be carried to another
//! scope; a message of a delegated agent or of an installed controller; a
//! conversation that is not the successor the retirement itself named.
//!
//! No model, tool or hook runs here. The old originals are sent nowhere.

use super::*;
use craftsman_harness::provenance::ConversationAdopter;

/// A piece of what a model sees for an adopted message: raw text, admitted
/// anew by the adopting conversation from the retired capture. It goes out
/// as that admission's view.
fn host(node: AdmittedNode) -> AdditionPart {
    AdditionPart::Admitted(node)
}

/// A piece of what a model sees for an adopted message: model-origin text
/// in the adopting scope's placeholders. It goes out as it is.
fn model(text: Arc<str>) -> AdditionPart {
    AdditionPart::Carried {
        text: text.clone(),
        view: text,
    }
}

/// One mapping the retired scope held, as its own record gave it.
struct RetiredValue {
    placeholder: String,
    original: Zeroizing<String>,
    category: Category,
}

/// What a privacy conversation leaves for its successor: its identity, for
/// checking that a carried source is its own, and every private value its
/// scope held. Made only by [`PrivacySession::retiring`], from the live
/// conversation. It cannot be built from a string, a file or a caller's
/// state, and it is neither cloned nor serialized.
pub struct RetiredConversation {
    inner: Arc<Inner>,
    lane: Arc<Lane>,
    agent: ActualAgentOwner,
    values: Vec<RetiredValue>,
}

impl std::fmt::Debug for RetiredConversation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetiredConversation([private])")
    }
}

impl RetiredConversation {
    /// The binding of the one conversation that may adopt this record: the
    /// same thread and workspace, one scope further. `None` when the
    /// thread has no further scope to give: a scope is never used twice.
    pub(crate) fn successor(&self) -> Option<FreshConversationBinding> {
        let binding = &self.inner.binding;
        Some(FreshConversationBinding {
            thread_id: binding.thread_id.clone(),
            workspace: binding.workspace.clone(),
            generation: binding.generation.checked_add(1)?,
        })
    }

    /// Whether the conversation this was read from has ended.
    pub(crate) fn ended(&self) -> bool {
        self.inner.live().is_err()
    }
}

/// One placeholder of a retired scope: the placeholder, the adopting
/// scope's placeholder for the same value, and the value's category.
type Name = (String, String, Category);

/// Whether the parts of a recorded message are the conversation's own, are
/// exactly its recorded content, and can be renamed where they are
/// model-origin text.
fn parts_carry(parts: &[AdditionPart], content: &str, owner: &Arc<Inner>, names: &[Name]) -> bool {
    let mut recorded = String::with_capacity(content.len());
    for part in parts {
        match part {
            AdditionPart::Admitted(node) | AdditionPart::Quoted(node) => {
                if !Arc::ptr_eq(&node.inner.owner, owner) {
                    return false;
                }
                recorded.push_str(&node.inner.raw.inner.text);
            }
            AdditionPart::Model { node, text } => {
                if !Arc::ptr_eq(&node.inner.owner, owner) || renamed(text, names).is_none() {
                    return false;
                }
                recorded.push_str(text);
            }
            AdditionPart::Carried { text, view } => {
                if renamed(view, names).is_none() {
                    return false;
                }
                recorded.push_str(text);
            }
        }
    }
    recorded == content
}

/// Whether the arguments of every call can be renamed.
fn calls_carry(calls: &[ToolCall], names: &[Name]) -> bool {
    for call in calls {
        if renamed_value(&call.args, names).is_none() {
            return false;
        }
    }
    true
}

/// Whether an adoption carries the message that `first` recorded with the
/// source `held` beside it: the source is the conversation `owner`'s own,
/// on `lane`, it fits that message, and every model-origin text of it can
/// be renamed with `names`. It admits, seals and changes nothing.
///
/// This one function decides before a retirement, for the live
/// conversation, and in the adoption, for the retired one. What only the
/// adopting scope can decide is not decided here: its own admission of the
/// raw text and its send guard's reading of the renamed text.
fn carries(
    held: &ViewSource,
    first: &ActualRetainedCommit,
    owner: &Arc<Inner>,
    lane: &Arc<Lane>,
    names: &[Name],
) -> bool {
    let message = first.message();
    match (held, message) {
        (ViewSource::Admitted(node), Msg::User { content }) => {
            Arc::ptr_eq(&node.inner.owner, owner) && node.captured_text_is(content)
        }
        (
            ViewSource::Result {
                view,
                actual,
                commit: kept,
            },
            Msg::Tool {
                tool_call_id,
                content,
                ..
            },
        ) => {
            kept.same_commit(first)
                && actual.call_id == *tool_call_id
                && Arc::ptr_eq(&actual.owner, owner)
                && Arc::ptr_eq(&actual.lane, lane)
                && match view {
                    ResultView::Whole(node) => {
                        Arc::ptr_eq(&node.inner.owner, owner) && node.captured_text_is(content)
                    }
                    ResultView::Parts(parts) => parts_carry(parts, content, owner, names),
                }
        }
        (
            ViewSource::Framework {
                node, commit: kept, ..
            },
            Msg::Tool { content, .. } | Msg::Assistant { content, .. },
        ) => {
            kept.same_commit(first)
                && Arc::ptr_eq(&node.inner.owner, owner)
                && node.captured_text_is(content)
                && !matches!(message, Msg::Assistant { tool_calls, .. } if !tool_calls.is_empty())
        }
        (
            ViewSource::Envelope {
                parts,
                commit: kept,
            }
            | ViewSource::HostNote {
                parts,
                commit: Some(kept),
                ..
            },
            Msg::User { content },
        ) => kept.same_commit(first) && parts_carry(parts, content, owner, names),
        (
            ViewSource::Summary {
                commit: Some(kept),
                open,
                text,
                close,
                ..
            },
            Msg::User { content },
        ) => {
            kept.same_commit(first)
                && Arc::ptr_eq(&open.inner.owner, owner)
                && Arc::ptr_eq(&close.inner.owner, owner)
                && *content
                    == format!(
                        "{}{text}{}",
                        open.inner.raw.inner.text.as_str(),
                        close.inner.raw.inner.text.as_str()
                    )
                && renamed(text, names).is_some()
        }
        (
            ViewSource::Model(node),
            Msg::Assistant {
                content,
                tool_calls,
            },
        ) => {
            node.inner.commit.same_commit(first)
                && node.inner.message == *message
                && node.inner.native.is_none()
                && Arc::ptr_eq(&node.inner.owner, owner)
                && Arc::ptr_eq(&node.inner.lane, lane)
                && calls_carry(tool_calls, names)
                && renamed(content, names).is_some()
        }
        (
            ViewSource::Adopted {
                owner: adopter,
                lane: adopted_on,
                commit: kept,
                content: before,
                calls: asked,
            },
            _,
        ) => {
            if !kept.same_commit(first)
                || !Arc::ptr_eq(adopter, owner)
                || !Arc::ptr_eq(adopted_on, lane)
                || !calls_carry(asked, names)
            {
                return false;
            }
            for piece in before {
                let carried = match piece {
                    AdditionPart::Admitted(node) => Arc::ptr_eq(&node.inner.owner, owner),
                    AdditionPart::Carried { view, .. } => renamed(view, names).is_some(),
                    _ => false,
                };
                if !carried {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

/// Why a conversation cannot be carried over as it stands. Decided before
/// anything is retired; the conversation is kept.
pub(crate) const NOT_CARRIED_CONTROLLERS: &str = "This conversation runs with context, failure or verification controls, whose records cannot be carried into a new private scope yet. The setting was not changed.";
pub(crate) const NOT_CARRIED_RECORD: &str = "A recorded message of this conversation cannot be carried into a new private scope. The setting was not changed.";
pub(crate) const NOT_CARRIED_ROOM: &str = "The private vault has no room for this conversation's values in a new private scope. The setting was not changed.";

impl PrivacySession {
    /// Whether the conversation behind `boundary` can be carried over now,
    /// given every recorded message with the source the agent keeps beside
    /// it. Read-only. Each pair is decided by [`carries`], the function the
    /// adoption itself decides with, against this conversation's own
    /// placeholders.
    pub(crate) fn carriable(
        &self,
        boundary: &ConversationBoundary,
        recorded: Option<&[craftsman_harness::provenance::ActualSourcedMessage]>,
    ) -> Result<(), &'static str> {
        if self.inner.live().is_err()
            || !Arc::ptr_eq(&boundary.session.inner, &self.inner)
            || !boundary.is_root()
        {
            return Err(NOT_CARRIED_RECORD);
        }
        if boundary.controls.policy.get().is_some() || boundary.controls.completion.get().is_some()
        {
            return Err(NOT_CARRIED_CONTROLLERS);
        }
        // This scope's placeholders with their categories, read from the
        // conversation's own record. An admission that is running refuses
        // instead of being waited for.
        let Ok(ledger) = self.inner.operation.try_lock() else {
            return Err(NOT_CARRIED_RECORD);
        };
        let mut names: Vec<Name> = Vec::with_capacity(ledger.known.len());
        for known in &ledger.known {
            let category = match known.masked.resolution_records_for_host_fields() {
                Ok(records) => records.with_host_field(known.index, |field| field.category()),
                Err(error) => Err(error),
            };
            let Ok(category) = category else {
                return Err(NOT_CARRIED_RECORD);
            };
            names.push((known.placeholder.clone(), String::new(), category));
        }
        drop(ledger);
        // A message that has no source yet cannot be decided, and refuses.
        let Some(recorded) = recorded else {
            return Err(NOT_CARRIED_RECORD);
        };
        for message in recorded {
            let Some(held) = message.source().downcast::<ViewSource>() else {
                return Err(NOT_CARRIED_RECORD);
            };
            if !carries(&held, message.commit(), &self.inner, &boundary.lane, &names) {
                return Err(NOT_CARRIED_RECORD);
            }
        }
        Ok(())
    }

    /// Read-only: whether the vault has room now for a successor scope that
    /// holds as many mappings and sealed bytes as this conversation's scope.
    /// Asked before a retirement; nothing is reserved by it.
    pub(crate) async fn successor_has_room(&self) -> Result<(), String> {
        match self.inner.session.admits_successor_scope().await {
            Ok(true) => Ok(()),
            Ok(false) => Err(NOT_CARRIED_ROOM.to_owned()),
            Err(error) => Err(SessionStop::Vault(error).to_string()),
        }
    }

    /// Reads what the live conversation leaves for its successor. The
    /// conversation is not stopped here: its agent's own retirement closes
    /// it, and until then it can go on if that retirement is refused.
    pub(crate) fn retiring(
        &self,
        boundary: &ConversationBoundary,
    ) -> Result<RetiredConversation, SessionStop> {
        let inner = &self.inner;
        inner.live()?;
        if !Arc::ptr_eq(&boundary.session.inner, inner) || !boundary.is_root() {
            return Err(SessionStop::Unbound);
        }
        let Some(agent) = inner.agent_owner.lock().unwrap().clone() else {
            return Err(SessionStop::Unbound);
        };
        // No admission runs while the conversation is idle. One that does
        // refuses the retirement instead of being waited for.
        let Ok(ledger) = inner.operation.try_lock() else {
            return Err(SessionStop::Unbound);
        };
        let mut values = Vec::with_capacity(ledger.known.len());
        for known in &ledger.known {
            let value = known
                .masked
                .resolution_records_for_host_fields()
                .and_then(|records| {
                    records.with_host_field(known.index, |field| RetiredValue {
                        placeholder: field.known_alias().to_owned(),
                        original: Zeroizing::new(
                            field.original_for_validated_host_field().to_owned(),
                        ),
                        category: field.category(),
                    })
                })
                .map_err(SessionStop::Vault)?;
            values.push(value);
        }
        inner.live()?;
        Ok(RetiredConversation {
            inner: inner.clone(),
            lane: boundary.lane.clone(),
            agent,
            values,
        })
    }

    /// The adopter for the one agent of this conversation, which succeeds
    /// `retired`. Handed to that agent's adoption and to nothing else.
    pub(crate) fn adopter<'a>(
        self: &'a Arc<Self>,
        boundary: &'a ConversationBoundary,
        retired: &'a RetiredConversation,
    ) -> Adoption<'a> {
        Adoption {
            session: self,
            boundary,
            retired,
        }
    }
}

/// See [`PrivacySession::adopter`].
pub(crate) struct Adoption<'a> {
    session: &'a Arc<PrivacySession>,
    boundary: &'a ConversationBoundary,
    retired: &'a RetiredConversation,
}

/// `text` with every placeholder of the retired scope replaced by the
/// adopting scope's placeholder for the same value.
///
/// An occurrence counts where restoration would read it as a placeholder of
/// its category: as a whole token (`privacy_restore::token_boundary`). That
/// matters for a telephone placeholder, which is digits in the shape of its
/// original and can stand inside a longer number or inside another
/// telephone placeholder. Of two occurrences of which one lies within the
/// other, the longer one is taken. Two that overlap otherwise give `None`:
/// nothing is guessed.
///
/// What cannot be told apart: a number a model wrote by itself that stands
/// alone and equals a telephone placeholder. It is renamed like the
/// placeholder, as restoration would restore it.
fn renamed(text: &str, names: &[Name]) -> Option<String> {
    // Each occurrence as one number, so that plain numbers are put in
    // order: where it starts, then its length counted down, then which name
    // it is. Earliest first, and of two at one start the longer first.
    if text.len() > u32::MAX as usize || names.len() > 0xFFFF {
        return None;
    }
    let mut found: Vec<u64> = Vec::new();
    let mut index = 0;
    while index < names.len() {
        let (old, _, category) = &names[index];
        if old.is_empty() || old.len() > 0xFFFF {
            return None;
        }
        for (start, matched) in text.match_indices(old.as_str()) {
            let end = start + matched.len();
            if crate::privacy_restore::token_boundary(text, start, end, *category) {
                found.push(
                    (start as u64) << 32 | (0xFFFF - matched.len() as u64) << 16 | index as u64,
                );
            }
        }
        index += 1;
    }
    found.sort_unstable();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for key in found {
        let start = (key >> 32) as usize;
        let end = start + (0xFFFF - ((key >> 16) & 0xFFFF)) as usize;
        if start < at {
            if end <= at {
                // Within the occurrence already taken: the longer one.
                continue;
            }
            return None;
        }
        out.push_str(&text[at..start]);
        out.push_str(&names[(key & 0xFFFF) as usize].1);
        at = end;
    }
    out.push_str(&text[at..]);
    Some(out)
}

/// [`renamed`] for every string value of a call's arguments. Names of
/// members and everything that is not a string stay as they are.
fn renamed_value(value: &serde_json::Value, names: &[Name]) -> Option<serde_json::Value> {
    Some(match value {
        serde_json::Value::String(text) => serde_json::Value::String(renamed(text, names)?),
        serde_json::Value::Array(items) => {
            let mut renamed = Vec::with_capacity(items.len());
            for item in items {
                renamed.push(renamed_value(item, names)?);
            }
            serde_json::Value::Array(renamed)
        }
        serde_json::Value::Object(members) => {
            let mut renamed = serde_json::Map::new();
            for (name, member) in members {
                renamed.insert(name.clone(), renamed_value(member, names)?);
            }
            serde_json::Value::Object(renamed)
        }
        other => other.clone(),
    })
}

impl Adoption<'_> {
    fn unbound(&self) -> AgentError {
        fault(unbound(&self.session.inner))
    }

    /// The adopting conversation's admission of the capture behind a node
    /// of the retired conversation. A capture is admitted once.
    async fn admitted(
        &self,
        node: &AdmittedNode,
        seen: &mut Vec<(AdmittedNode, AdmittedNode)>,
    ) -> Result<AdmittedNode, AgentError> {
        if !Arc::ptr_eq(&node.inner.owner, &self.retired.inner) {
            return Err(self.unbound());
        }
        if let Some((_, admitted)) = seen
            .iter()
            .find(|(retired, _)| Arc::ptr_eq(&retired.inner, &node.inner))
        {
            return Ok(admitted.clone());
        }
        let admitted = self
            .session
            .admit_raw(&node.inner.raw)
            .await
            .map_err(refused)?;
        seen.push((node.clone(), admitted.clone()));
        Ok(admitted)
    }

    /// Model-origin text of the retired conversation in this scope's
    /// placeholders, after this conversation's guard checked it.
    fn written(&self, text: &str, names: &[Name]) -> Result<Arc<str>, AgentError> {
        let Some(text) = renamed(text, names) else {
            return Err(self.unbound());
        };
        if let Err(refusal) = self
            .session
            .inner
            .guard
            .seal(b"null".to_vec(), &[text.as_str()])
        {
            self.session.inner.stop();
            return Err(fault(SessionStop::Guard(refusal)));
        }
        Ok(Arc::from(text))
    }

    /// The pieces of a retired message that was made of parts, after
    /// checking that those parts are exactly its recorded content.
    async fn pieces(
        &self,
        parts: &[AdditionPart],
        content: &str,
        names: &[Name],
        seen: &mut Vec<(AdmittedNode, AdmittedNode)>,
    ) -> Result<Vec<AdditionPart>, AgentError> {
        let mut recorded = String::with_capacity(content.len());
        let mut pieces = Vec::with_capacity(parts.len());
        for part in parts {
            match part {
                AdditionPart::Admitted(node) | AdditionPart::Quoted(node) => {
                    recorded.push_str(&node.inner.raw.inner.text);
                    pieces.push(host(self.admitted(node, seen).await?));
                }
                AdditionPart::Model { node, text } => {
                    if !Arc::ptr_eq(&node.inner.owner, &self.retired.inner) {
                        return Err(self.unbound());
                    }
                    recorded.push_str(text);
                    pieces.push(model(self.written(text, names)?));
                }
                // Carried once before: recorded as it always was, and seen
                // through the retired scope's own rendering of it.
                AdditionPart::Carried { text, view } => {
                    recorded.push_str(text);
                    pieces.push(model(self.written(view, names)?));
                }
            }
        }
        if recorded != content {
            return Err(self.unbound());
        }
        Ok(pieces)
    }
}

impl ConversationAdopter for Adoption<'_> {
    fn adopted_sources<'a, 'b, 'f>(
        &'a self,
        adopted: &'b [ActualRetainedCommit],
    ) -> BoxedBoundaryFuture<'f, Vec<SourceRef>>
    where
        'a: 'f,
        'b: 'f,
        Self: 'f,
    {
        Box::pin(async move {
            let inner = &self.session.inner;
            let retired = self.retired;
            inner.live().map_err(fault)?;
            // The adopting conversation is the successor that the retirement
            // named, and the retired one has ended.
            let (new, old) = (&inner.binding, &retired.inner.binding);
            if !Arc::ptr_eq(&self.boundary.session.inner, inner)
                || !self.boundary.is_root()
                || Arc::ptr_eq(inner, &retired.inner)
                || !retired.ended()
                || new.thread_id != old.thread_id
                || new.workspace != old.workspace
                || Some(new.generation) != old.generation.checked_add(1)
            {
                return Err(self.unbound());
            }
            // Every value the retired scope held gets its mapping here
            // first, under its recorded category.
            let mut names = Vec::with_capacity(retired.values.len());
            for value in &retired.values {
                let capture = CapturedRaw {
                    inner: Arc::new(RawInner {
                        origin: RawOrigin::RetiredValue,
                        text: Zeroizing::new(value.original.to_string()),
                        known: Some(value.category),
                    }),
                };
                let node = self.session.admit_raw(&capture).await.map_err(refused)?;
                // The view of a value that is one whole span is its
                // placeholder.
                let placeholder = node.model_view().map_err(fault)?.to_owned();
                if node.replacements() != 1 || placeholder == *value.original {
                    return Err(self.unbound());
                }
                names.push((value.placeholder.clone(), placeholder, value.category));
            }
            let mut seen = Vec::new();
            let mut sources = Vec::with_capacity(adopted.len());
            for commit in adopted {
                self.boundary.require_agent(commit.owner())?;
                let Some(adoption) = commit.adopted() else {
                    return Err(self.unbound());
                };
                let original = adoption.original();
                let first = original.commit();
                let message = commit.message();
                if commit.event() != RetainedEvent::Adopted
                    || !first.owner().same_agent(&retired.agent)
                    || first.message() != message
                {
                    return Err(self.unbound());
                }
                let Some(held) = original.source().downcast::<ViewSource>() else {
                    return Err(self.unbound());
                };
                // The decision that was made before the retirement, made
                // again for the retired record with this scope's names.
                if !carries(&held, first, &retired.inner, &retired.lane, &names) {
                    return Err(self.unbound());
                }
                let mut calls = Vec::new();
                let content = match (&*held, message) {
                    (ViewSource::Admitted(node), Msg::User { content })
                        if node.captured_text_is(content) =>
                    {
                        vec![host(self.admitted(node, &mut seen).await?)]
                    }
                    (
                        ViewSource::Result {
                            view,
                            actual,
                            commit: kept,
                        },
                        Msg::Tool {
                            tool_call_id,
                            content,
                            ..
                        },
                    ) if kept.same_commit(first)
                        && actual.call_id == *tool_call_id
                        && Arc::ptr_eq(&actual.owner, &retired.inner)
                        && Arc::ptr_eq(&actual.lane, &retired.lane) =>
                    {
                        match view {
                            ResultView::Whole(node) if node.captured_text_is(content) => {
                                vec![host(self.admitted(node, &mut seen).await?)]
                            }
                            ResultView::Whole(_) => return Err(self.unbound()),
                            ResultView::Parts(parts) => {
                                self.pieces(parts, content, &names, &mut seen).await?
                            }
                        }
                    }
                    (
                        ViewSource::Framework {
                            node, commit: kept, ..
                        },
                        Msg::Tool { content, .. } | Msg::Assistant { content, .. },
                    ) if kept.same_commit(first)
                        && node.captured_text_is(content)
                        && !matches!(message, Msg::Assistant { tool_calls, .. } if !tool_calls.is_empty()) =>
                    {
                        vec![host(self.admitted(node, &mut seen).await?)]
                    }
                    (
                        ViewSource::Envelope {
                            parts,
                            commit: kept,
                        }
                        | ViewSource::HostNote {
                            parts,
                            commit: Some(kept),
                            ..
                        },
                        Msg::User { content },
                    ) if kept.same_commit(first) => {
                        self.pieces(parts, content, &names, &mut seen).await?
                    }
                    (
                        ViewSource::Summary {
                            commit: Some(kept),
                            open,
                            text,
                            close,
                            ..
                        },
                        Msg::User { content },
                    ) if kept.same_commit(first)
                        && Arc::ptr_eq(&open.inner.owner, &retired.inner)
                        && Arc::ptr_eq(&close.inner.owner, &retired.inner)
                        && *content
                            == format!(
                                "{}{text}{}",
                                open.inner.raw.inner.text.as_str(),
                                close.inner.raw.inner.text.as_str()
                            ) =>
                    {
                        vec![
                            host(self.admitted(open, &mut seen).await?),
                            model(self.written(text, &names)?),
                            host(self.admitted(close, &mut seen).await?),
                        ]
                    }
                    (
                        ViewSource::Model(node),
                        Msg::Assistant {
                            content,
                            tool_calls,
                        },
                    ) if node.inner.commit.same_commit(first)
                        && node.inner.message == *message
                        && node.inner.native.is_none()
                        && Arc::ptr_eq(&node.inner.owner, &retired.inner)
                        && Arc::ptr_eq(&node.inner.lane, &retired.lane) =>
                    {
                        for call in tool_calls {
                            let Some(args) = renamed_value(&call.args, &names) else {
                                return Err(self.unbound());
                            };
                            // The guard reads the arguments as they will be
                            // encoded.
                            self.written(&args.to_string(), &[])?;
                            calls.push(ToolCall {
                                id: call.id.clone(),
                                name: call.name.clone(),
                                args,
                            });
                        }
                        vec![model(self.written(content, &names)?)]
                    }
                    // A message the retired conversation had adopted itself:
                    // its raw pieces are admitted anew from their captures,
                    // and its model-origin text is renamed once more.
                    (
                        ViewSource::Adopted {
                            owner,
                            lane,
                            commit: kept,
                            content: before,
                            calls: asked,
                        },
                        _,
                    ) if kept.same_commit(first)
                        && Arc::ptr_eq(owner, &retired.inner)
                        && Arc::ptr_eq(lane, &retired.lane) =>
                    {
                        for call in asked {
                            let Some(args) = renamed_value(&call.args, &names) else {
                                return Err(self.unbound());
                            };
                            self.written(&args.to_string(), &[])?;
                            calls.push(ToolCall {
                                id: call.id.clone(),
                                name: call.name.clone(),
                                args,
                            });
                        }
                        let mut pieces = Vec::with_capacity(before.len());
                        for piece in before {
                            pieces.push(match piece {
                                AdditionPart::Admitted(node) => {
                                    host(self.admitted(node, &mut seen).await?)
                                }
                                AdditionPart::Carried { view, .. } => {
                                    model(self.written(view, &names)?)
                                }
                                _ => return Err(self.unbound()),
                            });
                        }
                        pieces
                    }
                    _ => return Err(self.unbound()),
                };
                inner.live().map_err(fault)?;
                sources.push(SourceRef::new(Arc::new(ViewSource::Adopted {
                    owner: inner.clone(),
                    lane: self.boundary.lane.clone(),
                    commit: commit.clone(),
                    content,
                    calls,
                })));
            }
            Ok(sources)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Name, renamed};
    use crate::privacy_vault::Category;

    fn name(old: &str, new: &str, category: Category) -> Name {
        (old.to_owned(), new.to_owned(), category)
    }

    #[test]
    fn a_retired_placeholder_is_renamed_where_restoration_would_read_it_as_one() {
        let phone = name("203 918 4420", "771 030 5586", Category::Telephone);
        let mail = name(
            "pv1-00112233445566778899aabbccddeeff@privacy.invalid",
            "pv1-ffeeddccbbaa99887766554433221100@privacy.invalid",
            Category::Email,
        );
        let names = [phone, mail];
        // Standing alone, each is renamed, more than once and next to
        // punctuation.
        assert_eq!(
            renamed(
                "call 203 918 4420, or \"203 918 4420\"; write <pv1-00112233445566778899aabbccddeeff@privacy.invalid>",
                &names
            )
            .as_deref(),
            Some(
                "call 771 030 5586, or \"771 030 5586\"; write <pv1-ffeeddccbbaa99887766554433221100@privacy.invalid>"
            )
        );
        // The same digits as a piece of a longer number are not the
        // placeholder: directly attached, or one more group of a spaced or
        // dotted number before or after it.
        for longer in [
            "id 9203 918 4420",
            "id 203 918 44207",
            "+203 918 4420",
            "1 203 918 4420",
            "203 918 4420 55",
            "203 918 4420.55",
            "x203 918 4420",
            // A parenthesis belongs to the notation of a number.
            "(203 918 4420)",
        ] {
            assert_eq!(renamed(longer, &names).as_deref(), Some(longer), "{longer}");
        }
        // A number that a model wrote by itself and that equals the
        // placeholder, standing alone, cannot be told from it and is renamed.
        assert_eq!(
            renamed("the sum is 203 918 4420", &names).as_deref(),
            Some("the sum is 771 030 5586")
        );
        // Of two occurrences of which one lies within the other, the longer
        // is taken; two that overlap otherwise are not guessed about.
        let outer = name("a@b c@d", "OUTER", Category::Email);
        let inner = name("c@d", "INNER", Category::Email);
        let across = name("c@d e@f", "ACROSS", Category::Email);
        assert_eq!(
            renamed("see a@b c@d, then c@d", &[inner.clone(), outer.clone()]).as_deref(),
            Some("see OUTER, then INNER")
        );
        assert_eq!(renamed("see a@b c@d e@f", &[outer, across]), None);
        // Without names the text is as it was.
        assert_eq!(
            renamed("203 918 4420", &[]).as_deref(),
            Some("203 918 4420")
        );
    }
}
