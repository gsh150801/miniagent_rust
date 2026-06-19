use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_core::event::{ContentBlock, StopReason, Usage};
use tokio_util::sync::CancellationToken;

use crate::traits::{CompletionRequest, CompletionResponse, LlmProvider, StreamChunk, StreamResponse};

pub struct MockProvider {
    response: String,
    delay_ms: u64,
}

impl MockProvider {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            delay_ms: 0,
        }
    }

    pub fn with_delay(mut self, ms: u64) -> Self {
        self.delay_ms = ms;
        self
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    async fn complete(
        &self,
        _request: &CompletionRequest,
        cancel: CancellationToken,
    ) -> Result<CompletionResponse, AgentError> {
        if self.delay_ms > 0 {
            tokio::select! {
                _ = cancel.cancelled() => return Err(AgentError::Cancelled),
                _ = tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)) => {},
            }
        }

        Ok(CompletionResponse {
            content: vec![ContentBlock::Text {
                text: self.response.clone(),
            }],
            usage: Usage {
                input_tokens: 10,
                output_tokens: 20,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
            stop_reason: StopReason::EndTurn,
        })
    }

    async fn stream(
        &self,
        request: &CompletionRequest,
        cancel: CancellationToken,
    ) -> Result<StreamResponse, AgentError> {
        let response = self.complete(request, cancel).await?;
        let text = match &response.content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => String::new(),
        };

        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            let _ = tx
                .send(Ok(StreamChunk::TextDelta { text }))
                .await;
            let _ = tx
                .send(Ok(StreamChunk::Stop(StopReason::EndTurn)))
                .await;
        });

        Ok(StreamResponse {
            content_receiver: rx,
        })
    }
}
