use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ocel_etl::StagingLog;
use ocel_etl_backlog::client::BacklogClient;
use ocel_etl_backlog::mapper::ProjectMapper;

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
        /// Project key or id; repeat or comma-separate for several
        /// (e.g. --project DEMO --project OPS or --project DEMO,OPS).
        #[arg(long = "project", value_delimiter = ',', required = true)]
        projects: Vec<String>,
        /// Output file (.json/.jsonocel, .sqlite/.db, .xml/.xmlocel).
        #[arg(long)]
        out: PathBuf,
    },
}

fn pull(project_keys: &[String], out: &Path) -> Result<(), Box<dyn Error>> {
    let client = BacklogClient::from_env()?;
    let mut staging = StagingLog::new();

    for key in project_keys {
        let project = client.project(key)?;
        eprintln!("project: {} ({})", project.name, project.project_key);

        // the issue list is buffered (needed to resolve parent links);
        // comments — the bulk of the data — are mapped and dropped per issue
        let issues = client.all_issues(project.id)?;
        eprintln!("issues: {}", issues.len());

        let mapper = ProjectMapper::new(&project, &issues);
        mapper.register(&mut staging);
        for (index, issue) in issues.iter().enumerate() {
            let comments = client.all_comments(issue.id)?;
            mapper.map_issue(&mut staging, issue, &comments);
            if (index + 1) % 100 == 0 {
                eprintln!("  mapped {} issues...", index + 1);
            }
        }
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
        Command::Pull { projects, out } => match pull(&projects, &out) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        },
    }
}
