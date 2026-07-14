mod assignment;
pub mod config;
pub mod deps;
pub mod file_error;
mod generation;
pub mod init;
pub mod mangling;
pub mod validation;
use crate::{
    assignment::{AssignmentError, assign},
    config::{Config, ConfigLoadError, ConfigValidationError},
    deps::{DepTree, Inverted},
    file_error::{FileError, IOToFileResult},
    mangling::{ManglingDetectionError, ManglingVersion, rustc_mangling_version},
    validation::{IgnoreList, ValidationError, ValidationProblem},
};

use anstream::println;
use cargo_metadata::Message;
use deps::{DepsError, get_deps};
use std::{
    env,
    error::Error,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub use generation::ManglingMatches;

const DEFAULT_CONFIG_NAME: &str = "Memory.toml";
const DEFAULT_OUTPUT_NAME: &str = "memory.x";
const DEFAULT_IGNORELIST_NAME: &str = ".crateplace-ignore";
const CARGO_MANIFEST: &str = "Cargo.toml";

#[derive(thiserror::Error, Debug)]
pub enum CratePlacerError {
    #[error("Failed to retrieve dependencies")]
    Deps(
        #[source]
        #[from]
        DepsError,
    ),
    #[error("Failed to parse toml")]
    TomlParse(
        #[source]
        #[from]
        toml::de::Error,
    ),
    #[error("IO Error: {err}: {path}")]
    IO {
        #[source]
        err: std::io::Error,
        path: String,
    },
    #[error("Failed to assign sections to crates")]
    Placement(
        #[source]
        #[from]
        AssignmentError,
    ),
    #[error("Output path had no parent: {0}")]
    InvalidPath(String),
    #[error("Failed to find {0}")]
    FailedToFindConfig(String),
    #[error("Failed to find Cargo.toml")]
    NoOutput,
    #[error("Failed to find crate: {0}")]
    DepNotFound(String),
    #[error("Invalid configuration")]
    InvalidConfig(
        #[source]
        #[from]
        ConfigValidationError,
    ),
    #[error("Failed to detect mangling version")]
    ManglingDetectionError(
        #[source]
        #[from]
        ManglingDetectionError,
    ),

    #[error("Validation")]
    ValidationError(
        #[source]
        #[from]
        ValidationError,
    ),
    #[error("Build error")]
    BuildError,
    #[error("Project has no output binary")]
    NoOutputBinary,
    #[error("Failed to load config")]
    ConifgLoadError(
        #[source]
        #[from]
        ConfigLoadError,
    ),
    #[error("File error")]
    FileError(
        #[source]
        #[from]
        FileError,
    ),
}

pub fn report(mut err: &dyn Error) {
    eprint!("{err}");
    while let Some(source) = err.source() {
        eprint!(": {source}");
        err = source;
    }
}

fn divine_mangling() -> Result<ManglingVersion, CratePlacerError> {
    let flags = env::var("CARGO_ENCODED_RUSTFLAGS");
    let flags = flags.iter().flat_map(|flags| flags.split('\x1f'));

    let target = env::var("TARGET");
    let target = target
        .iter()
        .flat_map(|target| ["--target", target.as_str()].into_iter());

    Ok(rustc_mangling_version(
        env::var("RUSTC").ok().as_deref(),
        flags.chain(target).filter(|arg| !arg.is_empty()),
    )?)
}

trait FileConfigData: Sized {
    type Error;
    fn from_file(path: &Path) -> Result<Self, Self::Error>;
}

#[derive(Clone, Debug)]
struct FileConfig<Config> {
    default_file_name: &'static str,
    path: Option<PathBuf>,
    config: Option<Config>,
}

impl<Config: FileConfigData> FileConfig<Config>
where
    CratePlacerError: From<<Config as FileConfigData>::Error>,
{
    pub fn new(default_file_name: &'static str) -> Self {
        Self {
            default_file_name,
            path: None,
            config: None,
        }
    }

    pub fn set(&mut self, config: Config) {
        self.config = Some(config);
    }

    pub fn set_path<P: Into<PathBuf>>(&mut self, path: P) {
        self.path = Some(path.into());
    }

    fn get_path(&mut self, manifest_dir: Option<&Path>) -> Result<&Path, CratePlacerError> {
        if self.path.is_none() {
            match manifest_dir {
                Some(manifest_dir) => {
                    let mut config_file = manifest_dir.to_path_buf();
                    config_file.push(self.default_file_name);
                    self.path = Some(config_file);
                }
                None => {
                    self.path = look_up(Path::new(self.default_file_name));
                }
            }
        }
        self.path
            .as_deref()
            .ok_or_else(|| CratePlacerError::FailedToFindConfig(self.default_file_name.to_string()))
    }

    fn exists(&mut self, manifest_dir: Option<&Path>) -> bool {
        self.get_path(manifest_dir)
            .map(|path| path.exists())
            .unwrap_or_default()
    }

    fn get(&mut self, manifest_dir: Option<&Path>) -> Result<&Config, CratePlacerError> {
        if self.config.is_none() {
            let path = self.get_path(manifest_dir)?;
            let config = Config::from_file(path)?;
            self.config = Some(config);
        }
        self.config
            .as_ref()
            .ok_or_else(|| CratePlacerError::FailedToFindConfig(self.default_file_name.to_string()))
    }
}

#[derive(Clone, Debug)]
pub struct CratePlacer {
    manifest: Option<PathBuf>,
    config: FileConfig<Config>,
    output: Option<PathBuf>,
    ignorelist: FileConfig<IgnoreList>,
    pre_script: Option<String>,
    post_script: Option<String>,
    stdout: bool,
}

impl Default for CratePlacer {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn look_up(filename: &Path) -> Option<PathBuf> {
    let curdir = env::current_dir().ok()?;
    let mut dir = curdir.as_path();
    loop {
        let dirlist = fs::read_dir(dir);
        if let Some(file) = dirlist
            .ok()?
            .filter_map(|entry| entry.ok())
            .find(|entry| entry.file_name() == filename)
        {
            return Some(file.path());
        };
        dir = dir.parent()?;
    }
}

fn get_manifest(manifest_path: &mut Option<PathBuf>) -> Option<&Path> {
    if manifest_path.is_none() {
        *manifest_path = look_up(Path::new(CARGO_MANIFEST));
    }
    manifest_path.as_deref()
}

fn get_manifest_dir(manifest_path: &mut Option<PathBuf>) -> Option<&Path> {
    get_manifest(manifest_path)?.parent()
}

impl CratePlacer {
    pub fn new() -> Self {
        Self {
            manifest: None,
            config: FileConfig::new(DEFAULT_CONFIG_NAME),
            output: None,
            stdout: false,
            ignorelist: FileConfig::new(DEFAULT_IGNORELIST_NAME),
            pre_script: None,
            post_script: None,
        }
    }

    pub fn stdout(&mut self, stdout: bool) {
        self.stdout = stdout;
    }

    pub fn cargo_manifest<P: Into<PathBuf>>(&mut self, manifest: P) -> &mut Self {
        self.manifest = Some(manifest.into());
        self
    }

    pub fn config_file<P: Into<PathBuf>>(&mut self, config_file: P) -> &mut Self {
        self.config.set_path(config_file);
        self
    }

    pub fn config(&mut self, config: Config) -> &mut Self {
        self.config.set(config);
        self
    }

    pub fn output<P: Into<PathBuf>>(&mut self, output: P) -> &mut Self {
        self.output = Some(output.into());
        self
    }

    pub fn ignorelist_file<P: Into<PathBuf>>(&mut self, ignorelist_file: P) -> &mut Self {
        self.ignorelist.set_path(ignorelist_file);
        self
    }

    pub fn ignorelist(&mut self, list: IgnoreList) -> &mut Self {
        self.ignorelist.set(list);
        self
    }

    pub fn pre_script(&mut self, script: &str) -> &mut Self {
        self.pre_script = Some(script.to_string());
        self
    }

    pub fn post_script(&mut self, script: &str) -> &mut Self {
        self.post_script = Some(script.to_string());
        self
    }

    pub fn get_deps(&self) -> Result<DepTree, CratePlacerError> {
        Ok(get_deps(self.manifest.as_deref())?)
    }

    fn get_output_dir(&mut self) -> Result<PathBuf, CratePlacerError> {
        if let Some(path) = &self.output {
            Ok(path
                .parent()
                .ok_or(CratePlacerError::InvalidPath(
                    path.to_string_lossy().to_string(),
                ))?
                .to_owned())
        } else {
            if let Some(manifest) = &self.manifest
                && let Some(manifest_dir) = manifest.parent()
            {
                Ok(manifest_dir.to_path_buf())
            } else {
                Ok(PathBuf::from(
                    env::var_os("OUT_DIR").ok_or(CratePlacerError::NoOutput)?,
                ))
            }
        }
    }

    fn get_output_file(&mut self) -> Result<&Path, CratePlacerError> {
        if self.output.is_none() {
            let mut output_file = self.get_output_dir()?;
            output_file.push(DEFAULT_OUTPUT_NAME);
            self.output = Some(output_file);
        }
        Ok(self.output.as_ref().unwrap())
    }

    pub fn get_assigned_deps(&mut self) -> Result<DepTree, CratePlacerError> {
        let mut deps = get_deps(self.manifest.as_deref())?;
        let config = self.config.get(self.manifest.as_deref())?;
        config.validate()?;
        assign(config, &mut deps)?;
        Ok(deps)
    }

    pub fn display_tree(
        &mut self,
        show_unspecified: bool,
        no_dedupe: bool,
        inverted: Inverted,
    ) -> Result<(), CratePlacerError> {
        let mut deps = self.get_assigned_deps()?;
        deps.no_dedupe(no_dedupe);
        deps.display_unspecified(show_unspecified);
        match inverted {
            Inverted::Not => (),
            Inverted::Inverted(dep) => {
                deps.inverted(Inverted::Inverted(
                    deps.crates
                        .iter()
                        .find(|(_, node)| node.name == dep)
                        .ok_or(CratePlacerError::DepNotFound(dep))?
                        .0
                        .clone(),
                ));
            }
        }
        println!("{deps}");
        Ok(())
    }

    pub fn get_linkerscript(
        &mut self,
        mangling: Option<ManglingMatches>,
    ) -> Result<String, CratePlacerError> {
        let deps = self.get_assigned_deps()?;
        let config = self.config.get(get_manifest_dir(&mut self.manifest))?;
        let mangling = match mangling {
            Some(mangling) => mangling,
            None => match divine_mangling()? {
                ManglingVersion::Legacy => generation::ManglingMatches::All,
                ManglingVersion::V0 => generation::ManglingMatches::V0,
            },
        };
        Ok(generation::generate_script(
            config,
            &deps,
            mangling,
            self.pre_script.as_deref(),
            self.post_script.as_deref(),
        ))
    }

    pub fn write_linkerscript(
        &mut self,
        mangling: Option<ManglingMatches>,
    ) -> Result<(), CratePlacerError> {
        let linkerscript = self.get_linkerscript(mangling)?;
        if self.stdout {
            println!("{}", linkerscript);
        } else {
            let output_file = self.get_output_file()?;
            let mut output = File::create(output_file).file_out_result(output_file)?;
            output
                .write_all(linkerscript.as_bytes())
                .file_out_result(output_file)?;
        }
        Ok(())
    }

    pub fn buildscript(&mut self) -> Result<(), CratePlacerError> {
        let manifest_dir = get_manifest_dir(&mut self.manifest);
        println!(
            "cargo::rerun-if-changed={}",
            self.config.get_path(manifest_dir)?.to_string_lossy()
        );
        println!(
            "cargo::rerun-if-changed={}",
            self.ignorelist.get_path(manifest_dir)?.to_string_lossy()
        );
        println!(
            "cargo:rustc-link-search={}",
            self.get_output_dir()?.to_string_lossy()
        );

        self.write_linkerscript(None)?;
        Ok(())
    }

    pub fn bless(&mut self, problems: &[ValidationProblem]) -> Result<(), CratePlacerError> {
        let manifest_dir = get_manifest_dir(&mut self.manifest);
        let ignore_list_path = self.ignorelist.get_path(manifest_dir)?.to_path_buf();
        let ignore_list = self.ignorelist.get(manifest_dir)?;
        let mut patterns: Vec<String> = ignore_list
            .patterns()
            .iter()
            .map(String::to_string)
            .collect();
        patterns.extend(problems.iter().filter_map(|problem| {
            let name = match problem {
                ValidationProblem::SymbolTooBig { name, .. } => name,
                ValidationProblem::SymbolPlacement { name, .. } => name,
                ValidationProblem::SymbolAssignment { name, .. } => name,
                ValidationProblem::UnknownManglingScheme { name, .. } => name,
                ValidationProblem::NoCrateName { name } => name,
                ValidationProblem::ClassificationFailure { name } => name,
                ValidationProblem::NonExistentCrate { name, .. } => name,
                ValidationProblem::NonExistentSection { symbol, .. } => symbol,
                ValidationProblem::SymbolOverflow { symbol, .. } => symbol,
                ValidationProblem::NonExistentSectionCrate { .. }
                | ValidationProblem::Ignored(..)
                | ValidationProblem::InvalidGlobPattern { .. }
                | ValidationProblem::SectionOverflow { .. } => return None,
            };
            let new = format!("^{name}$");
            Some(new)
        }));
        let new_list = IgnoreList::new(&patterns)?;
        new_list
            .to_file(&ignore_list_path)
            .file_out_result(&ignore_list_path)?;
        self.ignorelist.set(new_list);
        Ok(())
    }

    pub fn validate(
        &mut self,
        output_file: &Path,
    ) -> Result<Vec<ValidationProblem>, CratePlacerError> {
        let manifest = get_manifest(&mut self.manifest);
        let manifest_dir = manifest.and_then(|path| path.parent());
        let mut deps = get_deps(manifest)?;
        let config = self.config.get(manifest_dir)?;
        config.validate()?;
        assign(config, &mut deps)?;
        if !self.ignorelist.exists(manifest_dir) {
            self.ignorelist.set(IgnoreList::default());
        }
        let ignore_list = self.ignorelist.get(manifest_dir)?;
        Ok(validation::validate(
            output_file,
            &deps,
            config,
            ignore_list,
        )?)
    }

    pub fn build_then_validate(&mut self) -> Result<Vec<ValidationProblem>, CratePlacerError> {
        let mut cmd = Command::new("cargo");
        cmd.args(["build", "--message-format=json"]);
        if let Some(manifest) = &self.manifest {
            cmd.args(["--manifest-path", &manifest.to_string_lossy()]);
        }
        let mut proc = cmd
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|_| CratePlacerError::BuildError)?;

        let reader =
            std::io::BufReader::new(proc.stdout.take().ok_or(CratePlacerError::BuildError)?);
        let mut output = None;
        for message in Message::parse_stream(reader) {
            if let Message::CompilerArtifact(artifact) =
                message.map_err(|_| CratePlacerError::BuildError)?
                && let Some(exec) = artifact.executable
            {
                output = Some(exec);
            }
        }
        match proc.wait() {
            Ok(status) => {
                if !status.success() {
                    return Err(CratePlacerError::BuildError);
                }
            }
            Err(_) => return Err(CratePlacerError::BuildError),
        };
        self.validate(Path::new(&output.ok_or(CratePlacerError::NoOutputBinary)?))
    }
}
