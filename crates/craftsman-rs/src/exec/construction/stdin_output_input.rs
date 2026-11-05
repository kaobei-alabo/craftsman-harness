//! Explicit native stdin + pre-run retained file target, never caller pairing.
use super::*;
use crate::exec::input::{StdinError, StdinSelection};
use crate::exec::output::{Destination, Error as OutputError, Selected as SelectedOutput, Target};

pub(in crate::exec) struct PreparedStdinOutputInput {
    input: PreparedInput,
    target: Target,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StdinOutputPreparationError {
    Stdin(StdinPreparationError),
    Output(OutputError),
}
impl std::fmt::Display for StdinOutputPreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for StdinOutputPreparationError {}
impl From<StdinError> for StdinOutputPreparationError {
    fn from(error: StdinError) -> Self {
        Self::Stdin(error.into())
    }
}
impl From<PreparationError> for StdinOutputPreparationError {
    fn from(error: PreparationError) -> Self {
        Self::Stdin(error.into())
    }
}
impl From<OutputError> for StdinOutputPreparationError {
    fn from(error: OutputError) -> Self {
        Self::Output(error)
    }
}
impl PreparedStdinOutputInput {
    pub(in crate::exec) async fn prepare(
        options: Options,
        sources: Sources<'_>,
        destination: Destination,
    ) -> std::result::Result<Self, StdinOutputPreparationError> {
        let (bootstrap, selection, selected, target) =
            Self::start(options, sources, destination).await?;
        // Only this point can inspect actual stdin: whole selection/config and
        // read-only retained destination admission already passed.
        let input = selected.load(&bootstrap.filesystem).await?;
        Self::finish(bootstrap, selection, input, target).await
    }
    async fn start(
        options: Options,
        sources: Sources<'_>,
        destination: Destination,
    ) -> std::result::Result<
        (Bootstrap, SessionSelection, StdinSelection, Target),
        StdinOutputPreparationError,
    > {
        let selected = StdinSelection::select(sources)?;
        let output = SelectedOutput::select(destination)?;
        let selection = options.session.clone();
        let bootstrap = tokio::task::spawn_blocking(move || Bootstrap::load(options))
            .await
            .map_err(|_| PreparationError::Worker)??;
        let (bootstrap, target) = tokio::task::spawn_blocking(move || {
            let target = output.capture(&bootstrap.source_roots)?;
            Ok::<_, OutputError>((bootstrap, target))
        })
        .await
        .map_err(|_| OutputError::Worker)??;
        Ok((bootstrap, selection, selected, target))
    }
    async fn finish(
        bootstrap: Bootstrap,
        selection: SessionSelection,
        input: Admitted,
        target: Target,
    ) -> std::result::Result<Self, StdinOutputPreparationError> {
        // Native capture closed/restored its handle; whole UTF-8/attachments
        // are admitted. Detect changes during that wait BEFORE IDs/store.
        let (bootstrap, target) = tokio::task::spawn_blocking(move || {
            target.revalidate_before_session()?;
            Ok::<_, OutputError>((bootstrap, target))
        })
        .await
        .map_err(|_| OutputError::Worker)??;
        let prepared = Prepared::from_bootstrap(bootstrap, selection).await?;
        Ok(Self {
            input: PreparedInput { prepared, input },
            target,
        })
    }
    pub(in crate::exec) fn into_parts(self) -> (PreparedInput, Target) {
        (self.input, self.target)
    }
    #[cfg(test)]
    pub(in crate::exec) async fn prepare_for_tests(
        options: Options,
        sources: Sources<'_>,
        destination: Destination,
        pipe: crate::platform_os::stdin_pipe::Pipe,
        limits: crate::exec::input::stdin::Limits,
        after_capture: impl FnOnce(),
    ) -> std::result::Result<Self, StdinOutputPreparationError> {
        let (bootstrap, selection, selected, target) =
            Self::start(options, sources, destination).await?;
        // Actual retained-target readiness signal only, not supplied data/grants.
        after_capture();
        let input = selected
            .load_for_tests(&bootstrap.filesystem, pipe, limits)
            .await?;
        Self::finish(bootstrap, selection, input, target).await
    }
}
