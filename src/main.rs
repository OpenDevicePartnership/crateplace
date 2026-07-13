use anstream::println;
use clap::builder::styling::{AnsiColor, Color, Style};
use clap::{Parser, Subcommand};
use crateplace::validation::{ProblemLevel, ValidationProblem};
use crateplace::{
    CratePlacer, CratePlacerError,
    deps::Inverted,
    init::InitError,
    mangling::{ManglingDetectionError, rustc_mangling_version},
    report,
    validation::ValidationError,
};
use std::{
    env,
    iter::Iterator,
    path::{Path, PathBuf},
    str::FromStr,
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
    #[error("Mangling detection")]
    ManglingDetection(
        #[source]
        #[from]
        ManglingDetectionError,
    ),
    #[error("Validation")]
    Validation(
        #[source]
        #[from]
        ValidationError,
    ),
}

#[derive(Copy, Clone, Debug)]
enum ManglingVersion {
    Legacy,
    V0,
    All,
}

#[derive(thiserror::Error, Debug)]
#[error("Unrecognized mangling version")]
struct ManglingParseError;

impl FromStr for ManglingVersion {
    type Err = ManglingParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "legacy" => Ok(Self::Legacy),
            "v0" => Ok(Self::V0),
            "all" => Ok(Self::All),
            _ => Err(ManglingParseError),
        }
    }
}

impl From<ManglingVersion> for crateplace::ManglingMatches {
    fn from(value: ManglingVersion) -> Self {
        match value {
            ManglingVersion::Legacy => crateplace::ManglingMatches::Legacy,
            ManglingVersion::V0 => crateplace::ManglingMatches::V0,
            ManglingVersion::All => crateplace::ManglingMatches::All,
        }
    }
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
        /// Mangling version to use for script generation
        #[arg(short, long)]
        rustc_mangling_version: Option<ManglingVersion>,
        /// Include extra linkerscript before the crateplace script
        #[arg(short, long)]
        before_script: Option<String>,
        /// Include extra linkerscript after the crateplace script
        #[arg(short, long)]
        after_script: Option<String>,
    },
    /// Setup default build.rs and Memory.toml files
    Init,
    /// Determine mangling version of rustc in path
    ManglingVersion {
        /// Rustc path
        #[arg(short, long)]
        rustc: Option<String>,
    },
    /// Validate the output using debug info
    Validate {
        /// Path to binary file
        #[arg(short, long)]
        file: Option<PathBuf>,
        /// Ignore file containing symbol regex
        #[arg(short, long)]
        ignore_file: Option<PathBuf>,
        /// Add misplaced symbols to the ignore file
        #[arg(short, long)]
        bless: bool,
        /// Show ignored symbols
        #[arg(short, long)]
        show_ignored: bool,
    },
}

#[derive(Debug, Clone, Parser)]
struct Commandline {
    /// Cargo.toml file of the target crate
    #[arg(short, long, global = true)]
    manifest_path: Option<PathBuf>,
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
        } => placer.display_tree(
            show_unspecified,
            no_dedupe,
            match invert {
                Some(dep) => Inverted::Inverted(dep),
                None => Inverted::Not,
            },
        )?,
        Command::MakeScript {
            output,
            stdout,
            rustc_mangling_version,
            before_script,
            after_script,
        } => {
            if let Some(output) = &output {
                placer.output(output.as_path());
            }
            if let Some(pre_script) = &before_script {
                placer.pre_script(pre_script);
            }
            if let Some(post_script) = &after_script {
                placer.post_script(post_script);
            }
            placer.stdout(stdout);
            placer.write_linkerscript(rustc_mangling_version.map(Into::into))?
        }
        Command::Init => crateplace::init::init(manifest)?,
        Command::ManglingVersion { rustc } => {
            let flags = std::env::var("RUSTFLAGS").ok();
            let rustflags = flags
                .as_ref()
                .map(|flags| flags.split_whitespace())
                .into_iter()
                .flatten();
            let version = rustc_mangling_version(rustc.as_deref(), rustflags)?;
            println!("{version}");
        }
        Command::Validate {
            file,
            ignore_file,
            bless,
            show_ignored,
        } => {
            if let Some(ignore_file) = ignore_file {
                placer.ignorelist_file(ignore_file);
            }
            let problems = match file {
                Some(binary) => placer.validate(&binary),
                None => placer.build_then_validate(),
            }?;
            if bless {
                placer.bless(&problems)?;
            }
            let error = Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Red)))
                .bold();
            let warning = Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Yellow)))
                .bold();
            let ignored = Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Blue)))
                .bold();
            let mut problem_count = 0;
            let mut custom_symbol_error = false;
            for problem in &problems {
                if let ValidationProblem::SymbolAssignment { owner, .. } = problem
                    && owner.contains("custom")
                {
                    custom_symbol_error = true;
                }

                let prep = match problem.problem_level() {
                    ProblemLevel::Error => {
                        problem_count += 1;
                        format!("{error}Error:{error:#}")
                    }
                    ProblemLevel::Warning => {
                        problem_count += 1;
                        format!("{warning}Warning:{warning:#}")
                    }
                    ProblemLevel::Ignored => {
                        if !show_ignored {
                            continue;
                        }
                        format!("{ignored}Ignored:{ignored:#}")
                    }
                };

                println!("{prep} {problem}");
            }
            if custom_symbol_error {
                println!(
                    "{warning}NOTE:{warning:#} custom symbol assignments on non-Rust symbols need to be compiled with \"-ffunction-sections -fdata-sections\" or crateplace will not be able to control their placement."
                );
            }
            if problem_count != 0 {
                std::process::exit(2);
            }
        }
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
        args.manifest_path.as_deref(),
        args.config.as_deref(),
        args.command,
    ) {
        report(&err);
        std::process::exit(1);
    }
}
