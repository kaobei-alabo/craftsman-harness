//! Fixed, cfg-test-only observations of one actual opening operation.
use super::super::PrivacyError;
use serde::Serialize;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const PREFIX: &[u8] = b"VAULT-OPEN-FACTS:";
const MAX_RECORD_BYTES: usize = 2048;
const AWAITING: u8 = 0;
const DROPPED: u8 = 1;
const RETURNED: u8 = 2;

#[derive(Clone, Copy)]
#[repr(u8)]
pub(super) enum Phase {
    NativeCapture = 1,
    KeyDerivation,
    DatabaseBuild,
    ConnectionSetup,
    InitialLayout,
    WalSetup,
    Pragmas,
    FreshTransaction,
    Commit,
    PostValidation,
    NativeReturn,
}

fn phase(value: u8) -> Option<&'static str> {
    Some(match value {
        1 => "native_capture",
        2 => "key_derivation",
        3 => "database_build",
        4 => "connection_setup",
        5 => "initial_layout",
        6 => "wal_setup",
        7 => "pragmas",
        8 => "fresh_transaction",
        9 => "commit",
        10 => "post_validation",
        11 => "native_return",
        _ => return None,
    })
}

fn edge(value: u8) -> Option<&'static str> {
    match value {
        1 => Some("entered"),
        2 => Some("returned"),
        _ => None,
    }
}

#[derive(Clone, Copy, Serialize)]
struct Sample {
    seconds: u64,
    nanos: u32,
}

impl From<Duration> for Sample {
    fn from(value: Duration) -> Self {
        Self {
            seconds: value.as_secs(),
            nanos: value.subsec_nanos(),
        }
    }
}

#[derive(Default)]
struct Stamp {
    present: AtomicBool,
    seconds: AtomicU64,
    nanos: AtomicU32,
}

impl Stamp {
    fn store(&self, value: Option<Sample>) {
        self.present.store(false, Ordering::Relaxed);
        if let Some(value) = value {
            self.seconds.store(value.seconds, Ordering::Relaxed);
            self.nanos.store(value.nanos, Ordering::Relaxed);
            self.present.store(true, Ordering::Release);
        }
    }
    fn read(&self) -> Option<Sample> {
        self.present.load(Ordering::Acquire).then(|| Sample {
            seconds: self.seconds.load(Ordering::Relaxed),
            nanos: self.nanos.load(Ordering::Relaxed),
        })
    }
}

pub(super) struct Opening {
    started: Instant,
    open_key: Sample,
    pre_dispatch: Stamp,
    worker_entry: Stamp,
    current_phase: AtomicU8,
    current_edge: AtomicU8,
    entered: Stamp,
    phase_return: Stamp,
    previous_phase: AtomicU8,
    previous_return: Stamp,
    deadline_seen: AtomicBool,
    refused_phase: AtomicU8,
    refused_edge: AtomicU8,
    refused_entered: Stamp,
    refused_return: Stamp,
    refused_previous_phase: AtomicU8,
    refused_previous_return: Stamp,
    refused_elapsed: Stamp,
    calls: AtomicUsize,
    work: AtomicUsize,
    waiter: AtomicU8,
}

pub(super) struct Waiter(Arc<Opening>);
impl Drop for Waiter {
    fn drop(&mut self) {
        // Observation only: no wait, close, rollback or production flag change.
        let _ =
            self.0
                .waiter
                .compare_exchange(AWAITING, DROPPED, Ordering::AcqRel, Ordering::Acquire);
    }
}

#[derive(Serialize)]
struct Deadline {
    elapsed: Option<Sample>,
    admitted_calls: usize,
    work_bytes: usize,
}

#[derive(Serialize)]
struct Record {
    version: u8,
    error: &'static str,
    open_key: Sample,
    pre_dispatch: Option<Sample>,
    worker_entry: Option<Sample>,
    phase: Option<&'static str>,
    phase_edge: Option<&'static str>,
    phase_entered: Option<Sample>,
    phase_return_observed: Option<Sample>,
    previous_phase: Option<&'static str>,
    previous_return_observed: Option<Sample>,
    deadline: Option<Deadline>,
    worker_result_returned: bool,
    waiter_drop_observed: bool,
}

