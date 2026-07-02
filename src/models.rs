//! Backlog API v2 response models (only the fields the mapping needs).
//!
//! Unknown fields are ignored by serde, so API additions don't break parsing.

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub id: u64,
    #[serde(rename = "projectKey")]
    pub project_key: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Named {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub id: u64,
    #[serde(rename = "issueKey")]
    pub issue_key: String,
    pub summary: String,
    #[serde(rename = "issueType")]
    pub issue_type: Named,
    pub status: Named,
    pub priority: Named,
    pub assignee: Option<User>,
    #[serde(default)]
    pub category: Vec<Named>,
    #[serde(default)]
    pub milestone: Vec<Named>,
    #[serde(rename = "parentIssueId")]
    pub parent_issue_id: Option<u64>,
    #[serde(rename = "createdUser")]
    pub created_user: User,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    #[serde(rename = "dueDate")]
    pub due_date: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Comment {
    pub id: u64,
    /// Plain comments carry text; pure change records have `null` content.
    pub content: Option<String>,
    #[serde(rename = "createdUser")]
    pub created_user: User,
    pub created: DateTime<Utc>,
    #[serde(rename = "changeLog", default)]
    pub change_log: Vec<ChangeLogEntry>,
}

/// One field change recorded on a comment (Backlog stores status/assignee/...
/// changes as `changeLog` entries attached to comments).
#[derive(Debug, Clone, Deserialize)]
pub struct ChangeLogEntry {
    pub field: String,
    #[serde(rename = "newValue")]
    pub new_value: Option<String>,
    #[serde(rename = "originalValue")]
    pub original_value: Option<String>,
}
