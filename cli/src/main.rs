macro_rules! setup_message { ($($arg:tt)*) => { crate::ui::message(format!($($arg)*)) }; }

mod confirmation;
mod docs;
mod github;
mod gpg;
mod jitpack;
mod journal;
mod publishing;
mod release;
mod release_identity;
mod setup;
mod signing;
mod ui;
mod vault;
mod workflows;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(version, about = "Configure and release an Android repository")]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    project: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Select setup operations interactively.
    Setup,
    /// Create or verify the APK key and configure its GitHub environment.
    Signing(setup::SigningOptions),
    /// Preview or generate the managed GitHub workflows.
    Workflows {
        #[arg(long, conflicts_with_all = ["overwrite", "only_missing"])]
        check: bool,
        #[arg(long, conflicts_with = "only_missing")]
        overwrite: bool,
        #[arg(long)]
        only_missing: bool,
        #[arg(long, requires = "overwrite")]
        set_tooling_ref: Option<String>,
    },
    /// Coordinate or verify Android releases.
    #[command(name = "module_release", alias = "module-release")]
    ModuleRelease {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
    #[command(name = "publishing_config", alias = "publishing-config")]
    PublishingConfig {
        #[command(subcommand)]
        command: PublishingCommand,
    },
    #[command(
        name = "setup_github_environments",
        alias = "setup-github-environments"
    )]
    SetupGithubEnvironments {
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    #[command(name = "jitpack_build", alias = "jitpack-build")]
    JitpackBuild,
    #[command(name = "import_docs", alias = "import-docs")]
    ImportDocs {
        #[arg(long)]
        check: bool,
    },
    #[command(name = "documentation_history", alias = "documentation-history")]
    DocumentationHistory {
        #[command(subcommand)]
        command: HistoryCommand,
    },
    #[command(name = "build_docs", alias = "build-docs")]
    BuildDocs {
        #[arg(long)]
        base: Option<String>,
        #[arg(long, help = "Verify that the site builds successfully")]
        check: bool,
    },
}

#[derive(Subcommand)]
enum PublishingCommand {
    Generate,
    Check,
    Resolve {
        #[arg(long)]
        repository: Option<String>,
    },
    Preflight {
        #[arg(long)]
        repository: Option<String>,
    },
}

#[derive(Subcommand)]
enum HistoryCommand {
    Seed,
    Verify,
    Export,
}

#[derive(Args)]
struct ReleaseTarget {
    #[arg(long)]
    module: String,
    #[arg(long)]
    version: String,
}

#[derive(Subcommand)]
enum ReleaseCommand {
    Generate,
    Verify,
    Prepare {
        #[arg(long)]
        module: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        allow_pending_jitpack: bool,
        #[arg(long)]
        skip_central_version_check: bool,
    },
    CheckLocal {
        #[command(flatten)]
        target: ReleaseTarget,
        #[arg(long)]
        source: String,
    },
    Reserve {
        #[command(flatten)]
        target: ReleaseTarget,
        #[arg(long)]
        source: String,
        #[arg(long)]
        allow_pending_jitpack: bool,
    },
    Guard(ReleaseTarget),
    PrepareFinalization {
        #[command(flatten)]
        target: ReleaseTarget,
        #[arg(long)]
        source: Option<String>,
    },
    Confirm {
        #[command(flatten)]
        target: ReleaseTarget,
        #[arg(long)]
        source: Option<String>,
        #[arg(long, default_value_t = 2400)]
        timeout: u64,
    },
    Finalize {
        #[command(flatten)]
        target: ReleaseTarget,
        #[arg(long)]
        source: String,
    },
    AbortUnconfirmed(ReleaseTarget),
    /// Review failed JitPack journals and optionally archive proven failures.
    ReconcileFailedJitpack {
        #[arg(long)]
        module: String,
        /// Limit the review to these versions. Repeat for multiple versions.
        #[arg(long = "version")]
        versions: Vec<String>,
        /// Atomically archive eligible journals and remove their pending refs.
        #[arg(long)]
        apply: bool,
    },
}

fn release_destination() -> String {
    std::env::var("RELEASE_REPOSITORY").unwrap_or_else(|_| "maven-central".into())
}

