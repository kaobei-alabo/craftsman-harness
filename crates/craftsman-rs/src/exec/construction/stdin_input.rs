//! New explicit native-input route; old config/input/file-output APIs stay exact.
use super::*;
use crate::exec::input::{StdinError, StdinSelection};

pub(in crate::exec) struct PreparedStdinInput {
    input: PreparedInput,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StdinPreparationError {
    Input(StdinError),
    Preparation(PreparationError),
    Runtime,
}
impl std::fmt::Display for StdinPreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for StdinPreparationError {}
impl From<StdinError> for StdinPreparationError {
    fn from(error: StdinError) -> Self {
        Self::Input(error)
    }
}
impl From<PreparationError> for StdinPreparationError {
    fn from(error: PreparationError) -> Self {
        Self::Preparation(error)
    }
}
impl PreparedStdinInput {
    pub(in crate::exec) async fn prepare(
        options: Options,
        sources: Sources<'_>,
    ) -> std::result::Result<Self, StdinPreparationError> {
        // All source/count/path grammar precedes even inspection of stdin.
        let selected = StdinSelection::select(sources)?;
        let selection = options.session.clone();
        let bootstrap = tokio::task::spawn_blocking(move || Bootstrap::load(options))
            .await
            .map_err(|_| PreparationError::Worker)??;
        let input = selected.load(&bootstrap.filesystem).await?;
        // Whole native UTF-8/attachment/frame admission and mode restoration
        // precede IDs/store/provider construction through SAME Bootstrap.
        let prepared = Prepared::from_bootstrap(bootstrap, selection).await?;
        Ok(Self {
            input: PreparedInput { prepared, input },
        })
    }
    pub(in crate::exec) fn into_parts(self) -> (Prepared, Admitted) {
        self.input.into_parts()
    }
    #[cfg(test)]
    pub(in crate::exec) async fn prepare_for_tests(
        options: Options,
        sources: Sources<'_>,
        pipe: crate::platform_os::stdin_pipe::Pipe,
        limits: crate::exec::input::stdin::Limits,
    ) -> std::result::Result<Self, StdinPreparationError> {
        let selected = StdinSelection::select(sources)?;
        let selection = options.session.clone();
        let bootstrap = tokio::task::spawn_blocking(move || Bootstrap::load(options))
            .await
            .map_err(|_| PreparationError::Worker)??;
        let input = selected
            .load_for_tests(&bootstrap.filesystem, pipe, limits)
            .await?;
        let prepared = Prepared::from_bootstrap(bootstrap, selection).await?;
        Ok(Self {
            input: PreparedInput { prepared, input },
        })
    }
}
