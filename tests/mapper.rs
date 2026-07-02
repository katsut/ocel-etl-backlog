use chrono::DateTime;
use ocel::AttrValue;
use ocel_etl_backlog::mapper::map_project;
use ocel_etl_backlog::models::{Comment, Issue, Project};

fn project() -> Project {
    serde_json::from_str(r#"{"id":1,"projectKey":"DEMO","name":"Demo Project"}"#).unwrap()
}

fn issue() -> Issue {
    serde_json::from_str(include_str!("fixtures/issue.json")).unwrap()
}

fn comments() -> Vec<Comment> {
    serde_json::from_str(include_str!("fixtures/comments.json")).unwrap()
}

#[test]
fn maps_fixture_to_valid_ocel() {
    let ocel = map_project(&project(), &[(issue(), comments())])
        .into_ocel()
        .unwrap();
    assert_eq!(ocel.validate(), Ok(()));

    let mut types: Vec<&str> = ocel.events.iter().map(|e| e.event_type.as_str()).collect();
    types.sort_unstable();
    assert_eq!(
        types,
        vec![
            "assignee_changed",
            "comment_added",
            "status_changed",
            "task_created"
        ]
    );

    let mut object_types: Vec<&str> = ocel.object_types.iter().map(|t| t.name.as_str()).collect();
    object_types.sort_unstable();
    assert_eq!(
        object_types,
        vec!["category", "milestone", "project", "task", "user"]
    );
}

/// The task's dynamic status starts at the reconstructed initial value and
/// changes at the changeLog timestamp.
#[test]
fn reconstructs_dynamic_status() {
    let ocel = map_project(&project(), &[(issue(), comments())])
        .into_ocel()
        .unwrap();
    let task = ocel.objects.iter().find(|o| o.id == "DEMO-1").unwrap();

    let at_creation = DateTime::parse_from_rfc3339("2026-01-05T09:00:00Z")
        .unwrap()
        .to_utc();
    let after_change = DateTime::parse_from_rfc3339("2026-01-07T00:00:00Z")
        .unwrap()
        .to_utc();
    assert_eq!(
        task.attribute_at("status", at_creation),
        Some(&AttrValue::String("Open".into()))
    );
    assert_eq!(
        task.attribute_at("status", after_change),
        Some(&AttrValue::String("In Progress".into()))
    );
}

/// E2O qualifiers follow the mapping design; provenance rides on change events.
#[test]
fn qualifiers_and_provenance() {
    let ocel = map_project(&project(), &[(issue(), comments())])
        .into_ocel()
        .unwrap();

    let created = ocel
        .events
        .iter()
        .find(|e| e.id == "DEMO-1/created")
        .unwrap();
    let quals: Vec<&str> = created
        .relationships
        .iter()
        .map(|r| r.qualifier.as_str())
        .collect();
    assert_eq!(quals, vec!["created task", "creator", "belongs to project"]);

    let change = ocel
        .events
        .iter()
        .find(|e| e.event_type == "status_changed")
        .unwrap();
    assert!(change
        .attributes
        .iter()
        .any(|a| a.name == "_source" && a.value == AttrValue::String("backlog-changelog".into())));
    assert!(change
        .attributes
        .iter()
        .any(|a| a.name == "new_value" && a.value == AttrValue::String("In Progress".into())));
}

/// O2O: project membership, milestone, and category links exist.
#[test]
fn o2o_links() {
    let ocel = map_project(&project(), &[(issue(), comments())])
        .into_ocel()
        .unwrap();
    assert!(ocel
        .o2o()
        .any(|r| r.source_id == "DEMO-1" && r.target_id == "DEMO" && r.qualifier == "belongs to"));
    assert!(ocel
        .o2o()
        .any(|r| r.source_id == "DEMO-1" && r.qualifier == "assigned to"));
    assert!(ocel
        .o2o()
        .any(|r| r.source_id == "DEMO-1" && r.qualifier == "categorized as"));
}
