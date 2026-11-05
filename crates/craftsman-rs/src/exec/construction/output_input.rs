//! New explicit file-input route; the old configuration/input routes stay exact.
use super::*;
use crate::exec::output::{Destination, Error as OutputError, Selected as SelectedOutput, Target};

pub(in crate::exec) struct PreparedOutputInput {
    input: PreparedInput,
    target: Target,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputPreparationError {
    Input(InputPreparationError),
    Output(OutputError),
}
impl std::fmt::Display for OutputPreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::error::Error for OutputPreparationError {}
impl From<InputPreparationError> for OutputPreparationError {
    fn from(e: InputPreparationError) -> Self {
        Self::Input(e)
    }
}
impl From<PreparationError> for OutputPreparationError {
    fn from(e: PreparationError) -> Self {
        Self::Input(e.into())
    }
}
impl From<InputError> for OutputPreparationError {
    fn from(e: InputError) -> Self {
        Self::Input(e.into())
    }
}
impl From<OutputError> for OutputPreparationError {
    fn from(e: OutputError) -> Self {
        Self::Output(e)
    }
}
impl PreparedOutputInput {
    pub(in crate::exec) async fn prepare(
        options: Options,
        sources: Sources<'_>,
        destination: Destination,
    ) -> std::result::Result<Self, OutputPreparationError> {
        let selected_input = super::super::input::Selected::select(sources)?;
        let selected_output = SelectedOutput::select(destination)?;
        let selection = options.session.clone();
        let bootstrap = tokio::task::spawn_blocking(move || Bootstrap::load(options))
            .await
            .map_err(|_| PreparationError::Worker)??;
        let input = selected_input.load(&bootstrap.filesystem).await?;
        // BOTH complete input and read-only retained output capture precede
        // session selection/IDs/store open/shared-builder/provider construction.
        let (bootstrap, target) = tokio::task::spawn_blocking(move || {
            let target = selected_output.capture(&bootstrap.source_roots)?;
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
}
