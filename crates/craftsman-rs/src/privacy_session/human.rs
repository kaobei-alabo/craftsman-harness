//! What a person may read of a reply of this conversation.
//!
//! The model sees and writes placeholders. A person reading the reply on
//! their own terminal may see the values behind them. This module makes that
//! view, and nothing else: it is a separate typed text, it is never written
//! into the recorded conversation, a source, a checkpoint or a tool call, and
//! it is never sent to a model. Drawing it is the frontend's work, through
//! the same safe backend as every other cell.
//!
//! Authority comes only from the reply itself: a committed reply of this
//! live conversation, and the occurrences that reply's own commit recorded
//! for placeholders this conversation issued. Each occurrence is checked
//! again, against the reply's text and against the live record of the commit
//! that issued the placeholder, when the view is made. A string, a spelling,
//! a role or a position grants nothing. A token that is unknown, altered,
//! from another conversation or inside a longer token stays as it is.

use super::view::FieldPlace;
use super::{Inner, PrivacySession, SessionStop};
use crate::privacy_restore::{RestoreError, Restored, Restorer, display_text};
use craftsman_harness::Msg;
use craftsman_harness::provenance::SourceRef;
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

/// Why no human view was made. A refusal of the restorer has already closed
/// the vault's scope, as that contract says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HumanRefusal {
    #[error("Privacy stopped this conversation. {0}")]
    Stopped(SessionStop),
    #[error("This is not a reply this conversation committed.")]
    NotAReply,
    #[error("{0}")]
    Restore(RestoreError),
}

/// A reply as a person may read it. It can only be read through
/// [`read`](Self::read), while the conversation is live. It has no copy, no
/// encoding and no debug text.
pub struct HumanText {
    owner: Arc<Inner>,
    text: Zeroizing<String>,
    restored: usize,
    unresolved: usize,
}

impl HumanText {
    /// The text, for the duration of `read`. The conversation is checked
    /// before and after; what `read` copied out cannot be taken back.
    pub fn read<T>(&self, read: impl FnOnce(&str) -> T) -> Result<T, SessionStop> {
        self.owner.live()?;
        let result = read(&self.text);
        self.owner.live()?;
        Ok(result)
    }

    /// How many occurrences were replaced by their values.
    pub fn restored(&self) -> usize {
        self.restored
    }

    /// How many recorded occurrences were left as they are: a placeholder
    /// inside a longer token, or a reply whose occurrences overlap.
    pub fn unresolved(&self) -> usize {
        self.unresolved
    }
}

impl std::fmt::Debug for HumanText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HumanText([private])")
    }
}

impl PrivacySession {
    /// The human view of the committed reply behind `source`, which is the
    /// source the agent keeps beside that reply. The recorded reply is not
    /// changed.
    pub fn human_reply(&self, source: &SourceRef) -> Result<HumanText, HumanRefusal> {
        self.inner.live().map_err(HumanRefusal::Stopped)?;
        let node = self
            .committed_model(source)
            .ok_or(HumanRefusal::NotAReply)?;
        let Msg::Assistant { content, .. } = node.message().map_err(HumanRefusal::Stopped)? else {
            return Err(HumanRefusal::NotAReply);
        };
        // The occurrences the reply's own commit recorded. `None` means two
        // of them overlap, so that no reading of the text is certain: every
        // token then stays as it is.
        let fields = node.fields().map_err(HumanRefusal::Stopped)?;
        let mut restored = Vec::new();
        let mut unresolved = 0;
        for field in fields.unwrap_or_default() {
            if *field.place() != FieldPlace::ReplyText {
                continue;
            }
            let range = field.range();
            // The live record of the commit that issued this placeholder
            // says whether it stands here as a whole token, and what is
            // behind it.
            let original = field
                .with_record(|mapping, records, index| {
                    Restorer::from_records(mapping, records)
                        .and_then(|restorer| restorer.occurrence(content, range.clone(), index))
                })
                .map_err(HumanRefusal::Stopped)?
                .map_err(HumanRefusal::Restore)?;
            match original {
                Some(original) => restored.push(Restored { range, original }),
                None => unresolved += 1,
            }
        }
        if fields.is_none() {
            unresolved = content.matches("pv1-").count();
        }
        let text = display_text(content, &restored).map_err(HumanRefusal::Restore)?;
        self.inner.live().map_err(HumanRefusal::Stopped)?;
        Ok(HumanText {
            owner: self.inner.clone(),
            text,
            restored: restored.len(),
            unresolved,
        })
    }
}

/// The most text of one reply that is held while it streams.
const MAX_HELD: usize = 256 * 1024;

/// How what was streamed compares with the reply that was committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Streamed {
    /// The provider streamed exactly the committed text.
    Whole,
    /// The provider streamed nothing through this hold.
    Nothing,
    /// What was streamed is not the committed text.
    Differs,
    /// More was streamed than the hold keeps.
    Overflowed,
}

/// The text of one reply while its provider streams it. Nothing of it is
/// shown: a placeholder can arrive in two pieces, and a person must see
/// neither a piece of it nor a value twice. The hold only counts and keeps
/// the masked text. When the reply is committed, the human view is made from
/// the committed reply, and the hold says how the stream compares with it.
/// When the turn is cancelled or fails first, the hold is dropped.
#[derive(Default)]
pub struct HeldStream {
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    text: String,
    overflowed: bool,
}

impl HeldStream {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The sink to give the conversation's provider in place of the
    /// terminal's.
    pub fn sink(self: &Arc<Self>) -> crate::agent::TokenSink {
        let hold = self.clone();
        Arc::new(move |delta: &str| hold.push(delta))
    }

