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

use std::collections::HashMap;

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
    let epoch = DateTime::UNIX_EPOCH;

    staging.upsert_object(&project.project_key, "project");
    staging.add_object_attribute(
        &project.project_key,
        "name",
        AttrValue::String(project.name.clone()),
        epoch,
    );

    // issue id -> key, for resolving parentIssueId (same-project only)
    let key_of: HashMap<u64, &str> = issues
        .iter()
        .map(|(issue, _)| (issue.id, issue.issue_key.as_str()))
        .collect();

    for (issue, comments) in issues {
        map_issue(staging, project, issue, comments, &key_of);
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
    key_of: &HashMap<u64, &str>,
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
        if comment.content.as_deref().is_some_and(|c| !c.is_empty()) {
            staging.add_event(StagingEvent {
                id: format!("{task}/comment/{}", comment.id),
                event_type: "comment_added".into(),
                time: comment.created,
                attributes: vec![],
                relations: vec![
                    (task.to_owned(), "commented on".into()),
                    (commenter.clone(), "commenter".into()),
                ],
            });
        }
        for (index, change) in comment.change_log.iter().enumerate() {
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
