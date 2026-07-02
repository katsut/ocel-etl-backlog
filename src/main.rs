use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{DateTime, NaiveDate, Utc};
use clap::{Parser, Subcommand};
use ocel_etl::StagingLog;
use ocel_etl_backlog::client::BacklogClient;
use ocel_etl_backlog::mapper::ProjectMapper;
use ocel_etl_backlog::models::{Issue, Project};
use ocel_etl_backlog::sync::{prune_refreshed, repair_parent_links};

/// Backlog → OCEL 2.0 extraction.
#[derive(Debug, Parser)]
#[command(name = "ocel-backlog", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Pull a project's history into an OCEL 2.0 file.
    ///
    /// Reads `BACKLOG_BASE_URL` (e.g. <https://example.backlog.com>) and
    /// `BACKLOG_API_KEY` from the environment. When the output file already
    /// exists, only issues updated since its newest event are refreshed and
    /// merged in (incremental sync).
    Pull {
        /// Project key or id; repeat or comma-separate for several
        /// (e.g. --project DEMO --project OPS or --project DEMO,OPS).
        #[arg(long = "project", value_delimiter = ',', required = true)]
        projects: Vec<String>,
        /// Output file (.json/.jsonocel, .sqlite/.db, .xml/.xmlocel).
        #[arg(long)]
        out: PathBuf,
        /// Only refresh issues updated at or after this time
        /// (RFC 3339 or YYYY-MM-DD). Defaults to the newest event in --out.
        #[arg(long)]
        since: Option<String>,
        /// Ignore any existing --out file and pull everything.
        #[arg(long)]
        full: bool,
    },
}

fn parse_since(s: &str) -> Result<DateTime<Utc>, Box<dyn Error>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.to_utc());
    }
    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d")?;
    Ok(date
        .and_hms_opt(0, 0, 0)
        .expect("midnight is valid")
        .and_utc())
}

fn pull(
    project_keys: &[String],
    out: &Path,
    since_arg: Option<&str>,
    full: bool,
) -> Result<(), Box<dyn Error>> {
    let client = BacklogClient::from_env()?;

    let existing = if !full && out.exists() {
        eprintln!("existing log found: {}", out.display());
        Some(ocel::io::read_path(out)?)
    } else {
        None
    };
    let since: Option<DateTime<Utc>> = match (since_arg, &existing) {
        (Some(s), _) => Some(parse_since(s)?),
        (None, Some(log)) => log.events.iter().map(|e| e.time).max(),
        (None, None) => None,
    };
    if let Some(s) = since {
        eprintln!("incremental: refreshing issues updated at/after {s}");
    }

    // pass 1: issue lists (lightweight) decide what to refresh
    let mut per_project: Vec<(Project, Vec<Issue>)> = Vec::with_capacity(project_keys.len());
    let mut refreshed: BTreeSet<String> = BTreeSet::new();
    for key in project_keys {
        let project = client.project(key)?;
        let issues = client.all_issues(project.id)?;
        eprintln!(
            "project: {} ({}) — {} issues",
            project.name,
            project.project_key,
            issues.len()
        );
        for issue in &issues {
            if since.is_none_or(|s| issue.updated >= s) {
                refreshed.insert(issue.issue_key.clone());
            }
        }
        per_project.push((project, issues));
    }

    // base: the existing log minus everything belonging to refreshed issues
    let mut staging = match &existing {
        Some(log) => StagingLog::from_ocel(prune_refreshed(log, &refreshed)),
        None => StagingLog::new(),
    };

    // pass 2: map refreshed issues, streaming comments per issue
    for (project, issues) in &per_project {
        let mut mapper = ProjectMapper::new(project, issues.iter());
        mapper.register(&mut staging);
        let mut count = 0usize;
        for issue in issues {
            if !refreshed.contains(&issue.issue_key) {
                continue;
            }
            let comments = client.all_comments(issue.id)?;
            mapper.map_issue(&mut staging, issue, &comments);
            count += 1;
            if count.is_multiple_of(100) {
                eprintln!("  mapped {count} refreshed issues...");
            }
        }
        eprintln!("  refreshed {count} issues");
        if !mapper.skipped_fields().is_empty() {
            let summary: Vec<String> = mapper
                .skipped_fields()
                .iter()
                .map(|(field, n)| format!("{field} x{n}"))
                .collect();
            eprintln!("  skipped changeLog fields: {}", summary.join(", "));
        }

        repair_parent_links(&mut staging, issues, &refreshed);
    }

    let log = staging
        .into_ocel()
        .map_err(|violations| format!("staged data is not a valid OCEL log: {violations:?}"))?;
    eprintln!(
        "log: {} events / {} objects",
        log.events.len(),
        log.objects.len()
    );

    ocel::io::write_path(&log, out)?;
    eprintln!("wrote {}", out.display());
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Pull {
            projects,
            out,
            since,
            full,
        } => match pull(&projects, &out, since.as_deref(), full) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        },
    }
}
