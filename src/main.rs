use anstream::println;
use clap::builder::styling::{AnsiColor, Color, Style};
use clap::{Parser, Subcommand};
use crateplace::config::ByteUnit;
use crateplace::file_error::{FileError, IOToFileResult};
use crateplace::init::{
    MemoryX, MemoryXParseError, generate_memory_toml, init_cargo_toml, init_existing_build_rs,
    init_new_build_rs,
};
use crateplace::validation::{ProblemLevel, ValidationProblem};
use crateplace::{
    CratePlacer, CratePlacerError,
    deps::Inverted,
    init::InitError,
    mangling::{ManglingDetectionError, rustc_mangling_version},
    report,
    validation::ValidationError,
};
use crateplace::{DEFAULT_CONFIG_NAME, DEFAULT_IGNORELIST_NAME, look_up};
use std::fs::File;
use std::io::Write;
use std::{
    env,
    iter::Iterator,
    path::{Path, PathBuf},
    str::FromStr,
};
use thiserror::Error;

static ERR: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Red)))
    .bold();
static WARN: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Yellow)))
    .bold();
static IGN: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Blue)))
    .bold();

#[derive(Error, Debug)]
enum CommandlineError {
    #[error("crateplace")]
    CratePlacer(
        #[source]
        #[from]
        CratePlacerError,
    ),
    #[error("init")]
    InitError(
        #[source]
        #[from]
        InitError,
    ),
    #[error("mangling detection")]
    ManglingDetection(
        #[source]
        #[from]
        ManglingDetectionError,
    ),
    #[error("validation")]
    Validation(
        #[source]
        #[from]
        ValidationError,
    ),
    #[error("init")]
    Init(
        #[from]
        #[source]
        CmdInitError,
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
enum Add {
    /// Add a section to the config file
    Section {
        /// The section name
        #[arg(short, long)]
        name: String,
        /// Flash origin of the section
        #[arg(short, long)]
        origin: ByteUnit,
        /// Length of the section
        #[arg(short, long)]
        length: ByteUnit,
        #[arg(short, long)]
        /// Priority during assignment
        priority: u32,
        /// Default section when unassigned
        #[arg(short, long)]
        default: bool,
    },
    /// Add a crate to the config file
    Crate {
        /// The crate name
        #[arg(short, long)]
        name: String,
        /// The assigned section
        #[arg(short, long)]
        section: String,
        /// Do not make dependencies inherit this assignment
        #[arg(short('d'), long)]
        nodeps: bool,
    },
    /// Add symbol globs to place specific symbols in flash
    Symbol {
        #[arg(short, long)]
        /// The glob pattern to match the symbols
        pattern: String,
        #[arg(short, long)]
        /// The section the symbols should be placed in
        section: String,
        #[arg(short('t'), long)]
        /// Do not match symbols from the text section
        notext: bool,
        #[arg(short('r'), long)]
        /// Do not match symbols from the rodata section
        norodata: bool,
        #[arg(short('d'), long)]
        /// Do not match symbols from the datarel section
        nodatarel: bool,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum Remove {
    /// Remove a section from the config file
    Section {
        /// The section name
        #[arg(short, long)]
        name: String,
    },
    /// Remove a crate from the config file
    Crate {
        /// The crate name
        #[arg(short, long)]
        name: String,
    },
    /// Remove symbol pattern from the config file
    Symbol {
        /// The crate name
        #[arg(short, long)]
        pattern: String,
    },
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
    /// Validate the configuration file
    ValidateConfig,
    /// Add to the config file
    #[command(subcommand)]
    Add(Add),
    /// Remove from the config file
    #[command(subcommand)]
    Remove(Remove),
    /// Set the origin and length of ram in the config file
    SetRam {
        /// Flash origin of the ram
        #[arg(short, long)]
        origin: ByteUnit,
        /// Flash length of the ram
        #[arg(short, long)]
        length: ByteUnit,
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

#[derive(Debug, thiserror::Error)]
pub enum CmdInitError {
    #[error("invalid manifest path")]
    InvalidManifestPath,
    #[error("failed to find manifest")]
    ManifestNotFound,
    #[error("failed to modify \"build.rs\"")]
    FailedToModifyBuildRs,
    #[error("failed to parse Cargo.toml")]
    TomlError(
        #[source]
        #[from]
        toml_edit::TomlError,
    ),
    #[error("build dependencies not a table")]
    DepsNotTableError,
    #[error("\"Memory.toml\" already exists. Is this project already initialized?")]
    AlreadyInitialized,
    #[error("failed to find \"Cargo.toml\"")]
    NoCargoToml,
    #[error("init error")]
    InitError(
        #[source]
        #[from]
        InitError,
    ),
    #[error("memory.x parse error")]
    MemoryXParseError(
        #[source]
        #[from]
        MemoryXParseError,
    ),
    #[error("file error")]
    IOError(
        #[source]
        #[from]
        FileError,
    ),
}

fn get_memory_x(project_path: &Path) -> Result<MemoryX, CmdInitError> {
    let memory_x_path = project_path.join("memory.x");
    if memory_x_path.exists() {
        let memoryx_content =
            &std::fs::read_to_string(&memory_x_path).into_in_result(&memory_x_path)?;
        if memoryx_content.contains("### Generated by crateplace") {
            println!(
                "{WARN}Warning{WARN:#}: found \"memory.x\" generated by crateplace. Using default \"Memory.toml\" content. Make sure to input the correct origin and length for ram and flash."
            );
            Ok(MemoryX::default())
        } else {
            println!("Creating backup of \"memory.x\": \".bck_memory.x\"");
            let backup_path = project_path.join(".bck_memory.x");
            std::fs::rename(memory_x_path, &backup_path).into_out_result(&backup_path)?;
            Ok(MemoryX::from_str(memoryx_content)?)
        }
    } else {
        println!(
            "{WARN}Warning{WARN:#}: did not find a \"memory.x\". Using default \"Memory.toml\" content. Make sure to input the correct origin and length for ram and flash."
        );
        Ok(MemoryX::default())
    }
}

fn backup_if_exists(project_path: &Path, file_name: &str) -> Result<(), CmdInitError> {
    let ignorelist = project_path.join(file_name);
    if ignorelist.exists() {
        let backup_name = ".bck_".to_string()
            + if file_name.starts_with('.') {
                file_name.get(1..).unwrap_or(file_name)
            } else {
                file_name
            };
        println!(
            "{WARN}Warning{WARN:#}: \"{file_name}\" already exists. Creating backup: \"{backup_name}\""
        );
        let out_path = project_path.join(backup_name);
        std::fs::rename(file_name, &out_path).into_out_result(&out_path)?;
    }
    Ok(())
}

fn init(manifest: Option<&Path>) -> Result<(), CmdInitError> {
    let found_toml;
    let project_path = match manifest {
        Some(manifest_path) => manifest_path
            .parent()
            .ok_or(CmdInitError::InvalidManifestPath)?,
        None => {
            found_toml = look_up(Path::new("Cargo.toml")).ok_or(CmdInitError::ManifestNotFound)?;
            found_toml.parent().ok_or(CmdInitError::ManifestNotFound)?
        }
    };

    let mut memory_toml = project_path.to_path_buf();
    memory_toml.push(DEFAULT_CONFIG_NAME);
    if memory_toml.exists() {
        return Err(CmdInitError::AlreadyInitialized);
    }

    let memory_x = get_memory_x(project_path)?;

    let mut memory_toml_file =
        File::create_new(memory_toml.clone()).into_out_result(&memory_toml)?;

    memory_toml_file
        .write_all(generate_memory_toml(&memory_x).as_bytes())
        .into_out_result(&memory_toml)?;

    let ignorelist_path = project_path.join(DEFAULT_IGNORELIST_NAME);
    backup_if_exists(project_path, DEFAULT_IGNORELIST_NAME)?;
    crateplace::validation::IgnoreList::default()
        .to_file(&ignorelist_path)
        .into_out_result(&ignorelist_path)?;

    let pre_path = project_path.join("pre.x");
    if let Some(pre) = &memory_x.get_pre() {
        backup_if_exists(project_path, "pre.x")?;
        let mut pre_file = File::create(pre_path.clone()).into_out_result(&pre_path)?;
        pre_file
            .write_all(pre.as_bytes())
            .into_out_result(&pre_path)?;
    };
    let post_path = project_path.join("post.x");
    if let Some(post) = &memory_x.get_post() {
        backup_if_exists(project_path, "post.x")?;
        let mut post_file = File::create(post_path.clone()).into_out_result(&post_path)?;
        post_file
            .write_all(post.as_bytes())
            .into_out_result(&post_path)?;
    };

    let cargo_toml = project_path.join("Cargo.toml");
    if !cargo_toml.exists() {
        Err(CmdInitError::NoCargoToml)?;
    }
    let cargo_toml_content = std::fs::read_to_string(&cargo_toml).into_in_result(&cargo_toml)?;
    let mut cargo_toml_file = File::create(&cargo_toml).into_out_result(&cargo_toml)?;
    cargo_toml_file
        .write_all(init_cargo_toml(&cargo_toml_content)?.as_bytes())
        .into_out_result(&cargo_toml)?;

    let build_rs_path = project_path.join("build.rs");
    let build_rs_content = if build_rs_path.exists() {
        let build_rs_content =
            std::fs::read_to_string(&build_rs_path).into_in_result(&build_rs_path)?;
        backup_if_exists(project_path, "build.rs")?;
        init_existing_build_rs(&memory_x, build_rs_content)?
    } else {
        init_new_build_rs(&memory_x)
    };
    let mut build_rs_file = File::create(&build_rs_path).into_out_result(&build_rs_path)?;
    build_rs_file
        .write_all(build_rs_content.as_bytes())
        .into_out_result(&build_rs_path)?;
    println!("Done! Make sure to remove \"memory.x\" or code producing that file.");
    Ok(())
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
        Command::Init => init(manifest)?,
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
                        format!("{ERR}Error{ERR:#}:")
                    }
                    ProblemLevel::Warning => {
                        problem_count += 1;
                        format!("{WARN}Warning{WARN:#}:")
                    }
                    ProblemLevel::Ignored => {
                        if !show_ignored {
                            continue;
                        }
                        format!("{IGN}Ignored{IGN:#}:")
                    }
                };

                println!("{prep} {problem}");
            }
            if custom_symbol_error {
                println!(
                    "{WARN}NOTE{WARN:#}: custom symbol assignments on non-Rust symbols need to be compiled with \"-ffunction-sections -fdata-sections\" or crateplace will not be able to control their placement."
                );
            }
            if problem_count != 0 {
                std::process::exit(2);
            }
        }
        Command::Add(add) => match add {
            Add::Section {
                name,
                origin,
                length,
                priority,
                default,
            } => {
                placer.add_section(&name, origin, length, priority, default)?;
            }
            Add::Crate {
                name,
                nodeps,
                section,
            } => {
                placer.add_crate(&name, &section, !nodeps)?;
            }
            Add::Symbol {
                pattern,
                section,
                notext,
                norodata,
                nodatarel,
            } => placer.add_symbol(&pattern, &section, !notext, !norodata, !nodatarel)?,
        },
        Command::Remove(remove) => match remove {
            Remove::Section { name } => {
                placer.remove_section(&name)?;
            }
            Remove::Crate { name } => placer.remove_crate(&name)?,
            Remove::Symbol { pattern } => placer.remove_symbol(&pattern)?,
        },
        Command::SetRam { origin, length } => placer.set_ram(origin, length)?,
        Command::ValidateConfig => placer.validate_config()?,
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
