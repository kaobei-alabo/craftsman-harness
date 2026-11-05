//! Private actual-builder request producer. Response/control parsing remains
//! original/private; no public output/storage/trace, token meter or live exec.
pub(crate) mod diagnostics;
pub(crate) mod request;
use crate::agent::ProviderScope;
use crate::llm::LlmClient;
use async_trait::async_trait;
use craftsman_harness::{AgentError, ModelProvider, ModelRequest, ModelResponse, Usage};
use std::sync::{Arc, Mutex};

pub(crate) struct Provider {
    client: LlmClient,
    usage: Arc<Mutex<(u64, u64)>>,
}
impl Provider {
    #[cfg(test)]
    pub(crate) fn diagnostic_for_tests(&self) -> Option<Arc<diagnostics::Diagnostic>> {
        self.client.diagnostic_for_tests()
    }
    pub(crate) fn from_scope(scope: ProviderScope<'_>) -> Result<Self, request::Fault> {
        let usage = scope.usage();
        Ok(Self {
            client: LlmClient::headless(scope)?,
            usage,
        })
    }
}
#[async_trait]
impl ModelProvider for Provider {
    async fn complete(&self, original: ModelRequest) -> Result<ModelResponse, AgentError> {
        // All delta/status callbacks remain silent. A transport/response adapter
        // still precedes any publication; never echo its private raw diagnostics.
        let turn = self
            .client
            .stream_model_turn(&original)
            .await
            .map_err(|_| AgentError::Model("[REDACTED]".into()))?;
        {
            let mut usage = self.usage.lock().unwrap();
            usage.0 += turn.usage.input_tokens;
            usage.1 += turn.usage.output_tokens;
        }
        let usage = Usage {
            input_tokens: turn.usage.input_tokens,
            output_tokens: turn.usage.output_tokens,
        };
        Ok(ModelResponse {
            message: crate::agent::to_fw_message(turn),
            usage,
        })
    }
}

impl Provider {
    pub(in crate::exec) fn prepare_reservation(
        &self,
        original: &ModelRequest,
    ) -> Result<request::Checked, request::Fault> {
        self.client.prepare_reservation(original)
    }
    pub(in crate::exec) async fn complete_reserved(
        &self,
        admitted: &crate::exec::meter::tokens::Admitted,
    ) -> Result<ModelResponse, AgentError> {
        let turn = self
            .client
            .stream_admitted_model_turn(admitted)
            .await
            .map_err(|_| AgentError::Model("[REDACTED]".into()))?;
        {
            let mut usage = self.usage.lock().unwrap();
            usage.0 += turn.usage.input_tokens;
            usage.1 += turn.usage.output_tokens;
        }
        let usage = Usage {
            input_tokens: turn.usage.input_tokens,
            output_tokens: turn.usage.output_tokens,
        };
        Ok(ModelResponse {
            message: crate::agent::to_fw_message(turn),
            usage,
        })
    }
}

#[cfg(test)]
impl Provider {
    pub(crate) fn request_source_for_tests(&self) -> Arc<request::Source> {
        self.client.request_source_for_tests().unwrap()
    }
}

#[cfg(test)]
impl Provider {
    pub(crate) fn observation_client_for_tests(&self) -> &crate::llm::LlmClient {
        &self.client
    }
}
