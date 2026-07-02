use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ocel_etl_backlog::client::BacklogClient;
use ocel_etl_backlog::mapper::map_project;

/// Backlog → OCEL 2.0 extraction.
#[derive(Debug, Parser)]
#[command(name = "ocel-backlog", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Pull a project's full history into an OCEL 2.0 file.
    ///
    /// Reads `BACKLOG_BASE_URL` (e.g. <https://example.backlog.com>) and
    /// `BACKLOG_API_KEY` from the environment.
    Pull {
        /// Project key or id (e.g. DEMO).
        #[arg(long)]
        project: String,
        /// Output file (.json/.jsonocel, .sqlite/.db, .xml/.xmlocel).
        #[arg(long)]
        out: PathBuf,
    },
}

fn pull(project_key: &str, out: &Path) -> Result<(), Box<dyn Error>> {
    let client = BacklogClient::from_env()?;
    let project = client.project(project_key)?;
    eprintln!("project: {} ({})", project.name, project.project_key);

    let issues = client.all_issues(project.id)?;
    eprintln!("issues: {}", issues.len());

    let mut with_comments = Vec::with_capacity(issues.len());
    for (index, issue) in issues.into_iter().enumerate() {
        let comments = client.all_comments(issue.id)?;
        if (index + 1) % 100 == 0 {
            eprintln!("  fetched comments for {} issues...", index + 1);
        }
        with_comments.push((issue, comments));
    }

    let staging = map_project(&project, &with_comments);
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
        Command::Pull { project, out } => match pull(&project, &out) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        },
    }
}
