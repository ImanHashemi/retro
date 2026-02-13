use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Pattern types ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PatternType {
    RepetitiveInstruction,
    RecurringMistake,
    WorkflowPattern,
    StaleContext,
    RedundantContext,
}

impl std::fmt::Display for PatternType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RepetitiveInstruction => write!(f, "repetitive_instruction"),
            Self::RecurringMistake => write!(f, "recurring_mistake"),
            Self::WorkflowPattern => write!(f, "workflow_pattern"),
            Self::StaleContext => write!(f, "stale_context"),
            Self::RedundantContext => write!(f, "redundant_context"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PatternStatus {
    Discovered,
    Active,
    Archived,
    Dismissed,
}

impl std::fmt::Display for PatternStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Discovered => write!(f, "discovered"),
            Self::Active => write!(f, "active"),
            Self::Archived => write!(f, "archived"),
            Self::Dismissed => write!(f, "dismissed"),
        }
    }
}

impl PatternStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "discovered" => Self::Discovered,
            "active" => Self::Active,
            "archived" => Self::Archived,
            "dismissed" => Self::Dismissed,
            _ => Self::Discovered,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SuggestedTarget {
    Skill,
    ClaudeMd,
    GlobalAgent,
    DbOnly,
}

impl std::fmt::Display for SuggestedTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Skill => write!(f, "skill"),
            Self::ClaudeMd => write!(f, "claude_md"),
            Self::GlobalAgent => write!(f, "global_agent"),
            Self::DbOnly => write!(f, "db_only"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    pub id: String,
    pub pattern_type: PatternType,
    pub description: String,
    pub confidence: f64,
    pub times_seen: i64,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub last_projected: Option<DateTime<Utc>>,
    pub status: PatternStatus,
    pub source_sessions: Vec<String>,
    pub related_files: Vec<String>,
    pub suggested_content: String,
    pub suggested_target: SuggestedTarget,
    pub project: Option<String>,
    pub generation_failed: bool,
}

// ── Session JSONL types ──

/// Top-level entry in a session JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionEntry {
    #[serde(rename = "user")]
    User(UserEntry),
    #[serde(rename = "assistant")]
    Assistant(AssistantEntry),
    #[serde(rename = "summary")]
    Summary(SummaryEntry),
    #[serde(rename = "file-history-snapshot")]
    FileHistorySnapshot(serde_json::Value),
    #[serde(rename = "progress")]
    Progress(serde_json::Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserEntry {
    pub uuid: String,
    #[serde(default)]
    pub parent_uuid: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    pub message: UserMessage,
    #[serde(default)]
    pub is_sidechain: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub role: String,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Extract the user-facing text from the message content.
    pub fn as_text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Blocks(blocks) => {
                let mut parts = Vec::new();
                for block in blocks {
                    match block {
                        ContentBlock::Text { text } => parts.push(text.clone()),
                        ContentBlock::ToolResult { content, .. } => {
                            if let Some(c) = content {
                                parts.push(c.as_text());
                            }
                        }
                        _ => {}
                    }
                }
                parts.join("\n")
            }
        }
    }

    /// Returns true if this is a tool_result message (not a user prompt).
    pub fn is_tool_result(&self) -> bool {
        matches!(self, MessageContent::Blocks(blocks) if blocks.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. })))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Option<ToolResultContent>,
    },
}

/// Tool result content can be a string or an array of content blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<serde_json::Value>),
}

impl ToolResultContent {
    pub fn as_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Blocks(blocks) => {
                blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantEntry {
    pub uuid: String,
    #[serde(default)]
    pub parent_uuid: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    pub message: AssistantMessage,
    #[serde(default)]
    pub is_sidechain: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub role: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryEntry {
    pub uuid: String,
    #[serde(default)]
    pub parent_uuid: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    /// Summaries may also carry a message field.
    #[serde(default)]
    pub message: Option<serde_json::Value>,
}

// ── Parsed session (for analysis) ──

/// A parsed and processed session ready for analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub project: String,
    pub session_path: String,
    pub user_messages: Vec<ParsedUserMessage>,
    pub assistant_messages: Vec<ParsedAssistantMessage>,
    pub summaries: Vec<String>,
    pub tools_used: Vec<String>,
    pub errors: Vec<String>,
    pub metadata: SessionMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedUserMessage {
    pub text: String,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedAssistantMessage {
    pub text: String,
    pub thinking_summary: Option<String>,
    pub tools: Vec<String>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub cwd: Option<String>,
    pub version: Option<String>,
    pub git_branch: Option<String>,
    pub model: Option<String>,
}

// ── History entry ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default)]
    pub timestamp: Option<u64>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

// ── Context snapshot ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub claude_md: Option<String>,
    pub skills: Vec<SkillFile>,
    pub memory_md: Option<String>,
    pub global_agents: Vec<AgentFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentFile {
    pub path: String,
    pub content: String,
}

// ── Ingestion tracking ──

#[derive(Debug, Clone)]
pub struct IngestedSession {
    pub session_id: String,
    pub project: String,
    pub session_path: String,
    pub file_size: u64,
    pub file_mtime: String,
    pub ingested_at: DateTime<Utc>,
}
