//! Backlog data → OCEL 2.0 mapping (via the `StagingLog` gate).
//!
//! Event types: `task_created`, `comment_added`, and — from `changeLog`
//! entries — `status_changed` / `assignee_changed` / `priority_changed` /
//! `milestone_changed` / `due_date_changed`. Other changeLog fields are
//! skipped for now.
//!
//! Object types: `task` (with `status` / `priority` / `assignee` / `due_date`
//! as dynamic attributes, initial values reconstructed from the first change's
//! `originalValue`), `user`, `project`, `milestone`, `category`.

use std::collections::{BTreeMap, HashMap};

use chrono::DateTime;
use ocel::AttrValue;
use ocel_etl::{StagingEvent, StagingLog};

use crate::models::{ChangeLogEntry, Comment, Issue, Project};

/// Map a project's issues (with their comments) into a [`StagingLog`].
///
/// The result still has to pass the gate: call
/// [`StagingLog::into_ocel`] to obtain a validated log.
#[must_use]
pub fn map_project(project: &Project, issues: &[(Issue, Vec<Comment>)]) -> StagingLog {
    let mut staging = StagingLog::new();
    map_project_into(&mut staging, project, issues);
    staging
}

/// A project paired with its issues, each issue paired with its comments.
pub type ProjectIssues = (Project, Vec<(Issue, Vec<Comment>)>);

/// Map several projects into one [`StagingLog`] (objects such as users merge
/// naturally across projects).
#[must_use]
pub fn map_projects(projects: &[ProjectIssues]) -> StagingLog {
    let mut staging = StagingLog::new();
    for (project, issues) in projects {
        map_project_into(&mut staging, project, issues);
    }
    staging
}

/// Append one project's data to an existing [`StagingLog`].
pub fn map_project_into(
    staging: &mut StagingLog,
    project: &Project,
    issues: &[(Issue, Vec<Comment>)],
) {
    let mut mapper = ProjectMapper::new(project, issues.iter().map(|(issue, _)| issue), false);
    mapper.register(staging);
    for (issue, comments) in issues {
        mapper.map_issue(staging, issue, comments);
    }
}

/// Streaming-friendly mapper for one project: build it from the (lightweight)
/// issue list, then feed each issue's comments as they are fetched and drop
/// them — memory over comments stays constant.
///
/// The issue list is needed up front only to resolve `parentIssueId` into
/// issue keys.
#[derive(Debug)]
pub struct ProjectMapper<'a> {
    project: &'a Project,
    key_of: HashMap<u64, String>,
    /// Store comment text as a `body` event attribute (default off:
    /// Backlog spaces are private data; opt in with `--comment-bodies`
    /// when content predicates need the text).
    comment_bodies: bool,
    skipped: BTreeMap<String, usize>,
}

impl<'a> ProjectMapper<'a> {
    /// Build the mapper from the project and its issue list.
    pub fn new<'i>(
        project: &'a Project,
        issues: impl IntoIterator<Item = &'i Issue>,
        comment_bodies: bool,
    ) -> Self {
        let key_of = issues
            .into_iter()
            .map(|issue| (issue.id, issue.issue_key.clone()))
            .collect();
        Self {
            project,
            key_of,
            comment_bodies,
            skipped: BTreeMap::new(),
        }
    }

    /// Register the project object (call once per project).
    pub fn register(&self, staging: &mut StagingLog) {
        staging.upsert_object(&self.project.project_key, "project");
        staging.add_object_attribute(
            &self.project.project_key,
            "name",
            AttrValue::String(self.project.name.clone()),
            DateTime::UNIX_EPOCH,
        );
    }

    /// Map one issue (with its comments) into the staging log.
    pub fn map_issue(&mut self, staging: &mut StagingLog, issue: &Issue, comments: &[Comment]) {
        map_issue(
            staging,
            self.project,
            issue,
            comments,
            &self.key_of,
            self.comment_bodies,
            &mut self.skipped,
        );
    }

    /// `changeLog` fields that were skipped (not in the mapping whitelist),
    /// with occurrence counts. Dropping is deliberate; dropping silently is not.
    #[must_use]
    pub fn skipped_fields(&self) -> &BTreeMap<String, usize> {
        &self.skipped
    }
}

fn user_object(staging: &mut StagingLog, user: &crate::models::User) -> String {
    let id = format!("user:{}", user.id);
    staging.upsert_object(&id, "user");
    staging.add_object_attribute(
        &id,
        "name",
        AttrValue::String(user.name.clone()),
        DateTime::UNIX_EPOCH,
    );
    id
}

