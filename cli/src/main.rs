mod release;
mod workflows;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(version, about = "Configure an Android repository")]
struct Cli {
    #[arg(long, default_value = ".")]
    project: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate release artifacts against their expected source and destination.
    #[command(name = "module_release", alias = "module-release")]
    ModuleRelease {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
    /// Preview or generate the managed GitHub workflows.
    Workflows {
        #[arg(long, conflicts_with_all = ["overwrite", "only_missing"])]
        check: bool,
        #[arg(long, conflicts_with = "only_missing")]
        overwrite: bool,
        #[arg(long)]
        only_missing: bool,
    },
}

#[derive(Subcommand)]
enum ReleaseCommand {
    /// Verify locally assembled publication artifacts and print a candidate release record.
    CheckLocal {
        #[arg(long)]
        module: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        source: String,
    },
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Command::ModuleRelease {
            command:
                ReleaseCommand::CheckLocal {
                    module,
                    version,
                    source,
                },
        } => {
            let destination =
                std::env::var("RELEASE_REPOSITORY").unwrap_or_else(|_| "maven-central".into());
            let run_id = std::env::var("GITHUB_RUN_ID").unwrap_or_else(|_| "local".into());
            let record = release::check_local(
                &cli.project,
                &module,
                &version,
                &source,
                &destination,
                &run_id,
            )?;
            println!("{}", serde_json::to_string_pretty(&record)?);
            Ok(ExitCode::SUCCESS)
        }
        Command::Workflows {
            check,
            overwrite,
            only_missing,
        } => {
            let mode = match (check, overwrite, only_missing) {
                (true, false, false) => workflows::Mode::Check,
                (false, true, false) => workflows::Mode::Overwrite,
                (false, false, true) => workflows::Mode::OnlyMissing,
                (false, false, false) => workflows::Mode::Generate,
                _ => bail!("Select one workflow operation"),
            };
            let drift = workflows::update(&cli.project, mode)?;
            Ok(if check && drift {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
    }
}

fn main() -> ExitCode {
    run(Cli::parse()).unwrap_or_else(|error| {
        eprintln!("error: {error:#}");
        ExitCode::FAILURE
    })
}
