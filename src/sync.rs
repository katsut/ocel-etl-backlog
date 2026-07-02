//! Incremental sync: merge a fresh pull of updated issues into an existing log.
//!
//! Event ids produced by this connector are prefixed with the issue key
//! (`KEY/created`, `KEY/comment/<id>`, `KEY/change/<id>/<n>`), so everything
//! belonging to an issue can be pruned before its refreshed data is mapped
//! back in. True unbounded streaming is out of scope — OCEL 2.0 is a static
//! exchange format — so incremental sync is checkpointed re-writes.

use std::collections::{BTreeSet, HashMap};

use ocel::Ocel;
use ocel_etl::StagingLog;

use crate::models::Issue;

/// The issue key an event id belongs to (`"DEMO-1/created"` → `"DEMO-1"`).
fn issue_key_of(event_id: &str) -> &str {
    event_id.split('/').next().unwrap_or(event_id)
}

/// Drop everything belonging to the issues about to be refreshed.
///
/// Objects that were only referenced by the pruned events (the issues' tasks,
/// and e.g. users who only touched them) are dropped too and come back with
/// the refreshed mapping.
#[must_use]
pub fn prune_refreshed(existing: &Ocel, refreshed: &BTreeSet<String>) -> Ocel {
    existing.filter_events(|e| !refreshed.contains(issue_key_of(&e.id)))
}

/// Re-add `parent of` links from refreshed parents to unrefreshed children.
///
/// The link lives on the parent object, which is rebuilt when the parent is
/// refreshed — but only the child's record knows the relation, so children
/// outside the refresh set must contribute it here.
pub fn repair_parent_links(
    staging: &mut StagingLog,
    issues: &[Issue],
    refreshed: &BTreeSet<String>,
) {
    let key_by_id: HashMap<u64, &str> = issues
        .iter()
        .map(|issue| (issue.id, issue.issue_key.as_str()))
        .collect();
    for child in issues {
        if refreshed.contains(&child.issue_key) {
            continue;
        }
        if let Some(parent_key) = child.parent_issue_id.and_then(|id| key_by_id.get(&id)) {
            if refreshed.contains(*parent_key) {
                staging.add_o2o(parent_key, &child.issue_key, "parent of");
            }
        }
    }
}