impl Opening {
    pub(super) fn new(started: Instant) -> (Arc<Self>, Waiter) {
        let opening = Arc::new(Self {
            started,
            open_key: started.elapsed().into(),
            pre_dispatch: Stamp::default(),
            worker_entry: Stamp::default(),
            current_phase: AtomicU8::new(0),
            current_edge: AtomicU8::new(0),
            entered: Stamp::default(),
            phase_return: Stamp::default(),
            previous_phase: AtomicU8::new(0),
            previous_return: Stamp::default(),
            deadline_seen: AtomicBool::new(false),
            refused_phase: AtomicU8::new(0),
            refused_edge: AtomicU8::new(0),
            refused_entered: Stamp::default(),
            refused_return: Stamp::default(),
            refused_previous_phase: AtomicU8::new(0),
            refused_previous_return: Stamp::default(),
            refused_elapsed: Stamp::default(),
            calls: AtomicUsize::new(0),
            work: AtomicUsize::new(0),
            waiter: AtomicU8::new(AWAITING),
        });
        let waiter = Waiter(opening.clone());
        (opening, waiter)
    }
    pub(super) fn pre_dispatch(&self) {
        self.pre_dispatch.store(Some(self.started.elapsed().into()));
    }
    pub(super) fn worker_entry(&self) {
        self.worker_entry.store(Some(self.started.elapsed().into()));
    }
    pub(super) fn enter(&self, value: Phase) {
        // Phase data has one writer, the actual owned worker. Only the waiter
        // drop observation can race with the final returned-result record.
        self.previous_phase.store(
            self.current_phase.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.previous_return.store(self.phase_return.read());
        self.current_phase.store(value as u8, Ordering::Relaxed);
        self.current_edge.store(1, Ordering::Relaxed);
        self.entered.store(Some(self.started.elapsed().into()));
        self.phase_return.store(None);
    }
    pub(super) fn returned(&self) {
        self.phase_return.store(Some(self.started.elapsed().into()));
        self.current_edge.store(2, Ordering::Relaxed);
    }
    pub(super) fn deadline(&self, elapsed: Duration, calls: usize, work: usize) {
        if self.deadline_seen.load(Ordering::Relaxed) {
            return;
        }
        self.refused_phase.store(
            self.current_phase.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.refused_edge
            .store(self.current_edge.load(Ordering::Relaxed), Ordering::Relaxed);
        self.refused_entered.store(self.entered.read());
        self.refused_return.store(self.phase_return.read());
        self.refused_previous_phase.store(
            self.previous_phase.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.refused_previous_return
            .store(self.previous_return.read());
        self.refused_elapsed.store(Some(elapsed.into()));
        self.calls.store(calls, Ordering::Relaxed);
        self.work.store(work, Ordering::Relaxed);
        self.deadline_seen.store(true, Ordering::Release);
    }
    pub(super) fn finish(&self, error: Option<PrivacyError>) {
        // After the original worker future and its existing return hook return.
        // A dropped/pending worker never emits from Drop.
        let prior = self.waiter.swap(RETURNED, Ordering::AcqRel);
        if prior == RETURNED {
            return;
        }
        let Some(error) = error else {
            return;
        };
        let error = match error {
            PrivacyError::Closed => "closed",
            PrivacyError::Key => "key",
            PrivacyError::Integrity => "integrity",
            PrivacyError::Bounds => "bounds",
            PrivacyError::Input => "input",
            PrivacyError::Unsupported => "unsupported",
            PrivacyError::UnknownAlias => "unknown_alias",
            PrivacyError::ForeignScope => "foreign_scope",
            PrivacyError::Collision => "collision",
            PrivacyError::Busy => "busy",
            PrivacyError::Io => "io",
            PrivacyError::Worker => "worker",
            PrivacyError::Deadline => "deadline",
        };
        let refused = self.deadline_seen.load(Ordering::Acquire);
        let record = Record {
            version: 1,
            error,
            open_key: self.open_key,
            pre_dispatch: self.pre_dispatch.read(),
            worker_entry: self.worker_entry.read(),
            phase: phase(if refused {
                self.refused_phase.load(Ordering::Relaxed)
            } else {
                self.current_phase.load(Ordering::Relaxed)
            }),
            phase_edge: edge(if refused {
                self.refused_edge.load(Ordering::Relaxed)
            } else {
                self.current_edge.load(Ordering::Relaxed)
            }),
            phase_entered: if refused {
                self.refused_entered.read()
            } else {
                self.entered.read()
            },
            phase_return_observed: if refused {
                self.refused_return.read()
            } else {
                self.phase_return.read()
            },
            previous_phase: phase(if refused {
                self.refused_previous_phase.load(Ordering::Relaxed)
            } else {
                self.previous_phase.load(Ordering::Relaxed)
            }),
            previous_return_observed: if refused {
                self.refused_previous_return.read()
            } else {
                self.previous_return.read()
            },
            deadline: refused.then(|| Deadline {
                elapsed: self.refused_elapsed.read(),
                admitted_calls: self.calls.load(Ordering::Relaxed),
                work_bytes: self.work.load(Ordering::Relaxed),
            }),
            worker_result_returned: true,
            waiter_drop_observed: prior == DROPPED,
        };
        let Ok(encoded) = serde_json::to_vec(&record) else {
            return;
        };
        if PREFIX.len() + encoded.len() + 1 > MAX_RECORD_BYTES {
            return;
        }
        let mut packet = Vec::with_capacity(PREFIX.len() + encoded.len() + 1);
        packet.extend_from_slice(PREFIX);
        packet.extend_from_slice(&encoded);
        packet.push(b'\n');
        // Best effort only after actual result return; no spill, raw fallback,
        // journal, retry or formatting/panic payload changes the original error.
        let _ = std::io::stdout().lock().write_all(&packet);
    }
}