fn run_release(root: &Path, command: ReleaseCommand) -> Result<()> {
    let destination = release_destination();
    let dest = &destination;
    let run_id = std::env::var("GITHUB_RUN_ID").unwrap_or_else(|_| "local".into());
    match command {
        ReleaseCommand::Generate => docs::documentation(root, false),
        ReleaseCommand::Verify => docs::documentation(root, true),
        ReleaseCommand::Prepare {
            module,
            version,
            allow_pending_jitpack,
            skip_central_version_check,
        } => journal::prepare(
            root,
            &module,
            version.as_deref().filter(|value| !value.is_empty()),
            dest,
            allow_pending_jitpack,
            skip_central_version_check,
        ),
        ReleaseCommand::CheckLocal { target, source } => {
            let record = release::check_local(
                root,
                &target.module,
                &target.version,
                &source,
                dest,
                &run_id,
            )?;
            println!("{}", serde_json::to_string_pretty(&record)?);
            Ok(())
        }
        ReleaseCommand::Reserve {
            target,
            source,
            allow_pending_jitpack,
        } => journal::reserve(
            root,
            &target.module,
            &target.version,
            &source,
            dest,
            &run_id,
            allow_pending_jitpack,
        ),
        ReleaseCommand::Guard(target) => {
            journal::guard(root, &target.module, &target.version, dest)
        }
        ReleaseCommand::PrepareFinalization { target, source } => journal::prepare_finalization(
            root,
            &target.module,
            &target.version,
            source.as_deref().filter(|value| !value.is_empty()),
            dest,
        ),
        ReleaseCommand::Confirm {
            target,
            source,
            timeout,
        } => confirmation::confirm(
            root,
            &target.module,
            &target.version,
            dest,
            source.as_deref().filter(|value| !value.is_empty()),
            Duration::from_secs(timeout),
        ),
        ReleaseCommand::Finalize { target, source } => {
            journal::finalize(root, &target.module, &target.version, &source, dest)
        }
        ReleaseCommand::ReconcileFailedJitpack {
            module,
            versions,
            apply,
        } => journal::reconcile_failed_jitpack(root, &module, &versions, apply),
        ReleaseCommand::AbortUnconfirmed(target) => {
            journal::abort_unconfirmed(root, &target.module, &target.version, dest)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let root = cli.project.canonicalize()?;
    match cli.command.unwrap_or(Command::Setup) {
        Command::Setup => ui::run(|| setup::run(&root))?,
        Command::Signing(options) => ui::run(|| setup::sign(&root, options))?,
        Command::ModuleRelease { command } => run_release(&root, command)?,
        Command::Workflows {
            check,
            overwrite,
            only_missing,
            set_tooling_ref,
        } => {
            let mode = if check {
                workflows::Mode::Check
            } else if overwrite {
                workflows::Mode::Overwrite
            } else if only_missing {
                workflows::Mode::OnlyMissing
            } else {
                workflows::Mode::Generate
            };
            let drift = workflows::update_with_ref(&root, mode, set_tooling_ref.as_deref())?;
            if check && drift {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::PublishingConfig { command } => match command {
            PublishingCommand::Generate => publishing::generate(&root, false)?,
            PublishingCommand::Check => publishing::generate(&root, true)?,
            PublishingCommand::Resolve { repository } => {
                publishing::resolve(&root, &repository.unwrap_or_else(release_destination))?
            }
            PublishingCommand::Preflight { repository } => {
                publishing::preflight(&root, &repository.unwrap_or_else(release_destination))?
            }
        },
        Command::SetupGithubEnvironments { repo, config } => {
            ui::run(|| publishing::setup(&root, repo.as_deref(), config.as_deref()))?
        }
        Command::JitpackBuild => jitpack::build(&root)?,
        Command::ImportDocs { check } => docs::documentation(&root, check)?,
        Command::DocumentationHistory { command } => {
            let command = match command {
                HistoryCommand::Seed => "seed",
                HistoryCommand::Verify => "verify",
                HistoryCommand::Export => "export",
            };
            docs::history(&root, command)?;
        }
        Command::BuildDocs { base, check } => docs::build(&root, base.as_deref(), check)?,
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    run(Cli::parse()).unwrap_or_else(|error| {
        eprintln!("error: {error:#}");
        ExitCode::FAILURE
    })
}