    fn push(&self, delta: &str) {
        let mut held = self.held.lock().unwrap();
        if held.overflowed || held.text.len() + delta.len() > MAX_HELD {
            held.overflowed = true;
            held.text.clear();
        } else {
            held.text.push_str(delta);
        }
    }

    /// How many bytes are held, for a progress label.
    pub fn held_bytes(&self) -> usize {
        self.held.lock().unwrap().text.len()
    }

    /// The reply was not committed: the turn was cancelled or failed. What
    /// was held is dropped. Nothing of it was shown, and nothing is made up
    /// in its place.
    pub fn discard(&self) {
        *self.held.lock().unwrap() = Held::default();
    }

    /// The reply behind `source` was committed. Returns its human view,
    /// made from the committed reply and never from the stream, and how the
    /// stream compares with it. The hold is empty afterwards.
    pub fn committed(
        &self,
        session: &PrivacySession,
        source: &SourceRef,
    ) -> Result<(HumanText, Streamed), HumanRefusal> {
        let held = std::mem::take(&mut *self.held.lock().unwrap());
        let text = session.human_reply(source)?;
        let committed = session
            .committed_model(source)
            .ok_or(HumanRefusal::NotAReply)?;
        let streamed = match committed.message().map_err(HumanRefusal::Stopped)? {
            _ if held.overflowed => Streamed::Overflowed,
            _ if held.text.is_empty() => Streamed::Nothing,
            Msg::Assistant { content, .. } if *content == held.text => Streamed::Whole,
            _ => Streamed::Differs,
        };
        Ok((text, streamed))
    }
}

/// What a frontend is given for one committed reply of the conversation's
/// root agent.
pub enum HumanReply {
    /// The reply as a person may read it.
    Text(HumanText),
    /// No human view could be made of the reply. The conversation says why;
    /// nothing of the reply is given in its place.
    Withheld(HumanRefusal),
}

/// Where a frontend takes the replies of a privacy conversation.
pub type HumanReplySink = Arc<dyn Fn(HumanReply) + Send + Sync>;

/// A frontend's side of the replies of a privacy conversation's root agent.
///
/// The provider streams into its hold, where nothing is shown. When the
/// agent has retained a reply, the conversation's boundary tells this
/// display, and the frontend is given the human view of that committed
/// reply, once, as a typed text. A reply that is never committed gives the
/// frontend nothing: a round that begins drops whatever the last one left
/// in the hold.
///
/// The frontend draws the text through its own safe path. What it copies
/// out of the view is its own; this display writes it nowhere, and nothing
/// here is sent to a model.
pub struct ReplyDisplay {
    hold: Arc<HeldStream>,
    session: Arc<PrivacySession>,
    show: HumanReplySink,
    /// The one sink this display gives the root agent's provider.
    sink: crate::agent::TokenSink,
    /// The round sink this display made and the requested one it calls, by
    /// address; zero until it was made.
    round: [std::sync::atomic::AtomicUsize; 2],
}

/// What this display holds, read from the display: the frontend's reply
/// sink, the sink it gives the provider, the round sink it made and the
/// requested round sink that one calls.
impl super::Consumer for ReplyDisplay {
    fn held(&self) -> [usize; 5] {
        [
            &*self.show as *const _ as *const () as usize,
            &*self.sink as *const _ as *const () as usize,
            self.round[0].load(std::sync::atomic::Ordering::Acquire),
            self.round[1].load(std::sync::atomic::Ordering::Acquire),
            0,
        ]
    }
}

impl ReplyDisplay {
    pub fn new(session: Arc<PrivacySession>, show: HumanReplySink) -> Arc<Self> {
        let hold = HeldStream::new();
        Arc::new(Self {
            sink: hold.sink(),
            hold,
            session,
            show,
            round: Default::default(),
        })
    }

    /// The sink to give the root agent's provider in place of the
    /// frontend's.
    pub fn token_sink(&self) -> crate::agent::TokenSink {
        self.sink.clone()
    }

    /// The round sink to give the root agent's provider: this display's own
    /// notification, then the requested sink. The display keeps which two
    /// objects those are, so that it can say what it made and what it calls.
    pub(crate) fn round_sink(
        self: &Arc<Self>,
        requested: Option<crate::agent::RoundSink>,
    ) -> crate::agent::RoundSink {
        let display = self.clone();
        let inner = super::held_address!(&requested);
        let sink: crate::agent::RoundSink = Arc::new(move || {
            display.round_started();
            if let Some(requested) = &requested {
                requested();
            }
        });
        self.round[0].store(
            &*sink as *const _ as *const () as usize,
            std::sync::atomic::Ordering::Release,
        );
        self.round[1].store(inner, std::sync::atomic::Ordering::Release);
        sink
    }

    /// A model round begins. What an earlier round streamed and never
    /// committed is dropped.
    pub fn round_started(&self) {
        self.hold.discard();
    }

    /// The agent retained the reply behind `source`. Called by the
    /// conversation's boundary for the root agent, with the source it made
    /// for that commit.
    pub(super) fn committed(&self, source: &SourceRef) {
        match self.hold.committed(&self.session, source) {
            // A reply that is only calls has nothing to read.
            Ok((text, _)) if text.text.is_empty() => {}
            Ok((text, _)) => (self.show)(HumanReply::Text(text)),
            Err(refusal) => (self.show)(HumanReply::Withheld(refusal)),
        }
    }
}
