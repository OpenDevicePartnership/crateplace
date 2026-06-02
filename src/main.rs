use clap::{Parser, Subcommand};
use crateplace::{CratePlacer, CratePlacerError, deps::Inverted, init::InitError, report};
use std::{
    env,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Error, Debug)]
enum CommandlineError {
    #[error("Crateplace")]
    CratePlacer(
        #[source]
        #[from]
        CratePlacerError,
    ),
    #[error("Init")]
    InitError(
        #[source]
        #[from]
        InitError,
    ),
}

#[derive(Subcommand, Debug, Clone)]
enum Command {
    /// Display the dependency tree with section assignments
    Tree {
        /// Show crates without section assignments
        #[arg(short, long)]
        show_unspecified: bool,
        /// Expand every occurence of a crates dependencies
        #[arg(short, long)]
        no_dedupe: bool,
        /// Show a tree from a specific dependency to its dependents
        #[arg(short, long)]
        invert: Option<String>,
    },
    /// Make the buildscript
    MakeScript {
        /// Output buildscript file (memory.x)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Print to screen instead of making output file
        #[arg(short, long)]
        stdout: bool,
    },
    /// Setup default build.rs and Memory.toml files
    Init,
}

#[derive(Debug, Clone, Parser)]
struct Commandline {
    /// Cargo.toml file of the target crate
    #[arg(short, long, global = true)]
    manifest: Option<PathBuf>,
    /// Config file to use, default: `Memory.toml`
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

fn perform_command(
    manifest: Option<&Path>,
    config_file: Option<&Path>,
    command: Command,
) -> Result<(), CommandlineError> {
    let mut placer = CratePlacer::new();
    if let Some(manifest) = manifest {
        placer.cargo_manifest(manifest);
    };
    if let Some(config_file) = config_file {
        placer.config_file(config_file);
    }
    match command {
        Command::Tree {
            show_unspecified,
            no_dedupe,
            invert,
        } => {
            placer.display_tree(
                show_unspecified,
                no_dedupe,
                match invert {
                    Some(dep) => Inverted::Inverted(dep),
                    None => Inverted::Not,
                },
            )?;
        }
        Command::MakeScript { output, stdout } => {
            if let Some(output) = &output {
                placer.output(output.as_path());
            }
            placer.stdout(stdout);
            placer.write_linkerscript()?
        }
        Command::Init => crateplace::init::init(manifest)?,
    }
    Ok(())
}

fn main() {
    env_logger::init();
    let mut args: Vec<String> = env::args().collect();
    if args.get(1).map(|arg| arg.as_str()) == Some("crateplace") {
        args.remove(1);
    }
    let args = Commandline::parse_from(args);
    if let Err(err) = perform_command(
        args.manifest.as_deref(),
        args.config.as_deref(),
        args.command,
    ) {
        report(&err);
    }
}