fn map_issue(
    staging: &mut StagingLog,
    project: &Project,
    issue: &Issue,
    comments: &[Comment],
    key_of: &HashMap<u64, String>,
    comment_bodies: bool,
    skipped: &mut BTreeMap<String, usize>,
) {
    let task = issue.issue_key.as_str();
    let epoch = DateTime::UNIX_EPOCH;

    staging.upsert_object(task, "task");
    staging.add_object_attribute(
        task,
        "summary",
        AttrValue::String(issue.summary.clone()),
        epoch,
    );
    staging.add_object_attribute(
        task,
        "issue_type",
        AttrValue::String(issue.issue_type.name.clone()),
        epoch,
    );

    // O2O: project / parent / milestone / category
    staging.add_o2o(task, &project.project_key, "belongs to");
    if let Some(parent_id) = issue.parent_issue_id {
        if let Some(parent_key) = key_of.get(&parent_id) {
            staging.add_o2o(parent_key, task, "parent of");
        }
    }
    for milestone in &issue.milestone {
        let id = format!("milestone:{}", milestone.id);
        staging.upsert_object(&id, "milestone");
        staging.add_object_attribute(
            &id,
            "name",
            AttrValue::String(milestone.name.clone()),
            epoch,
        );
        staging.add_o2o(task, &id, "assigned to");
    }
    for category in &issue.category {
        let id = format!("category:{}", category.id);
        staging.upsert_object(&id, "category");
        staging.add_object_attribute(&id, "name", AttrValue::String(category.name.clone()), epoch);
        staging.add_o2o(task, &id, "categorized as");
    }

    // task_created event
    let creator = user_object(staging, &issue.created_user);
    staging.add_event(StagingEvent {
        id: format!("{task}/created"),
        event_type: "task_created".into(),
        time: issue.created,
        attributes: vec![],
        relations: vec![
            (task.to_owned(), "created task".into()),
            (creator, "creator".into()),
            (project.project_key.clone(), "belongs to project".into()),
        ],
    });

    // dynamic attributes: initial value at creation time, reconstructed from
    // the first change's originalValue (falling back to the current value)
    let changes = collect_changes(comments);
    record_initial(staging, task, issue, &changes);

    // comments: plain ones become comment_added; changeLog entries become
    // field-specific change events (and dynamic attribute updates)
    for comment in comments {
        let commenter = user_object(staging, &comment.created_user);
        if let Some(content) = comment.content.as_deref().filter(|c| !c.is_empty()) {
            let attributes = if comment_bodies {
                vec![("body".to_owned(), AttrValue::String(content.to_owned()))]
            } else {
                vec![]
            };
            staging.add_event(StagingEvent {
                id: format!("{task}/comment/{}", comment.id),
                event_type: "comment_added".into(),
                time: comment.created,
                attributes,
                relations: vec![
                    (task.to_owned(), "commented on".into()),
                    (commenter.clone(), "commenter".into()),
                ],
            });
        }
        for (index, change) in comment.change_log.iter().enumerate() {
            if change_kind(&change.field).is_none() {
                *skipped.entry(change.field.clone()).or_insert(0) += 1;
                continue;
            }
            map_change(staging, task, comment, index, change, &commenter);
        }
    }
}

/// (event type, dynamic attribute name) for a changeLog field.
fn change_kind(field: &str) -> Option<(&'static str, &'static str)> {
    match field {
        "status" => Some(("status_changed", "status")),
        "assigner" => Some(("assignee_changed", "assignee")),
        "priority" => Some(("priority_changed", "priority")),
        "milestone" => Some(("milestone_changed", "milestone")),
        "limitDate" => Some(("due_date_changed", "due_date")),
        _ => None,
    }
}

fn collect_changes(comments: &[Comment]) -> HashMap<&'static str, Vec<&ChangeLogEntry>> {
    let mut changes: HashMap<&'static str, Vec<&ChangeLogEntry>> = HashMap::new();
    for comment in comments {
        for change in &comment.change_log {
            if let Some((_, attr)) = change_kind(&change.field) {
                changes.entry(attr).or_default().push(change);
            }
        }
    }
    changes
}

fn record_initial(
    staging: &mut StagingLog,
    task: &str,
    issue: &Issue,
    changes: &HashMap<&'static str, Vec<&ChangeLogEntry>>,
) {
    let initial = |attr: &str, current: Option<String>| -> Option<String> {
        match changes.get(attr).and_then(|c| c.first()) {
            Some(first) => first.original_value.clone(),
            None => current,
        }
    };

    let fields: [(&str, Option<String>); 4] = [
        ("status", Some(issue.status.name.clone())),
        ("priority", Some(issue.priority.name.clone())),
        ("assignee", issue.assignee.as_ref().map(|u| u.name.clone())),
        ("due_date", issue.due_date.map(|d| d.to_rfc3339())),
    ];
    for (attr, current) in fields {
        if let Some(value) = initial(attr, current) {
            staging.add_object_attribute(task, attr, AttrValue::String(value), issue.created);
        }
    }
}

fn map_change(
    staging: &mut StagingLog,
    task: &str,
    comment: &Comment,
    index: usize,
    change: &ChangeLogEntry,
    changer: &str,
) {
    let Some((event_type, attr)) = change_kind(&change.field) else {
        return;
    };
    let mut attributes = vec![(
        "_source".to_owned(),
        AttrValue::String("backlog-changelog".into()),
    )];
    if let Some(old) = &change.original_value {
        attributes.push(("old_value".into(), AttrValue::String(old.clone())));
    }
    if let Some(new) = &change.new_value {
        attributes.push(("new_value".into(), AttrValue::String(new.clone())));
        staging.add_object_attribute(task, attr, AttrValue::String(new.clone()), comment.created);
    }
    staging.add_event(StagingEvent {
        id: format!("{task}/change/{}/{index}", comment.id),
        event_type: event_type.into(),
        time: comment.created,
        attributes,
        relations: vec![
            (task.to_owned(), format!("{attr} updated")),
            (changer.to_owned(), "changed by".into()),
        ],
    });
}
