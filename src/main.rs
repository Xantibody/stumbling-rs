use anyhow::{Context, Result};
use rmcp::{
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::*,
    service::Peer,
    tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};
use std::{env, path::PathBuf};

mod notes;

#[derive(Clone)]
pub struct StumblingServer {
    root: PathBuf,
    parse_frontmatter: bool,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ReadNoteParams {
    /// Relative path to the note from STUMBLING_ROOT (e.g., "daily/2024-01-01.md")
    path: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SearchNotesParams {
    /// Search query (supports regex)
    query: String,
    /// Maximum number of results to return (default: 20)
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    20
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct WriteNoteParams {
    /// Relative path to the note from STUMBLING_ROOT (e.g., "daily/2024-01-01.md")
    path: String,
    /// Body content to write to the note
    content: String,
    /// Optional YAML frontmatter metadata (e.g., {"title": "My Note", "tags": ["rust"]})
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DeleteNoteParams {
    /// Relative path to the note from STUMBLING_ROOT
    path: String,
    /// If true, permanently delete. If false (default), move to .trash directory.
    #[serde(default)]
    permanent: bool,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SearchMetadataParams {
    /// Field to search in frontmatter (e.g., "title", "tags", "author.name")
    field: String,
    /// Value pattern to match (supports regex)
    pattern: String,
    /// Maximum number of results to return (default: 20)
    #[serde(default = "default_limit")]
    limit: usize,
}

#[tool_router]
impl StumblingServer {
    pub fn new() -> Result<Self> {
        let root =
            env::var("STUMBLING_ROOT").context("STUMBLING_ROOT environment variable not set")?;
        let root = PathBuf::from(root);

        if !root.exists() {
            anyhow::bail!("STUMBLING_ROOT does not exist: {}", root.display());
        }

        let parse_frontmatter = env::var("STUMBLING_PARSE_FRONTMATTER")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        Ok(Self {
            root,
            parse_frontmatter,
            tool_router: Self::tool_router(),
        })
    }

    /// Read a markdown note from the vault.
    /// Returns the note content, optionally with frontmatter parsed separately.
    #[tool(name = "read_note")]
    async fn read_note(
        &self,
        params: Parameters<ReadNoteParams>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(params) = params;
        let path = self.root.join(&params.path);

        match notes::read_note(&path, self.parse_frontmatter) {
            Ok(content) => Ok(CallToolResult::success(vec![Content::text(content)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to read note: {}",
                e
            ))])),
        }
    }

    /// Search for notes containing the given query.
    /// Uses parallel processing for fast search across all markdown files.
    #[tool(name = "search_notes")]
    async fn search_notes(
        &self,
        params: Parameters<SearchNotesParams>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(params) = params;

        match notes::search_notes(&self.root, &params.query, params.limit) {
            Ok(results) => {
                let output =
                    serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string());
                Ok(CallToolResult::success(vec![Content::text(output)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Search failed: {}",
                e
            ))])),
        }
    }

    /// Search notes by frontmatter metadata field.
    /// Supports nested fields with dot notation (e.g., "author.name").
    #[tool(name = "search_metadata")]
    async fn search_metadata(
        &self,
        params: Parameters<SearchMetadataParams>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(params) = params;

        match notes::search_metadata(&self.root, &params.field, &params.pattern, params.limit) {
            Ok(results) => {
                let output =
                    serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string());
                Ok(CallToolResult::success(vec![Content::text(output)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Metadata search failed: {}",
                e
            ))])),
        }
    }

    /// Create or overwrite a markdown note.
    /// Creates parent directories if they don't exist.
    /// If metadata is provided, formats as YAML frontmatter.
    #[tool(name = "write_note")]
    async fn write_note(
        &self,
        params: Parameters<WriteNoteParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(params) = params;
        let path = self.root.join(&params.path);
        let is_overwrite = path.exists();

        // Format content with frontmatter if metadata is provided
        let content = match params.metadata {
            Some(meta) => notes::format_with_frontmatter(&meta, &params.content),
            None => params.content.clone(),
        };

        match notes::write_note(&path, &content) {
            Ok(()) => {
                let action = if is_overwrite { "Overwrote" } else { "Created" };
                let msg = format!("{} {}", action, params.path);

                let _ = peer
                    .notify_logging_message(LoggingMessageNotificationParam {
                        level: LoggingLevel::Info,
                        logger: Some("stumbling-rs".into()),
                        data: msg.clone().into(),
                    })
                    .await;

                Ok(CallToolResult::success(vec![Content::text(msg)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to write note: {}",
                e
            ))])),
        }
    }

    /// Delete a markdown note.
    /// By default, moves to .trash directory. Set permanent=true to permanently delete.
    #[tool(name = "delete_note")]
    async fn delete_note(
        &self,
        params: Parameters<DeleteNoteParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(params) = params;
        let path = self.root.join(&params.path);

        match notes::delete_note(&self.root, &path, params.permanent) {
            Ok(msg) => {
                let _ = peer
                    .notify_logging_message(LoggingMessageNotificationParam {
                        level: LoggingLevel::Info,
                        logger: Some("stumbling-rs".into()),
                        data: msg.clone().into(),
                    })
                    .await;

                Ok(CallToolResult::success(vec![Content::text(msg)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to delete note: {}",
                e
            ))])),
        }
    }
}

#[tool_handler]
impl ServerHandler for StumblingServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_logging()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "MCP server for reading and searching markdown notes in a local vault.".to_string(),
            ),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let server = StumblingServer::new()?;

    let transport = rmcp::transport::io::stdio();
    let service = server.serve(transport).await?;

    service.waiting().await?;

    Ok(())
}
