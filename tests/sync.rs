use std::collections::BTreeSet;

use ocel_etl::StagingLog;
use ocel_etl_backlog::mapper::{map_project, ProjectMapper};
use ocel_etl_backlog::models::{Comment, Issue, Project};
use ocel_etl_backlog::sync::{prune_refreshed, repair_parent_links};

fn project() -> Project {
    serde_json::from_str(r#"{"id":1,"projectKey":"DEMO","name":"Demo Project"}"#).unwrap()
}

fn issue() -> Issue {
    serde_json::from_str(include_str!("fixtures/issue.json")).unwrap()
}

fn comments_v1() -> Vec<Comment> {
    serde_json::from_str(include_str!("fixtures/comments.json")).unwrap()
}

/// v2 = v1 plus a later status change to Closed.
fn comments_v2() -> Vec<Comment> {
    let mut comments = comments_v1();
    comments.push(
        serde_json::from_str(
            r#"{
              "id": 503,
              "content": null,
              "changeLog": [
                { "field": "status", "newValue": "Closed", "originalValue": "In Progress" }
              ],
              "createdUser": { "id": 11, "name": "Alice" },
              "created": "2026-01-09T18:00:00Z"
            }"#,
        )
        .unwrap(),
    );
    comments
}

fn refreshed(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| (*k).to_owned()).collect()
}

/// Pruning removes exactly the refreshed issue's events and orphaned objects.
#[test]
fn prune_drops_only_the_refreshed_issue() {
    let mut second = issue();
    second.id = 202;
    second.issue_key = "DEMO-2".into();

    let log = map_project(&project(), &[(issue(), comments_v1()), (second, vec![])])
        .into_ocel()
        .unwrap();

    let pruned = prune_refreshed(&log, &refreshed(&["DEMO-1"]));
    assert_eq!(pruned.validate(), Ok(()));
    assert!(!pruned.events.iter().any(|e| e.id.starts_with("DEMO-1/")));
    assert!(pruned.events.iter().any(|e| e.id == "DEMO-2/created"));
    assert!(!pruned.objects.iter().any(|o| o.id == "DEMO-1"));
}

/// The incremental path (prune + re-map the updated issue) produces exactly
/// the log a full re-pull would.
#[test]
fn incremental_merge_equals_full_pull() {
    let v1 = map_project(&project(), &[(issue(), comments_v1())])
        .into_ocel()
        .unwrap();

    // incremental: prune DEMO-1 from v1, then map its refreshed data (v2)
    let p = project();
    let issues = vec![issue()];
    let mut staging = StagingLog::from_ocel(prune_refreshed(&v1, &refreshed(&["DEMO-1"])));
    let mut mapper = ProjectMapper::new(&p, &issues);
    mapper.register(&mut staging);
    mapper.map_issue(&mut staging, &issues[0], &comments_v2());
    let incremental = staging.into_ocel().unwrap();

    let full = map_project(&project(), &[(issue(), comments_v2())])
        .into_ocel()
        .unwrap();

    assert_eq!(incremental, full);
    assert!(incremental
        .events
        .iter()
        .any(|e| e.id == "DEMO-1/change/503/0"));
}

/// An unrefreshed child keeps its parent link when only the parent refreshes.
#[test]
fn parent_link_survives_parent_refresh() {
    let parent = issue(); // DEMO-1, id 101
    let mut child = issue();
    child.id = 202;
    child.issue_key = "DEMO-2".into();
    child.parent_issue_id = Some(101);

    let v1 = map_project(
        &project(),
        &[(parent.clone(), comments_v1()), (child.clone(), vec![])],
    )
    .into_ocel()
    .unwrap();
    assert!(v1
        .o2o()
        .any(|r| r.source_id == "DEMO-1" && r.target_id == "DEMO-2" && r.qualifier == "parent of"));

    // refresh only the parent
    let p = project();
    let issues = vec![parent.clone(), child.clone()];
    let only_parent = refreshed(&["DEMO-1"]);
    let mut staging = StagingLog::from_ocel(prune_refreshed(&v1, &only_parent));
    let mut mapper = ProjectMapper::new(&p, &issues);
    mapper.register(&mut staging);
    mapper.map_issue(&mut staging, &parent, &comments_v2());
    repair_parent_links(&mut staging, &issues, &only_parent);
    let merged = staging.into_ocel().unwrap();

    assert_eq!(merged.validate(), Ok(()));
    assert!(merged
        .o2o()
        .any(|r| r.source_id == "DEMO-1" && r.target_id == "DEMO-2" && r.qualifier == "parent of"));
    assert!(merged.events.iter().any(|e| e.id == "DEMO-2/created"));
}
