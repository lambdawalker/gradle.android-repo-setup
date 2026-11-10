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

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
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
