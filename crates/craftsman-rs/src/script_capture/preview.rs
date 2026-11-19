//! Pure prospective CRUD from host originals and strict final candidates.
//! No native lookup, IO, approval or apply exists in this module.

use super::{
    CaptureError, MAX_FILE_BYTES, MAX_PATH_BYTES, MAX_PREVIEW_BYTES, MAX_TARGETS, MAX_TOTAL_BYTES,
    Result, SnapshotData, hex,
};
use crate::agent::Session;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde::de::{self, IgnoredAny, SeqAccess, Visitor};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::Write;
use std::sync::Arc;

/// Historical operation facts, never a permit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Create,
    Update,
    Delete,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateEnvelope {
    changes: CandidateList,
}
struct CandidateList(Vec<CandidateWire>);

impl<'de> Deserialize<'de> for CandidateList {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Candidates;
        impl<'de> Visitor<'de> for Candidates {
            type Value = CandidateList;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded complete final-candidate array")
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut candidates = Vec::new();
                loop {
                    if candidates.len() == MAX_TARGETS {
                        // Never allocate another owned candidate to discover
                        // count overflow. Input itself already has a byte bound.
                        if seq.next_element::<IgnoredAny>()?.is_some() {
                            return Err(de::Error::custom(
                                "complete candidate count exceeds its bound",
                            ));
                        }
                        break;
                    }
                    let Some(candidate) = seq.next_element::<CandidateWire>()? else {
                        break;
                    };
                    candidates.push(candidate);
                }
                Ok(CandidateList(candidates))
            }
        }
        deserializer.deserialize_seq(Candidates)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateWire {
    target: String,
    after: StateWire,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum StateWire {
    File { bytes_base64: String },
    Absent {},
}

enum FinalState {
    File(Vec<u8>),
    Absent,
}
struct Change {
    index: usize,
    operation: Operation,
    after: FinalState,
}

/// Complete immutable app data. Eligibility checks bind it to the original
/// owner/generation; an obtained historical view can never authorize effects.
pub struct ProspectivePreview {
    snapshots: Arc<SnapshotData>,
    changes: Vec<Change>,
    encoded: Vec<u8>,
    digest: [u8; 32],
}

impl ProspectivePreview {
    pub fn operations(&self, actual: &Session) -> Result<Vec<(String, Operation)>> {
        self.snapshots.check(actual)?;
        Ok(self
            .changes
            .iter()
            .map(|change| {
                (
                    self.snapshots.records[change.index].name().to_owned(),
                    change.operation,
                )
            })
            .collect())
    }

    /// Full exact binary/empty/operation/metadata/binding frame, with no clipping.
    pub fn complete_historical_preview<'a>(&'a self, actual: &Session) -> Result<&'a [u8]> {
        self.snapshots.check(actual)?;
        Ok(&self.encoded)
    }

    pub fn plan_digest(&self, actual: &Session) -> Result<[u8; 32]> {
        self.snapshots.check(actual)?;
        Ok(self.digest)
    }
}

