//! Opaque pre-run target + actual invocation chain. Never caller Run/target pairing.
use super::{Sources, invocation::Invocation};
use crate::exec::construction::{Options, OutputPreparationError, PreparedOutputInput};
use crate::exec::output::{Destination, Error, Target};
use crate::exec::runtime::Runtime;
use crate::exec::terminal::Terminal;

pub(crate) struct FileInvocation {
    invocation: Invocation,
    target: Target,
}
pub(crate) struct FileRun {
    run: super::Run,
    target: Target,
}
impl FileInvocation {
    pub(crate) async fn prepare(
        options: Options,
        sources: Sources<'_>,
        destination: Destination,
    ) -> Result<Self, OutputPreparationError> {
        let prepared = PreparedOutputInput::prepare(options, sources, destination).await?;
        let (prepared, target) = prepared.into_parts();
        let (prepared, input) = prepared.into_parts();
        let runtime = Runtime::build(prepared)
            .await
            .map_err(|_| super::super::construction::InputPreparationError::Runtime)?;
        // SAME existing Runtime/shared builder; no legacy Config/default adapter.
        let invocation = Invocation::from_runtime_input(runtime, input);
        Ok(Self { invocation, target })
    }
    pub(crate) async fn run(self) -> FileRun {
        FileRun {
            run: self.invocation.run().await,
            target: self.target,
        }
    }
}
impl FileRun {
    pub(crate) fn terminal(self) -> FileTerminal {
        FileTerminal {
            terminal: self.run.terminal(),
            target: self.target,
            attempted: false,
        }
    }
    #[cfg(test)]
    pub(in crate::exec) fn run_for_tests(&self) -> &super::Run {
        &self.run
    }
}

/// Only this module's consumed actual FileRun constructs the paired controller.
/// No raw Run/target/config/gates/seeds/bytes/stdio or Runtime/resume interface.
pub(crate) struct FileTerminal {
    terminal: Terminal,
    target: Target,
    attempted: bool,
}
impl FileTerminal {
    pub(crate) fn close(&self) {
        self.terminal.close();
    }
    pub(crate) fn publish(&mut self) -> Result<(), Failure> {
        self.attempt(|target, terminal, record, linked| {
            crate::exec::output::publish_target(target, terminal, record, linked)
        })
    }
    fn attempt(
        &mut self,
        publish: impl FnOnce(
            &Target,
            &Terminal,
            &crate::exec::terminal::Record<'_>,
            &mut bool,
        ) -> Result<(), Error>,
    ) -> Result<(), Failure> {
        if self.attempted {
            return Err(Failure {
                kind: Error::AlreadyAttempted,
                link_returned_success: false,
            });
        }
        self.attempted = true;
        let mut linked = false;
        let result = (|| {
            let (format, maximum) = self.target.parameters();
            let record = self.terminal.file_record(format, maximum)?;
            let published = publish(&self.target, &self.terminal, &record, &mut linked);
            // Recheck the SAME actual lease/epoch after every native result,
            // including early failures and owned staging cleanup via Drop.
            record.ready()?;
            published
        })();
        match result {
            Ok(()) => Ok(()),
            Err(kind) => {
                self.terminal.close();
                Err(Failure {
                    kind,
                    link_returned_success: linked,
                })
            }
        }
    }
    #[cfg(test)]
    pub(in crate::exec) fn publish_for_tests(
        &mut self,
        signals: &mut crate::exec::output::Signals<'_>,
    ) -> Result<(), Failure> {
        self.attempt(|target, terminal, record, linked| {
            crate::exec::output::publish_target_for_tests(target, terminal, record, linked, signals)
        })
    }
}

/// Only an actual attempt supplies this observation. No byte/request/usage/run
/// receipt, public outcome, caller status, raw path/body/OS string or Serialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Failure {
    kind: Error,
    link_returned_success: bool,
}
impl Failure {
    pub(crate) fn kind(self) -> Error {
        self.kind
    }
    /// ONLY our native link call returned success, not current file existence,
    /// complete result delivery, verification, durability or public run success.
    pub(crate) fn link_returned_success(self) -> bool {
        self.link_returned_success
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for Failure {}

// Child route can construct only from its opaque pre-run native/file preparation.
pub(super) mod stdin;
