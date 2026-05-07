// ABOUTME: retrieve_context mux tool — fetches the full text of a context
// ABOUTME: attachment, or asks a focused question about it via the summarizer.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;
use ulid::Ulid;

use barnstormer_core::actor::SpecActorHandle;

#[derive(Clone)]
pub struct RetrieveContextTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) home: PathBuf,
    pub(crate) summarizer: Arc<dyn crate::AttachmentSummarizer>,
}

#[async_trait]
impl Tool for RetrieveContextTool {
    fn name(&self) -> &str {
        "retrieve_context"
    }

    fn description(&self) -> &str {
        "Retrieve content from a context file attachment, or ask a focused question about it. \
         For text files, returns the full content (or a question-targeted answer). For images, \
         PDFs, audio, and video, returns the stored summary (or a fresh summary answering your \
         question)."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "attachment_id": {
                    "type": "string",
                    "description": "The ULID of the attachment to retrieve"
                },
                "question": {
                    "type": "string",
                    "description": "Optional. When provided, dispatches a fresh summarizer call with this question against the attachment. Use when you need a targeted answer the existing summary doesn't cover (e.g. 'what color palette is this?', 'what does this voice memo say about the deadline?', 'what's in section 3 of this PDF?')."
                }
            },
            "required": ["attachment_id"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        let id_str = params
            .get("attachment_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'attachment_id' parameter"))?;
        let attachment_id: Ulid = id_str
            .parse()
            .map_err(|e| anyhow::anyhow!("bad attachment id: {e}"))?;
        let question = params
            .get("question")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let state = self.actor.read_state().await;
        let att_opt = state
            .context_attachments
            .iter()
            .find(|a| a.attachment_id == attachment_id && !a.removed)
            .cloned();
        drop(state);
        let att = att_opt.ok_or_else(|| anyhow::anyhow!("attachment not found"))?;

        // Normalize mime: strip parameters (e.g. "; charset=utf-8"), trim,
        // lowercase. Same convention as build_summarizer_input.
        let mime = att
            .mime_type
            .split(';')
            .next()
            .unwrap_or(&att.mime_type)
            .trim()
            .to_ascii_lowercase();
        let is_text_kind = mime.starts_with("text/")
            || mime == "application/json"
            || mime == "application/yaml"
            || mime == "text/x-yaml";

        let spec_id = self.actor.spec_id;

