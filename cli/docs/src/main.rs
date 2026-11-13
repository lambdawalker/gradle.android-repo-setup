use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "android-repo-docs",
    version,
    about = "Standalone Android repository documentation"
)]
struct Args {
    #[arg(long, global = true, default_value = ".")]
    project: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Generate or verify pinned repository launchers.
    Bootstrap {
        #[command(subcommand)]
        command: Bootstrap,
    },
    /// Initialize independent documentation metadata from existing setup metadata.
    Init {
        #[arg(long)]
        repository: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        tooling_ref: Option<String>,
        #[arg(long)]
        source_root: Vec<String>,
    },
    Media {
        action: String,
    },
    Screenshots {
        action: String,
        directory: Option<String>,
        #[arg(long)]
        module: Option<String>,
    },
    Sources {
        #[command(subcommand)]
        command: Sources,
    },
    /// Verify generated documentation without writing or running site commands.
    Check,
    /// Update generated documentation without changing release confirmations or history.
    Update,
    /// Generate the installation guide from confirmed releases.
    Imports {
        #[arg(long)]
        check: bool,
    },
    /// Validate, explicitly seed, or export immutable release documentation history.
    History {
        #[command(subcommand)]
        command: History,
    },
    /// Build the local documentation site; this does not publish it.
    Build {
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        check: bool,
    },
    /// Preview the documentation locally on loopback.
    Serve {
        #[arg(long)]
        base: Option<String>,
        #[arg(long, default_value_t = 1111)]
        port: u16,
    },
}
#[derive(Subcommand)]
enum Sources {
    Init {
        #[arg(long)]
        source_root: Vec<String>,
    },
    Check,
    Update,
}
#[derive(Subcommand)]
enum Bootstrap {
    Generate {
        #[arg(long)]
        overwrite: bool,
    },
    Check,
}
#[derive(Subcommand)]
enum History {
    Seed,
    Verify,
    Export,
}
fn main() -> Result<()> {
    let args = Args::parse();
    let root = args
        .project
        .canonicalize()
        .context("Project directory does not exist")?;
    match args.command {
        Command::Media { action } => android_repo_docs::media::media(&root, &action),
        Command::Screenshots {
            action,
            directory,
            module,
        } => android_repo_docs::media::screenshots(
            &root,
            &action,
            module.as_deref().or(directory.as_deref()),
        ),
        Command::Bootstrap { command } => match command {
            Bootstrap::Generate { overwrite } => {
                android_repo_docs::bootstrap::run(&root, false, overwrite)
            }
            Bootstrap::Check => android_repo_docs::bootstrap::run(&root, true, false),
        },
        Command::Sources { command } => match command {
            Sources::Init { source_root } => {
                android_repo_docs::sources::init_with_roots(&root, &source_root)
            }
            Sources::Check => android_repo_docs::sources::check(&root),
            Sources::Update => android_repo_docs::sources::update(&root),
        },
        Command::Init {
            repository,
            title,
            tooling_ref,
            source_root,
        } => android_repo_docs::init_with(
            &root,
            repository.as_deref(),
            title.as_deref(),
            tooling_ref.as_deref(),
            &source_root,
        ),
        Command::Check => android_repo_docs::check(&root),
        Command::Update => android_repo_docs::update(&root),
        Command::Imports { check } => android_repo_docs::documentation(&root, check),
        Command::History { command } => android_repo_docs::history(
            &root,
            match command {
                History::Seed => "seed",
                History::Verify => "verify",
                History::Export => "export",
            },
        ),
        Command::Build { base, check } => android_repo_docs::build(&root, base.as_deref(), check),
        Command::Serve { base, port } => android_repo_docs::serve(&root, base.as_deref(), port),
    }
}
