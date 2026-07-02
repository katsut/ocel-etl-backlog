//! Backlog API v2 client: pagination and rate-limit backoff.
//!
//! The HTTP layer is abstracted behind [`HttpGet`] so pagination and backoff
//! are fully testable without a network; [`BacklogClient::from_env`] wires in
//! the real `reqwest`-based transport.

use std::thread::sleep;
use std::time::Duration;

use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::models::{Comment, Issue, Project};

/// Page size for list endpoints (the Backlog API maximum).
const PAGE: usize = 100;
/// Give up after this many consecutive rate-limit retries per request.
const MAX_RETRIES: u32 = 5;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("http transport error: {0}")]
    Transport(String),

    #[error("API returned status {status} for {context}")]
    Status { status: u16, context: String },

    #[error("rate limited; gave up after {0} retries")]
    RateLimited(u32),

    #[error("failed to parse response for {context}: {message}")]
    Parse { context: String, message: String },
}

/// A minimal HTTP response for [`HttpGet`] implementations.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    /// Seconds to wait before retrying (from rate-limit headers), if present.
    pub retry_after: Option<u64>,
    pub body: String,
}

/// The transport abstraction: perform a GET against a fully-formed URL.
pub trait HttpGet {
    fn get(&self, url: &str) -> Result<HttpResponse, ClientError>;
}

/// `reqwest`-based transport (blocking, rustls).
#[derive(Debug)]
pub struct ReqwestHttp {
    client: reqwest::blocking::Client,
}

impl ReqwestHttp {
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
        }
    }
}

impl Default for ReqwestHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpGet for ReqwestHttp {
    fn get(&self, url: &str) -> Result<HttpResponse, ClientError> {
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("Retry-After")
            .or_else(|| response.headers().get("X-RateLimit-Reset"))
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| {
                // X-RateLimit-Reset is an epoch timestamp; Retry-After is seconds.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                if v > now {
                    v - now
                } else {
                    v.min(60)
                }
            });
        let body = response
            .text()
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        Ok(HttpResponse {
            status,
            retry_after,
            body,
        })
    }
}

/// Backlog API client over an [`HttpGet`] transport.
#[derive(Debug)]
pub struct BacklogClient<H> {
    http: H,
    base_url: String,
    api_key: String,
}

impl BacklogClient<ReqwestHttp> {
    /// Build a client from `BACKLOG_BASE_URL` (e.g. `https://example.backlog.com`)
    /// and `BACKLOG_API_KEY`.
    pub fn from_env() -> Result<Self, ClientError> {
        let base_url = std::env::var("BACKLOG_BASE_URL")
            .map_err(|_| ClientError::Transport("BACKLOG_BASE_URL is not set".into()))?;
        let api_key = std::env::var("BACKLOG_API_KEY")
            .map_err(|_| ClientError::Transport("BACKLOG_API_KEY is not set".into()))?;
        Ok(Self::new(ReqwestHttp::new(), &base_url, &api_key))
    }
}

impl<H: HttpGet> BacklogClient<H> {
    /// Create a client over an arbitrary transport (tests inject fakes here).
    pub fn new(http: H, base_url: &str, api_key: &str) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
        }
    }

    /// Fetch project metadata by key or id.
    pub fn project(&self, project_id_or_key: &str) -> Result<Project, ClientError> {
        let url = self.url(&format!("/api/v2/projects/{project_id_or_key}"), &[]);
        self.get_json(&url, "project")
    }

    /// Fetch every issue of a project (paginated, creation order).
    pub fn all_issues(&self, project_id: u64) -> Result<Vec<Issue>, ClientError> {
        let mut issues: Vec<Issue> = Vec::new();
        loop {
            let url = self.url(
                "/api/v2/issues",
                &[
                    ("projectId[]", &project_id.to_string()),
                    ("count", &PAGE.to_string()),
                    ("offset", &issues.len().to_string()),
                    ("sort", "created"),
                    ("order", "asc"),
                ],
            );
            let page: Vec<Issue> = self.get_json(&url, "issues")?;
            let full = page.len() == PAGE;
            issues.extend(page);
            if !full {
                return Ok(issues);
            }
        }
    }

    /// Fetch every comment of an issue (paginated by `minId`, oldest first).
    pub fn all_comments(&self, issue_id: u64) -> Result<Vec<Comment>, ClientError> {
        let mut comments: Vec<Comment> = Vec::new();
        let mut min_id: Option<u64> = None;
        loop {
            let count = PAGE.to_string();
            let mut params: Vec<(&str, &str)> = vec![("count", &count), ("order", "asc")];
            let min_id_value = min_id.map(|id| id.to_string());
            if let Some(v) = &min_id_value {
                params.push(("minId", v));
            }
            let url = self.url(&format!("/api/v2/issues/{issue_id}/comments"), &params);
            let page: Vec<Comment> = self.get_json(&url, "comments")?;
            let full = page.len() == PAGE;
            min_id = page.last().map(|c| c.id);
            comments.extend(page);
            if !full {
                return Ok(comments);
            }
        }
    }

    fn url(&self, path: &str, params: &[(&str, &str)]) -> String {
        let mut url = format!("{}{}?apiKey={}", self.base_url, path, self.api_key);
        for (key, value) in params {
            url.push('&');
            url.push_str(key);
            url.push('=');
            url.push_str(value);
        }
        url
    }

    /// GET with rate-limit backoff, parsing the JSON body.
    fn get_json<T: DeserializeOwned>(&self, url: &str, context: &str) -> Result<T, ClientError> {
        let mut retries = 0;
        loop {
            let response = self.http.get(url)?;
            match response.status {
                200 => {
                    return serde_json::from_str(&response.body).map_err(|e| ClientError::Parse {
                        context: context.to_owned(),
                        message: e.to_string(),
                    });
                }
                429 => {
                    retries += 1;
                    if retries > MAX_RETRIES {
                        return Err(ClientError::RateLimited(MAX_RETRIES));
                    }
                    sleep(Duration::from_secs(response.retry_after.unwrap_or(1)));
                }
                status => {
                    return Err(ClientError::Status {
                        status,
                        context: context.to_owned(),
                    });
                }
            }
        }
    }
}
