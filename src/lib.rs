//! Backlog → OCEL 2.0 ETL connector.
//!
//! Extracts a Backlog project's full history (issues + comments, whose
//! `changeLog` entries carry status/assignee/... changes) and maps it into an
//! OCEL 2.0 event log through the [`ocel_etl::StagingLog`] gate.

pub mod client;
pub mod mapper;
pub mod models;
pub mod sync;
