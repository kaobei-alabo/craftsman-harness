//! New exact-'-' invocation composed with the unchanged paired file controller.
use crate::exec::construction::{
    Options, PreparedStdinOutputInput, StdinOutputPreparationError, StdinPreparationError,
};
use crate::exec::input::Sources;
use crate::exec::output::Destination;
use crate::exec::runtime::Runtime;

pub(crate) struct Invocation {
    invocation: super::FileInvocation,
}
impl Invocation {
    pub(crate) async fn prepare(
        options: Options,
        sources: Sources<'_>,
        destination: Destination,
    ) -> Result<Self, StdinOutputPreparationError> {
        Self::build(PreparedStdinOutputInput::prepare(options, sources, destination).await?).await
    }
    async fn build(
        prepared: PreparedStdinOutputInput,
    ) -> Result<Self, StdinOutputPreparationError> {
        let (prepared, target) = prepared.into_parts();
        let (prepared, input) = prepared.into_parts();
        let runtime = Runtime::build(prepared)
            .await
            .map_err(|_| StdinOutputPreparationError::Stdin(StdinPreparationError::Runtime))?;
        let invocation = super::Invocation::from_runtime_input(runtime, input);
        // Descendant construction from ONLY the opaque new pre-run preparation,
        // not a new arbitrary Run/target/Runtime constructor or publisher.
        Ok(Self {
            invocation: super::FileInvocation { invocation, target },
        })
    }
    pub(crate) async fn run(self) -> super::FileRun {
        self.invocation.run().await
    }
    pub(crate) async fn prepare_with_request_limit(
        options: Options,
        sources: Sources<'_>,
        destination: Destination,
        requests: crate::exec::meter::RequestLimit,
    ) -> Result<Self, StdinOutputPreparationError> {
        let prepared = PreparedStdinOutputInput::prepare(options, sources, destination).await?;
        let (prepared, target) = prepared.into_parts();
        let (prepared, input) = prepared.into_parts();
        let runtime = Runtime::build_with_request_limit(prepared, requests)
            .await
            .map_err(|_| StdinOutputPreparationError::Stdin(StdinPreparationError::Runtime))?;
        let invocation = super::Invocation::from_runtime_input(runtime, input);
        Ok(Self {
            invocation: super::FileInvocation { invocation, target },
        })
    }
    #[cfg(test)]
    pub(in crate::exec) async fn prepare_for_tests(
        options: Options,
        sources: Sources<'_>,
        destination: Destination,
        pipe: crate::platform_os::stdin_pipe::Pipe,
        limits: crate::exec::input::stdin::Limits,
        after_capture: impl FnOnce(),
    ) -> Result<Self, StdinOutputPreparationError> {
        Self::build(
            PreparedStdinOutputInput::prepare_for_tests(
                options,
                sources,
                destination,
                pipe,
                limits,
                after_capture,
            )
            .await?,
        )
        .await
    }
}