/// A memory writer only. Refuse the whole preview on the first encoded overflow.
struct CompleteFrame(Vec<u8>);
impl Write for CompleteFrame {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_PREVIEW_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("complete frame exceeds its bound"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn derive(snapshots: Arc<SnapshotData>, raw: &[u8]) -> Result<ProspectivePreview> {
    if raw.len() > MAX_PREVIEW_BYTES {
        return Err(CaptureError::Bounds);
    }
    // serde rejects duplicate object fields/tags, extra metadata/claims and
    // unsupported kinds. Canonical base64 is the sole binary representation.
    let wire: CandidateEnvelope =
        serde_json::from_slice(raw).map_err(|_| CaptureError::Candidate)?;
    let mut keys = std::collections::BTreeSet::new();
    let mut path_bytes = 0usize;
    let mut byte_count = 0usize;
    let mut mapped = Vec::with_capacity(wire.changes.0.len());
    for candidate in &wire.changes.0 {
        super::native::checked_name(&candidate.target).map_err(|_| CaptureError::Candidate)?;
        if !keys.insert(super::native::denial_key(&candidate.target)) {
            return Err(CaptureError::Candidate);
        }
        path_bytes = path_bytes
            .checked_add(candidate.target.len())
            .ok_or(CaptureError::Bounds)?;
        if path_bytes > MAX_PATH_BYTES {
            return Err(CaptureError::Bounds);
        }
        let (index, _) = snapshots.record(&candidate.target)?;
        if let StateWire::File { bytes_base64 } = &candidate.after {
            // Exact decoded size is admitted for the WHOLE plan before decoding
            // any candidate bytes. Padding/alphabet/trailing bits are checked
            // next; malformed inputs never become an alternate representation.
            let size = decoded_size(bytes_base64)?;
            if size > MAX_FILE_BYTES {
                return Err(CaptureError::Bounds);
            }
            byte_count = byte_count.checked_add(size).ok_or(CaptureError::Bounds)?;
            if byte_count > MAX_TOTAL_BYTES {
                return Err(CaptureError::Bounds);
            }
        }
        mapped.push(index);
    }
    let mut changes = Vec::new();
    for (candidate, index) in wire.changes.0.into_iter().zip(mapped) {
        let before = snapshots.records[index].bytes();
        let after = match candidate.after {
            StateWire::Absent {} => FinalState::Absent,
            StateWire::File { bytes_base64 } => {
                let bytes = STANDARD
                    .decode(&bytes_base64)
                    .map_err(|_| CaptureError::Candidate)?;
                if STANDARD.encode(&bytes) != bytes_base64 {
                    return Err(CaptureError::Candidate);
                }
                FinalState::File(bytes)
            }
        };
        let operation = match (before, &after) {
            (None, FinalState::File(_)) => Some(Operation::Create),
            (Some(old), FinalState::File(new)) if old != new => Some(Operation::Update),
            (Some(_), FinalState::Absent) => Some(Operation::Delete),
            _ => None, // restoration, transient create/delete or exact no-op.
        };
        if let Some(operation) = operation {
            changes.push(Change {
                index,
                operation,
                after,
            });
        }
    }
    // Only explicit candidates are considered. Omission never deletes.
    let operations: Vec<Value> = changes
        .iter()
        .map(|change| {
            let record = &snapshots.records[change.index];
            let after = match &change.after {
                FinalState::Absent => json!({"kind":"absent"}),
                FinalState::File(bytes) => json!({
                    "kind":"file","size":bytes.len(),"sha256":hex(&Sha256::digest(bytes)),
                    "bytes_base64":STANDARD.encode(bytes),
                }),
            };
            json!({
                "operation":change.operation.name(),
                "target":record.name(),
                "original":record.binding(),
                "after":after,
            })
        })
        .collect();
    let mut frame = json!({
        "version":1,
        "status":"historical-prospective",
        "session":hex(&snapshots.context.nonce),
        "generation":snapshots.generation,
        "capture":snapshots.serial,
        "capture_sha256":hex(&snapshots.digest),
        "scope":snapshots.context.root.binding(),
        "operations":operations,
    });
    let mut complete = CompleteFrame(Vec::new());
    serde_json::to_writer(&mut complete, &frame).map_err(|_| CaptureError::Bounds)?;
    let digest: [u8; 32] = Sha256::digest(&complete.0).into();
    frame["plan_sha256"] = json!(hex(&digest));
    complete.0.clear();
    serde_json::to_writer(&mut complete, &frame).map_err(|_| CaptureError::Bounds)?;
    Ok(ProspectivePreview {
        snapshots,
        changes,
        encoded: complete.0,
        digest,
    })
}

fn decoded_size(encoded: &str) -> Result<usize> {
    if encoded.is_empty() {
        return Ok(0);
    }
    if encoded.len() % 4 != 0 {
        return Err(CaptureError::Candidate);
    }
    let padding = encoded
        .as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'=')
        .count();
    if padding > 2 {
        return Err(CaptureError::Candidate);
    }
    encoded
        .len()
        .checked_div(4)
        .and_then(|groups| groups.checked_mul(3))
        .and_then(|bytes| bytes.checked_sub(padding))
        .ok_or(CaptureError::Bounds)
}