        match question {
            None if is_text_kind => {
                // Existing behavior: read text from disk.
                let path = self
                    .home
                    .join("specs")
                    .join(spec_id.to_string())
                    .join("context")
                    .join(attachment_id.to_string())
                    .join(&att.filename);
                let text = tokio::fs::read_to_string(&path)
                    .await
                    .map_err(|e| anyhow::anyhow!("failed to read attachment file: {e}"))?;
                Ok(ToolResult::text(text))
            }
            None => {
                // Media without question: return stored summary, error, or pending hint.
                if let Some(reason) = &att.summary_error {
                    Ok(ToolResult::error(format!("summary unavailable: {reason}")))
                } else if let Some(s) = &att.summary {
                    Ok(ToolResult::text(s.clone()))
                } else {
                    Ok(ToolResult::text(
                        "(summary still being generated — retry shortly, or pass a 'question' parameter to fetch a fresh answer now)".to_string()
                    ))
                }
            }
            Some(q) => {
                // Question mode — dispatch via the injected summarizer trait.
                match self.summarizer.answer_question(spec_id, &att, &q).await {
                    Ok(text) => Ok(ToolResult::text(text)),
                    Err(e) => Ok(ToolResult::error(e)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use barnstormer_core::actor;
    use barnstormer_core::command::Command;
    use barnstormer_core::state::SpecState;
    use tempfile::TempDir;

    /// Stub `AttachmentSummarizer` for unit tests — echoes the question back so
    /// tests can assert that `execute` actually dispatched into the trait.
    #[derive(Debug)]
    struct StubSummarizer;

    #[async_trait::async_trait]
    impl crate::AttachmentSummarizer for StubSummarizer {
        async fn answer_question(
            &self,
            _spec_id: Ulid,
            _attachment: &barnstormer_core::state::ContextAttachment,
            question: &str,
        ) -> Result<String, String> {
            Ok(format!("(stub answer to: {question})"))
        }
    }

    fn stub() -> Arc<dyn crate::AttachmentSummarizer> {
        Arc::new(StubSummarizer)
    }

    #[tokio::test]
    async fn tool_name_is_retrieve_context() {
        let tmp = TempDir::new().unwrap();
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        let tool = RetrieveContextTool {
            actor: Arc::new(handle),
            home: tmp.path().to_path_buf(),
            summarizer: stub(),
        };
        assert_eq!(tool.name(), "retrieve_context");
    }

    #[tokio::test]
    async fn retrieve_context_reads_file() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();

        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".to_string(),
                one_liner: "o".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "notes.md".to_string(),
                mime_type: "text/markdown".to_string(),
                size_bytes: 5,
            })
            .await
            .unwrap();

        // Write file to the expected path.
        let dir = home
            .join("specs")
            .join(spec_id.to_string())
            .join("context")
            .join(attachment_id.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.md"), "hello").unwrap();

        let tool = RetrieveContextTool {
            actor: Arc::new(handle),
            home,
            summarizer: stub(),
        };

        let result = tool
            .execute(json!({ "attachment_id": attachment_id.to_string() }))
            .await
            .unwrap();
        assert_eq!(result.content, "hello");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn retrieve_context_rejects_missing_id() {
        let tmp = TempDir::new().unwrap();
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        let tool = RetrieveContextTool {
            actor: Arc::new(handle),
            home: tmp.path().to_path_buf(),
            summarizer: stub(),
        };

        let err = tool.execute(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("attachment_id"));
    }

    #[tokio::test]
    async fn retrieve_context_rejects_bad_id() {
        let tmp = TempDir::new().unwrap();
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        let tool = RetrieveContextTool {
            actor: Arc::new(handle),
            home: tmp.path().to_path_buf(),
            summarizer: stub(),
        };

        let err = tool
            .execute(json!({ "attachment_id": "not-a-ulid" }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("bad attachment id"));
    }

    #[tokio::test]
    async fn retrieve_context_rejects_unknown_id() {
        let tmp = TempDir::new().unwrap();
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        let tool = RetrieveContextTool {
            actor: Arc::new(handle),
            home: tmp.path().to_path_buf(),
            summarizer: stub(),
        };

        let unknown = Ulid::new();
        let err = tool
            .execute(json!({ "attachment_id": unknown.to_string() }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("attachment not found"));
    }

    #[tokio::test]
    async fn retrieve_context_skips_removed_attachments() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();

        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".to_string(),
                one_liner: "o".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "notes.md".to_string(),
                mime_type: "text/markdown".to_string(),
                size_bytes: 5,
            })
            .await
            .unwrap();
        handle
            .send_command(Command::RemoveContext { attachment_id })
            .await
            .unwrap();

        let tool = RetrieveContextTool {
            actor: Arc::new(handle),
            home,
            summarizer: stub(),
        };

        let err = tool
            .execute(json!({ "attachment_id": attachment_id.to_string() }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("attachment not found"));
    }

    #[tokio::test]
    async fn retrieve_context_no_question_on_media_with_summary_returns_summary() {
        // Media attachment with a SummarizeContext recorded — no question:
        // returns the stored summary text, no on-disk read.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();

        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".to_string(),
                one_liner: "o".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "diagram.png".to_string(),
                mime_type: "image/png".to_string(),
                size_bytes: 1024,
            })
            .await
            .unwrap();
        handle
            .send_command(Command::SummarizeContext {
                attachment_id,
                summary: "the stored summary".to_string(),
            })
            .await
            .unwrap();

        let tool = RetrieveContextTool {
            actor: Arc::new(handle),
            home,
            summarizer: stub(),
        };

        let result = tool
            .execute(json!({ "attachment_id": attachment_id.to_string() }))
            .await
            .unwrap();
        assert_eq!(result.content, "the stored summary");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn retrieve_context_no_question_on_media_with_error_returns_tool_error() {
        // Media attachment with a recorded summarize failure — no question:
        // returns an error result that surfaces the failure reason.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();

        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".to_string(),
                one_liner: "o".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "clip.mp4".to_string(),
                mime_type: "video/mp4".to_string(),
                size_bytes: 2048,
            })
            .await
            .unwrap();
        handle
            .send_command(Command::MarkContextSummarizeFailed {
                attachment_id,
                reason: "provider X doesn't support video".to_string(),
            })
            .await
            .unwrap();

        let tool = RetrieveContextTool {
            actor: Arc::new(handle),
            home,
            summarizer: stub(),
        };

        let result = tool
            .execute(json!({ "attachment_id": attachment_id.to_string() }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("provider X"));
    }

    #[tokio::test]
    async fn retrieve_context_no_question_on_pending_media_returns_hint() {
        // Media attachment with no summary and no error — returns a hint
        // text result so the agent knows to retry or pass a question.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();

        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".to_string(),
                one_liner: "o".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "memo.mp3".to_string(),
                mime_type: "audio/mpeg".to_string(),
                size_bytes: 4096,
            })
            .await
            .unwrap();

        let tool = RetrieveContextTool {
            actor: Arc::new(handle),
            home,
            summarizer: stub(),
        };

        let result = tool
            .execute(json!({ "attachment_id": attachment_id.to_string() }))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("still being generated"));
    }

    #[tokio::test]
    async fn retrieve_context_with_question_dispatches_to_summarizer() {
        // Question mode — even on a text attachment, the question dispatches
        // through the AttachmentSummarizer trait rather than reading the file.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();

        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        handle
            .send_command(Command::CreateSpec {
                title: "t".to_string(),
                one_liner: "o".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let attachment_id = Ulid::new();
        handle
            .send_command(Command::AttachContext {
                attachment_id,
                filename: "notes.md".to_string(),
                mime_type: "text/markdown".to_string(),
                size_bytes: 5,
            })
            .await
            .unwrap();

        let tool = RetrieveContextTool {
            actor: Arc::new(handle),
            home,
            summarizer: stub(),
        };

        let result = tool
            .execute(json!({
                "attachment_id": attachment_id.to_string(),
                "question": "test?",
            }))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("stub answer to: test?"));
    }
}
