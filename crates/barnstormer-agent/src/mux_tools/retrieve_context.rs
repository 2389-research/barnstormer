// ABOUTME: retrieve_context mux tool — lets agents fetch the full text of a
// ABOUTME: context attachment by ID when a summary isn't enough.

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
}

#[async_trait]
impl Tool for RetrieveContextTool {
    fn name(&self) -> &str {
        "retrieve_context"
    }

    fn description(&self) -> &str {
        "Retrieve the full text of a context file attachment by ID. Use this when \
         the summary isn't enough and you need to see the actual content."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "attachment_id": {
                    "type": "string",
                    "description": "The ULID of the attachment to retrieve"
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

        let state = self.actor.read_state().await;
        let att = state
            .context_attachments
            .iter()
            .find(|a| a.attachment_id == attachment_id && !a.removed)
            .ok_or_else(|| anyhow::anyhow!("attachment not found"))?;
        let filename = att.filename.clone();
        let spec_id = self.actor.spec_id;
        drop(state);

        let path = self
            .home
            .join("specs")
            .join(spec_id.to_string())
            .join("context")
            .join(attachment_id.to_string())
            .join(&filename);
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| anyhow::anyhow!("failed to read attachment file: {e}"))?;
        Ok(ToolResult::text(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use barnstormer_core::actor;
    use barnstormer_core::command::Command;
    use barnstormer_core::state::SpecState;
    use tempfile::TempDir;

    #[tokio::test]
    async fn tool_name_is_retrieve_context() {
        let tmp = TempDir::new().unwrap();
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        let tool = RetrieveContextTool {
            actor: Arc::new(handle),
            home: tmp.path().to_path_buf(),
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
        };

        let err = tool
            .execute(json!({ "attachment_id": attachment_id.to_string() }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("attachment not found"));
    }
}
