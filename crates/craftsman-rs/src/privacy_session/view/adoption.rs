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

/// Why a conversation cannot be carried over as it stands. Decided before
/// anything is retired; the conversation is kept.
pub(crate) const NOT_CARRIED_CONTROLLERS: &str = "This conversation runs with context, failure or verification controls, whose records cannot be carried into a new private scope yet. The setting was not changed.";
pub(crate) const NOT_CARRIED_RECORD: &str = "A recorded message of this conversation cannot be carried into a new private scope. The setting was not changed.";

impl PrivacySession {
    /// Whether the conversation behind `boundary` can be carried over now,
    /// given the source the agent keeps beside each recorded message so far
    /// (`None` where it has not asked yet). Read-only.
    pub(crate) fn carriable(
        &self,
        boundary: &ConversationBoundary,
        sources: &[Option<SourceRef>],
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
        for source in sources.iter().flatten() {
            let held = source.downcast::<ViewSource>();
            let carried = match held.as_deref() {
                Some(ViewSource::Model(node)) => node.inner.native.is_none(),
                Some(
                    ViewSource::Admitted(_)
                    | ViewSource::Result { .. }
                    | ViewSource::Framework { .. }
                    | ViewSource::Envelope { .. }
                    | ViewSource::HostNote { .. }
                    | ViewSource::Summary { .. }
                    | ViewSource::Adopted { .. },
                ) => true,
                _ => false,
            };
            if !carried {
                return Err(NOT_CARRIED_RECORD);
            }
        }
        Ok(())
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
/// adopting scope's placeholder for the same value. `None` when two
/// occurrences overlap: nothing is guessed.
fn renamed(text: &str, names: &[(String, String)]) -> Option<String> {
    let mut found = Vec::new();
    for (old, new) in names {
        for (start, matched) in text.match_indices(old.as_str()) {
            found.push((start, start + matched.len(), new.as_str()));
        }
    }
    found.sort_by_key(|(start, end, _)| (*start, *end));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end, new) in found {
        if start < at {
            return None;
        }
        out.push_str(&text[at..start]);
        out.push_str(new);
        at = end;
    }
    out.push_str(&text[at..]);
    Some(out)
}

/// [`renamed`] for every string value of a call's arguments. Names of
/// members and everything that is not a string stay as they are.
fn renamed_value(
    value: &serde_json::Value,
    names: &[(String, String)],
) -> Option<serde_json::Value> {
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
    fn written(&self, text: &str, names: &[(String, String)]) -> Result<Arc<str>, AgentError> {
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
        names: &[(String, String)],
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
                names.push((value.placeholder.clone(), placeholder));
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
