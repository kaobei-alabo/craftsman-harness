//! Explicit-only native byte-pipe instruction capture. Old input routes stay exact.
use super::{Admitted, InputError, OriginKind, Sources, admit_path, decode, nonempty};
use crate::exec::construction::{Options, Prepared, PreparedStdinInput, StdinPreparationError};
use crate::exec::runtime::Runtime;
use crate::platform_os::stdin_pipe::{self, Pipe, Read};
use std::time::Duration;
use tokio::time::Instant;

#[cfg(test)]
mod tests;

const FIRST: Duration = Duration::from_secs(10);
const TOTAL: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(10);
const CALLS: usize = 4096;
#[derive(Clone, Copy)]
pub(in crate::exec) struct Limits {
    maximum: usize,
    first: Duration,
    total: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            maximum: super::INSTRUCTION_BYTES,
            first: FIRST,
            total: TOTAL,
        }
    }
}
impl Limits {
    fn valid(self) -> bool {
        self.maximum > 0
            && self.maximum <= super::INSTRUCTION_BYTES
            && !self.first.is_zero()
            && self.first <= FIRST
            && self.total >= self.first
            && self.total <= TOTAL
    }
    #[cfg(test)]
    pub(in crate::exec) fn tightening(maximum: usize, first: Duration, total: Duration) -> Self {
        Self {
            maximum,
            first,
            total,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    Input(InputError),
    NotSelected,
    Native(stdin_pipe::Error),
    FirstByteDeadline,
    TotalDeadline,
    ReadCalls,
    Resource,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for Error {}
impl From<InputError> for Error {
    fn from(error: InputError) -> Self {
        Self::Input(error)
    }
}
/// An opaque capability minted ONLY after the exact positional '-' and ALL
/// attachment selection grammar pass. It never stands for caller-supplied bytes.
pub(crate) struct Selected {
    files: Vec<String>,
}
impl Selected {
    pub(in crate::exec) fn select(sources: Sources<'_>) -> Result<Self, Error> {
        match (sources.prompt, sources.prompt_file) {
            (None, None) => return Err(InputError::MissingSource.into()),
            (Some(_), Some(_)) => return Err(InputError::ConflictingSources.into()),
            (Some(b"-"), None) => {}
            _ => return Err(Error::NotSelected),
        }
        if sources.files.len() > super::FILES {
            return Err(InputError::SelectionLimit.into());
        }
        for file in sources.files {
            admit_path(file)?;
        }
        Ok(Self {
            files: sources.files.iter().map(|s| (*s).to_owned()).collect(),
        })
    }
    pub(in crate::exec) async fn load(
        self,
        filesystem: &super::ConfinedFs,
    ) -> Result<Admitted, Error> {
        let started = Instant::now();
        let pipe = Pipe::stdin(&self).map_err(Error::Native)?;
        let bytes = capture(pipe, started, Limits::default()).await?;
        self.admit(filesystem, bytes).await
    }
    #[cfg(test)]
    pub(in crate::exec) async fn load_for_tests(
        self,
        filesystem: &super::ConfinedFs,
        pipe: Pipe,
        limits: Limits,
    ) -> Result<Admitted, Error> {
        let bytes = capture(pipe, Instant::now(), limits).await?;
        self.admit(filesystem, bytes).await
    }
    async fn admit(
        self,
        filesystem: &super::ConfinedFs,
        bytes: Vec<u8>,
    ) -> Result<Admitted, Error> {
        let observed = bytes.len();
        // A single encoding preamble is counted in the ORIGINAL stream budget.
        // Strict whole UTF-8 follows genuine EOF; never lossily decode a prefix.
        let (text, removed) = decode(bytes, true)?;
        nonempty(&text)?;
        // Reuse the SAME old attachment/file/framing admission body and limits.
        // Native capture, never caller text, supplies this selected instruction.
        let mut input = super::Selected {
            instruction: super::Instruction::Literal(text),
            files: self.files,
        }
        .load(filesystem)
        .await?;
        input.facts.instruction.kind = OriginKind::Stdin;
        input.facts.instruction.bytes = observed;
        input.facts.instruction.bom_removed = removed;
        Ok(input)
    }
}
async fn capture(mut pipe: Pipe, started: Instant, limits: Limits) -> Result<Vec<u8>, Error> {
    let result = read_all(&mut pipe, started, limits).await;
    // Restore/close the selected native mode BEFORE any admitted source is
    // returned. A dropped future owns no pending/background read; Pipe Drop
    // restores its own mode best-effort without clobbering foreign mutations.
    pipe.finish().map_err(Error::Native)?;
    result
}
fn deadline(started: Instant, limits: Limits, seen: bool) -> Result<Instant, Error> {
    let now = Instant::now();
    let total = started + limits.total;
    let first = started + limits.first;
    // Report the earliest expired boundary, even if a delayed scheduler poll
    // observes both clocks expired. An idle stream never had a first byte.
    if !seen && now >= first {
        return Err(Error::FirstByteDeadline);
    }
    if now >= total {
        return Err(Error::TotalDeadline);
    }
    Ok(if seen { total } else { first.min(total) })
}
async fn read_all(pipe: &mut Pipe, started: Instant, limits: Limits) -> Result<Vec<u8>, Error> {
    if !limits.valid() {
        return Err(Error::Resource);
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; stdin_pipe::STEP];
    let mut seen = false;
    for _ in 0..CALLS {
        deadline(started, limits, seen)?;
        // At the exact source ceiling, only one stack-byte EOF/overflow probe
        // remains; never grow a retained prefix beyond the accepted maximum.
        let length = stdin_pipe::STEP.min(limits.maximum - bytes.len() + 1);
        let result = pipe.read(&mut buffer[..length]);
        let wake = deadline(started, limits, seen)?;
        match result.map_err(Error::Native)? {
            Read::End => return Ok(bytes),
            Read::Pending => tokio::time::sleep_until(wake.min(Instant::now() + POLL)).await,
            Read::Bytes(n) => {
                if n == 0 || n > length {
                    return Err(Error::Resource);
                }
                if n > limits.maximum - bytes.len() {
                    return Err(InputError::InstructionLimit.into());
                }
                bytes.try_reserve(n).map_err(|_| Error::Resource)?;
                bytes.extend_from_slice(&buffer[..n]);
                seen = true;
            }
        }
    }
    Err(Error::ReadCalls)
}
/// The actual native-input/prepared/runtime chain, never caller raw input or
/// unrelated Prepared/Run/gates/seeds. It returns the unchanged actual input Run.
pub(crate) struct Invocation {
    invocation: super::Invocation,
}
impl Invocation {
    pub(crate) async fn prepare(
        options: Options,
        sources: Sources<'_>,
    ) -> Result<Self, StdinPreparationError> {
        let prepared = PreparedStdinInput::prepare(options, sources).await?;
        Self::build(prepared).await
    }
    async fn build(prepared: PreparedStdinInput) -> Result<Self, StdinPreparationError> {
        let (prepared, input) = prepared.into_parts();
        let runtime = Runtime::build(prepared)
            .await
            .map_err(|_| StdinPreparationError::Runtime)?;
        Ok(Self {
            invocation: super::invocation::Invocation::from_runtime_input(runtime, input),
        })
    }
    pub(crate) async fn prepare_with_request_limit(
        options: Options,
        sources: Sources<'_>,
        requests: crate::exec::meter::RequestLimit,
    ) -> Result<Self, StdinPreparationError> {
        let prepared = PreparedStdinInput::prepare(options, sources).await?;
        let (prepared, input) = prepared.into_parts();
        let runtime = Runtime::build_with_request_limit(prepared, requests)
            .await
            .map_err(|_| StdinPreparationError::Runtime)?;
        Ok(Self {
            invocation: super::invocation::Invocation::from_runtime_input(runtime, input),
        })
    }
    pub(crate) fn prepared(&self) -> &Prepared {
        self.invocation.prepared()
    }
    pub(crate) async fn run(self) -> super::Run {
        self.invocation.run().await
    }
    #[cfg(test)]
    pub(in crate::exec) async fn prepare_for_tests(
        options: Options,
        sources: Sources<'_>,
        pipe: Pipe,
        limits: Limits,
    ) -> Result<Self, StdinPreparationError> {
        Self::build(PreparedStdinInput::prepare_for_tests(options, sources, pipe, limits).await?)
            .await
    }
}
