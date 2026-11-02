// ABOUTME: Test utilities for barnstormer-agent, including a stub LLM client.
// ABOUTME: Used in tests to simulate LLM responses without real API calls.

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use mux::error::LlmError;
use mux::llm::{ContentBlock, LlmClient, Request, Response, StopReason, StreamEvent, Usage};

/// A stub LLM client that returns a pre-configured text response.
///
/// Useful in tests to drive a SubAgent to immediate completion without
/// making real API calls. The response contains only a text content block,
/// so the agent loop sees no tool-use requests and terminates.
#[derive(Debug, Clone)]
pub struct StubLlmClient {
    response_text: String,
    usage: Usage,
}

impl StubLlmClient {
    /// Create a stub client that always returns the given text.
    pub fn new(response_text: &str) -> Self {
        Self {
            response_text: response_text.to_owned(),
            usage: Usage::default(),
        }
    }

    /// Create a stub client that returns "Done."
    ///
    /// Convenience constructor for the common case where you just need the
    /// agent loop to complete without doing anything interesting.
    pub fn done() -> Self {
        Self::new("Done.")
    }

    /// Create a stub response with explicit provider token usage.
    pub fn with_usage(response_text: &str, usage: Usage) -> Self {
        Self {
            response_text: response_text.to_owned(),
            usage,
        }
    }
}

#[async_trait]
impl LlmClient for StubLlmClient {
    async fn create_message(&self, _req: &Request) -> Result<Response, LlmError> {
        Ok(Response {
            id: "stub-msg-001".to_owned(),
            content: vec![ContentBlock::text(&self.response_text)],
            stop_reason: StopReason::EndTurn,
            model: "stub-model".to_owned(),
            usage: self.usage.clone(),
        })
    }

    fn create_message_stream(
        &self,
        _req: &Request,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, LlmError>> + Send + 'static>> {
        // Return an empty stream rather than panicking. Callers that need
        // streaming behaviour should use a real client or a dedicated mock.
        Box::pin(futures::stream::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stub_new_returns_configured_response_text() {
        let client = StubLlmClient::new("Hello, world!");
        let req = Request::new("test-model");
        let resp = client.create_message(&req).await.unwrap();

        assert_eq!(resp.text(), "Hello, world!");
    }

    #[tokio::test]
    async fn stub_done_returns_done_text() {
        let client = StubLlmClient::done();
        let req = Request::new("test-model");
        let resp = client.create_message(&req).await.unwrap();

        assert_eq!(resp.text(), "Done.");
    }

    #[tokio::test]
    async fn stub_response_has_correct_structure() {
        let client = StubLlmClient::new("test output");
        let req = Request::new("test-model");
        let resp = client.create_message(&req).await.unwrap();

        // Role is implicit in the Response struct (always assistant for LLM responses).
        // Verify stop reason indicates natural completion.
        assert_eq!(resp.stop_reason, StopReason::EndTurn);

        // Verify it has exactly one text content block.
        assert_eq!(resp.content.len(), 1);
        assert!(matches!(&resp.content[0], ContentBlock::Text { text } if text == "test output"));

        // Verify there are no tool-use blocks (agent should not loop).
        assert!(!resp.has_tool_use());
    }

    #[tokio::test]
    async fn stub_with_usage_returns_configured_usage() {
        let usage = Usage {
            input_tokens: 11,
            output_tokens: 7,
            cache_read_tokens: 999,
            cache_write_tokens: 13,
        };
        let client = StubLlmClient::with_usage("used tokens", usage.clone());

        let response = client
            .create_message(&Request::new("stub-model"))
            .await
            .unwrap();

        assert_eq!(response.usage.input_tokens, usage.input_tokens);
        assert_eq!(response.usage.output_tokens, usage.output_tokens);
        assert_eq!(response.usage.cache_read_tokens, usage.cache_read_tokens);
        assert_eq!(response.usage.cache_write_tokens, usage.cache_write_tokens);
    }
}
