use crate::error::AgentError;
use crate::model::{ModelRequest, ModelResponse, ToolDef};
use crate::state::{AgentState, ToolCall};
use crate::tool::{Tool, ToolCtx, ToolOutcome};
use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// `review_model` verdict: how the loop continues after a model response.
#[derive(Debug, Clone, PartialEq)]
pub enum Flow {
    Proceed,
    Finish,
    /// Re-run the model immediately.
    Resample,
}

// ---- model-call wrap plumbing ------------------------------------------------

pub type BoxedModelFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ModelResponse, AgentError>> + Send + 'a>>;
pub type ModelHandler<'a> = &'a (dyn Fn(ModelRequest) -> BoxedModelFuture<'a> + Send + Sync);

/// Onion continuation for `around_model`. Consuming `run` walks the remaining
/// middleware, and reaching the empty tail calls the terminal handler.
pub struct Next<'a> {
    chain: &'a [Arc<dyn AgentMiddleware>],
    terminal: ModelHandler<'a>,
}

impl<'a> Next<'a> {
    pub async fn run(self, req: ModelRequest) -> Result<ModelResponse, AgentError> {
        match self.chain.split_first() {
            None => (self.terminal)(req).await,
            Some((mw, rest)) => {
                mw.around_model(
                    req,
                    Next {
                        chain: rest,
                        terminal: self.terminal,
                    },
                )
                .await
            }
        }
    }
}

// ---- tool-call wrap plumbing -------------------------------------------------

pub type BoxedToolFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ToolOutcome, AgentError>> + Send + 'a>>;
/// Terminal tool handler: receives the call plus the live `ToolCtx` (borrowed for
/// the duration of the returned future) and produces the outcome.
pub type ToolHandler<'a> =
    &'a (dyn for<'c, 'd> Fn(ToolCall, &'c mut ToolCtx<'d>) -> BoxedToolFuture<'c> + Send + Sync);

/// Onion continuation for `around_tool`. `ctx` is threaded by `&mut` reborrow
/// through the recursion, like `Next` on the model side.
pub struct ToolNext<'a> {
    chain: &'a [Arc<dyn AgentMiddleware>],
    terminal: ToolHandler<'a>,
}

impl<'a> ToolNext<'a> {
    pub async fn run(
        self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        match self.chain.split_first() {
            None => (self.terminal)(call, ctx).await,
            Some((mw, rest)) => {
                mw.around_tool(
                    call,
                    ctx,
                    ToolNext {
                        chain: rest,
                        terminal: self.terminal,
                    },
                )
                .await
            }
        }
    }
}

// ---- the trait ---------------------------------------------------------------

#[async_trait]
pub trait AgentMiddleware: Send + Sync {
    fn name(&self) -> &str;
    /// Tools this middleware contributes to the agent.
    fn tools(&self) -> Vec<(ToolDef, Arc<dyn Tool>)> {
        vec![]
    }
    async fn turn_start(&self, _st: &mut AgentState) -> Result<(), AgentError> {
        Ok(())
    }
    async fn prepare_model(&self, _st: &mut AgentState) -> Result<(), AgentError> {
        Ok(())
    }
    async fn around_model(
        &self,
        req: ModelRequest,
        next: Next<'_>,
    ) -> Result<ModelResponse, AgentError> {
        next.run(req).await
    }
    async fn review_model(&self, _st: &mut AgentState) -> Result<Flow, AgentError> {
        Ok(Flow::Proceed)
    }
    async fn around_tool(
        &self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        next: ToolNext<'_>,
    ) -> Result<ToolOutcome, AgentError> {
        next.run(call, ctx).await
    }
    async fn turn_end(&self, _st: &mut AgentState) -> Result<(), AgentError> {
        Ok(())
    }
}

// ---- the chain ---------------------------------------------------------------

/// Ordered stack of middleware. Before-hooks run in list order; after-hooks and
/// `turn_end` run in reverse; wrap-hooks form an onion (first entry outermost).
#[derive(Clone)]
pub struct MiddlewareChain {
    list: Vec<Arc<dyn AgentMiddleware>>,
}

impl MiddlewareChain {
    pub fn new(list: Vec<Arc<dyn AgentMiddleware>>) -> Self {
        Self { list }
    }

    pub fn middlewares(&self) -> &[Arc<dyn AgentMiddleware>] {
        &self.list
    }

    pub async fn turn_start(&self, st: &mut AgentState) -> Result<(), AgentError> {
        for m in self.list.iter() {
            m.turn_start(st).await?;
        }
        Ok(())
    }

    pub async fn prepare_model(&self, st: &mut AgentState) -> Result<(), AgentError> {
        for m in self.list.iter() {
            m.prepare_model(st).await?;
        }
        Ok(())
    }

