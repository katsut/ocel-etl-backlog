use std::cell::RefCell;
use std::collections::VecDeque;

use ocel_etl_backlog::client::{BacklogClient, ClientError, HttpGet, HttpResponse};
use ocel_etl_backlog::models::{Comment, Issue};

/// Scripted transport: pops pre-loaded responses, records requested URLs.
struct FakeHttp {
    responses: RefCell<VecDeque<HttpResponse>>,
    urls: RefCell<Vec<String>>,
}

impl FakeHttp {
    fn new(responses: Vec<HttpResponse>) -> Self {
        Self {
            responses: RefCell::new(responses.into()),
            urls: RefCell::new(Vec::new()),
        }
    }
}

impl HttpGet for &FakeHttp {
    fn get(&self, url: &str) -> Result<HttpResponse, ClientError> {
        self.urls.borrow_mut().push(url.to_owned());
        self.responses
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| ClientError::Transport("no scripted response left".into()))
    }
}

fn ok(body: String) -> HttpResponse {
    HttpResponse {
        status: 200,
        retry_after: None,
        body,
    }
}

fn issue_json(id: u64) -> String {
    format!(
        r#"{{"id":{id},"issueKey":"DEMO-{id}",
            "summary":"issue {id}",
            "issueType":{{"id":2,"name":"Task"}},
            "status":{{"id":1,"name":"Open"}},
            "priority":{{"id":3,"name":"Normal"}},
            "assignee":null,"category":[],"milestone":[],"parentIssueId":null,
            "createdUser":{{"id":12,"name":"Bob"}},
            "created":"2026-01-05T09:00:00Z","dueDate":null}}"#
    )
}

fn page_of_issues(ids: std::ops::Range<u64>) -> String {
    let items: Vec<String> = ids.map(issue_json).collect();
    format!("[{}]", items.join(","))
}

/// Realistic single-issue payload (full Backlog shape) parses into the model.
#[test]
fn parses_full_issue_fixture() {
    let body = include_str!("fixtures/issue.json");
    let issue: Issue = serde_json::from_str(body).unwrap();
    assert_eq!(issue.issue_key, "DEMO-1");
    assert_eq!(issue.status.name, "Open");
    assert_eq!(issue.assignee.unwrap().name, "Alice");
    assert_eq!(issue.milestone[0].name, "v1.0");
    assert_eq!(issue.category[0].name, "infra");
    assert!(issue.due_date.is_some());
}

/// Comments fixture: plain comment + pure change record with changeLog.
#[test]
fn parses_comments_with_change_log() {
    let body = include_str!("fixtures/comments.json");
    let comments: Vec<Comment> = serde_json::from_str(body).unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(
        comments[0].content.as_deref(),
        Some("Looks good, starting on this now.")
    );
    assert!(comments[0].change_log.is_empty());
    assert!(comments[1].content.is_none());
    let change = &comments[1].change_log[0];
    assert_eq!(change.field, "status");
    assert_eq!(change.new_value.as_deref(), Some("In Progress"));
    assert_eq!(change.original_value.as_deref(), Some("Open"));
}

/// Issue pagination: a full page (100) triggers the next offset; a short page stops.
#[test]
fn paginates_issues_by_offset() {
    let fake = FakeHttp::new(vec![
        ok(page_of_issues(0..100)),
        ok(page_of_issues(100..130)),
    ]);
    let client = BacklogClient::new(&fake, "https://example.backlog.com", "KEY");

    let issues = client.all_issues(7).unwrap();
    assert_eq!(issues.len(), 130);

    let urls = fake.urls.borrow();
    assert!(urls[0].contains("offset=0"));
    assert!(urls[1].contains("offset=100"));
    assert!(urls[0].contains("projectId[]=7"));
    assert!(urls[0].starts_with("https://example.backlog.com/api/v2/issues?apiKey=KEY"));
}

/// Comment pagination advances the minId cursor.
#[test]
fn paginates_comments_by_min_id() {
    let full: Vec<String> = (1..=100)
        .map(|id| {
            format!(
                r#"{{"id":{id},"content":"c","changeLog":[],
                    "createdUser":{{"id":1,"name":"Alice"}},
                    "created":"2026-01-06T09:15:00Z"}}"#
            )
        })
        .collect();
    let fake = FakeHttp::new(vec![
        ok(format!("[{}]", full.join(","))),
        ok("[]".to_owned()),
    ]);
    let client = BacklogClient::new(&fake, "https://example.backlog.com", "KEY");

    let comments = client.all_comments(101).unwrap();
    assert_eq!(comments.len(), 100);

    let urls = fake.urls.borrow();
    assert!(!urls[0].contains("minId"));
    assert!(urls[1].contains("minId=100"));
}

/// A 429 is retried after the advertised delay, then succeeds.
#[test]
fn retries_on_rate_limit() {
    let fake = FakeHttp::new(vec![
        HttpResponse {
            status: 429,
            retry_after: Some(0),
            body: String::new(),
        },
        ok(page_of_issues(0..2)),
    ]);
    let client = BacklogClient::new(&fake, "https://example.backlog.com", "KEY");

    let issues = client.all_issues(7).unwrap();
    assert_eq!(issues.len(), 2);
    assert_eq!(fake.urls.borrow().len(), 2);
}

/// Non-200/429 statuses surface as errors with context.
#[test]
fn error_status_is_reported() {
    let fake = FakeHttp::new(vec![HttpResponse {
        status: 404,
        retry_after: None,
        body: String::new(),
    }]);
    let client = BacklogClient::new(&fake, "https://example.backlog.com", "KEY");

    let err = client.project("MISSING").unwrap_err();
    assert!(matches!(err, ClientError::Status { status: 404, .. }));
}