    pub async fn around_model<'a>(
        &'a self,
        req: ModelRequest,
        terminal: ModelHandler<'a>,
    ) -> Result<ModelResponse, AgentError> {
        Next {
            chain: &self.list,
            terminal,
        }
        .run(req)
        .await
    }

    /// Hooks run in reverse list order (the onion's unwinding order); the first
    /// non-Continue verdict in execution order wins; all hooks always run.
    pub async fn review_model_flow(&self, st: &mut AgentState) -> Result<Flow, AgentError> {
        let mut decision = Flow::Proceed;
        for m in self.list.iter().rev() {
            let d = m.review_model(st).await?;
            if decision == Flow::Proceed {
                decision = d;
            }
        }
        Ok(decision)
    }

    /// Run all `review_model` hooks (reverse order), discarding the verdict.
    pub async fn review_model(&self, st: &mut AgentState) -> Result<(), AgentError> {
        self.review_model_flow(st).await.map(|_| ())
    }

    pub async fn around_tool<'a>(
        &'a self,
        call: ToolCall,
        ctx: &mut ToolCtx<'_>,
        terminal: ToolHandler<'a>,
    ) -> Result<ToolOutcome, AgentError> {
        ToolNext {
            chain: &self.list,
            terminal,
        }
        .run(call, ctx)
        .await
    }

    pub async fn turn_end(&self, st: &mut AgentState) -> Result<(), AgentError> {
        for m in self.list.iter().rev() {
            m.turn_end(st).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelRequest, ModelResponse, Usage};
    use crate::state::{AgentState, Msg};
    use std::sync::{Arc, Mutex};

    struct Tagger {
        tag: &'static str,
        log: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl AgentMiddleware for Tagger {
        fn name(&self) -> &str {
            self.tag
        }
        async fn prepare_model(&self, _st: &mut AgentState) -> Result<(), crate::AgentError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("before:{}", self.tag));
            Ok(())
        }
        async fn review_model(&self, _st: &mut AgentState) -> Result<Flow, crate::AgentError> {
            self.log.lock().unwrap().push(format!("after:{}", self.tag));
            Ok(Flow::Proceed)
        }
        async fn around_model(
            &self,
            req: ModelRequest,
            next: Next<'_>,
        ) -> Result<ModelResponse, crate::AgentError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("wrap-in:{}", self.tag));
            let r = next.run(req).await;
            self.log
                .lock()
                .unwrap()
                .push(format!("wrap-out:{}", self.tag));
            r
        }
    }

    #[tokio::test]
    async fn hooks_compose_in_onion_order() {
        let log = Arc::new(Mutex::new(vec![]));
        let chain = MiddlewareChain::new(vec![
            Arc::new(Tagger {
                tag: "A",
                log: log.clone(),
            }),
            Arc::new(Tagger {
                tag: "B",
                log: log.clone(),
            }),
        ]);
        let mut st = AgentState::default();
        chain.prepare_model(&mut st).await.unwrap();
        let resp = ModelResponse {
            message: Msg::assistant("x"),
            usage: Usage::default(),
        };
        chain
            .around_model(
                ModelRequest {
                    system: String::new(),
                    messages: vec![],
                    tools: vec![],
                },
                &|_req| Box::pin(async { Ok(resp.clone()) }),
            )
            .await
            .unwrap();
        chain.review_model(&mut st).await.unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                "before:A",
                "before:B",
                "wrap-in:A",
                "wrap-in:B",
                "wrap-out:B",
                "wrap-out:A",
                "after:B",
                "after:A",
            ]
        );
    }

    // ---- tool-wrap onion ordering ------------------------------------------

    use crate::backend::StateFs;
    use crate::state::ToolCall;
    use crate::tool::{ToolCtx, ToolOutcome};

    struct ToolTagger {
        tag: &'static str,
        log: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl AgentMiddleware for ToolTagger {
        fn name(&self) -> &str {
            self.tag
        }
        async fn around_tool(
            &self,
            call: ToolCall,
            ctx: &mut ToolCtx<'_>,
            next: ToolNext<'_>,
        ) -> Result<ToolOutcome, crate::AgentError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("twrap-in:{}", self.tag));
            let r = next.run(call, ctx).await;
            self.log
                .lock()
                .unwrap()
                .push(format!("twrap-out:{}", self.tag));
            r
        }
    }

    #[tokio::test]
    async fn tool_hooks_compose_in_onion_order() {
        let log = Arc::new(Mutex::new(vec![]));
        let chain = MiddlewareChain::new(vec![
            Arc::new(ToolTagger {
                tag: "A",
                log: log.clone(),
            }),
            Arc::new(ToolTagger {
                tag: "B",
                log: log.clone(),
            }),
        ]);
        let mut st = AgentState::default();
        let backend = Arc::new(StateFs::new(Arc::new(Mutex::new(AgentState::default()))));
        let registry = Arc::new(crate::subagent::AgentRegistry::default());
        let (notifications, _rx) = crate::subagent::notification_channel();
        let mut ctx = ToolCtx {
            state: &mut st,
            backend,
            registry,
            notifications,
        };
        let call = ToolCall {
            id: "c0".into(),
            name: "noop".into(),
            args: serde_json::json!({}),
        };
        // A `fn` item (not a closure) carries the higher-ranked return-lifetime
        // bound `for<'c> Fn(ToolCall, &'c mut ToolCtx) -> BoxedToolFuture<'c>`;
        // closures can't infer a return lifetime tied to an argument.
        fn terminal<'c>(_call: ToolCall, _ctx: &'c mut ToolCtx<'_>) -> BoxedToolFuture<'c> {
            Box::pin(async { Ok(ToolOutcome::ok("t")) })
        }
        let out = chain.around_tool(call, &mut ctx, &terminal).await.unwrap();
        assert_eq!(out.content, "t");
        assert_eq!(
            *log.lock().unwrap(),
            vec!["twrap-in:A", "twrap-in:B", "twrap-out:B", "twrap-out:A",]
        );
    }
}
