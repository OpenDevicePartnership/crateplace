mod assignment;
pub mod config;
pub mod deps;
mod generation;
pub mod init;
pub mod mangling;
pub mod validation;
use crate::{
    assignment::{AssignmentError, assign},
    config::{Config, ConfigValidationError},
    deps::{DepTree, Inverted},
    mangling::{ManglingDetectionError, ManglingVersion, rustc_mangling_version},
    validation::{IgnoreList, ValidationError, ValidationProblem},
};
use cargo_metadata::Message;
use deps::{DepsError, get_deps};
use std::{
    env,
    error::Error,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

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
}

pub fn report(mut err: &dyn Error) {
    eprint!("{err}");
    while let Some(source) = err.source() {
        eprint!(": {source}");
        err = source;
    }
}

trait IOToCratePlaceError<T> {
    fn file_error(self, path: &Path) -> Result<T, CratePlacerError>;
}

impl<T> IOToCratePlaceError<T> for Result<T, io::Error> {
    fn file_error(self, path: &Path) -> Result<T, CratePlacerError> {
        self.map_err(|err| CratePlacerError::IO {
            err,
            path: path.to_string_lossy().to_string(),
        })
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

trait ConfigFile: Sized {
    type Error;
    fn from_file(path: &Path) -> Result<Self, Self::Error>;
}

#[derive(Clone, Debug)]
struct FileConfig<Config> {
    default_file_name: &'static str,
    path: Option<PathBuf>,
    config: Option<Config>,
}

impl<Config: ConfigFile> FileConfig<Config>
where
    CratePlacerError: From<<Config as ConfigFile>::Error>,
{
    pub fn new(default_file_name: &'static str) -> Self {
        Self {
            default_file_name,
            path: None,
            config: None,
        }
    }

    pub fn set_config(&mut self, config: Config) {
        self.config = Some(config);
    }

    pub fn set_path<Pl: Into<PathBuf>>(&mut self, path: Pl) {
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

    fn get_config(
        &mut self,
        manifest_dir: Option<&Path>,
    ) -> Result<Option<&Config>, CratePlacerError> {
        if self.config.is_none() {
            let path = self.get_path(manifest_dir)?;
            let config = Config::from_file(path)?;
            self.config = Some(config);
        }
        Ok(self.config.as_ref())
    }
}

#[derive(Clone, Debug)]
pub struct CratePlacer<'p> {
    manifest: Option<&'p Path>,
    config: Option<Config>,
    config_file: Option<&'p Path>,
    output: Option<&'p Path>,
    ignorelist_file: Option<&'p Path>,
    ignorelist: Option<IgnoreList>,
    stdout: bool,
}

impl<'p> Default for CratePlacer<'p> {
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

impl<'p> CratePlacer<'p> {
    pub fn new() -> Self {
        Self {
            manifest: None,
            config_file: None,
            config: None,
            output: None,
            stdout: false,
            ignorelist_file: None,
            ignorelist: None,
        }
    }

    pub fn stdout(&mut self, stdout: bool) {
        self.stdout = stdout;
    }

    pub fn cargo_manifest(&mut self, manifest: &'p Path) -> &mut Self {
        self.manifest = Some(manifest);
        self
    }

    pub fn config_file(&mut self, config_file: &'p Path) -> &mut Self {
        self.config_file = Some(config_file);
        self
    }

    pub fn config(&mut self, config: Config) -> &mut Self {
        self.config = Some(config);
        self
    }

    pub fn output(&mut self, output: &'p Path) -> &mut Self {
        self.output = Some(output);
        self
    }

    pub fn ignorelist_file(&mut self, ignorelist_file: &'p Path) -> &mut Self {
        self.ignorelist_file = Some(ignorelist_file);
        self
    }

    pub fn ignorelist(&mut self, list: IgnoreList) -> &mut Self {
        self.ignorelist = Some(list);
        self
    }

    pub fn get_deps(&self) -> Result<DepTree, CratePlacerError> {
        Ok(get_deps(self.manifest)?)
    }

    fn get_manifest_dir(&self) -> Option<PathBuf> {
        if let Some(manifest_path) = &self.manifest {
            Some(manifest_path.parent()?.to_path_buf())
        } else {
            Some(look_up(Path::new(CARGO_MANIFEST))?)
        }
    }

    fn get_ignorelist_path(&self) -> Result<Option<PathBuf>, CratePlacerError> {
        match self.ignorelist_file {
            Some(path) => Ok(Some(path.to_owned())),
            None => match self.get_manifest_dir() {
                Some(manifest_dir) => {
                    let mut ignorelist_file = manifest_dir;
                    ignorelist_file.push(DEFAULT_IGNORELIST_NAME);
                    if ignorelist_file.exists() {
                        Ok(Some(ignorelist_file))
                    } else {
                        Ok(None)
                    }
                }
                None => Ok(look_up(Path::new(DEFAULT_IGNORELIST_NAME))),
            },
        }
    }

    fn get_ignorelist(&mut self) -> Result<&IgnoreList, CratePlacerError> {
        if self.ignorelist.is_none() {
            let path = self.get_ignorelist_path()?;
            if let Some(path) = path {
                self.ignorelist = Some(IgnoreList::from_file(&path)?);
            } else {
                self.ignorelist = Some(IgnoreList::default());
            }
        }
        Ok(self.ignorelist.as_ref().unwrap())
    }

    fn get_config_path(&self) -> Result<PathBuf, CratePlacerError> {
        match self.config_file {
            Some(path) => Ok(path.to_owned()),
            None => match self.get_manifest_dir() {
                Some(manifest_dir) => {
                    let mut config_file = manifest_dir;
                    config_file.push(DEFAULT_CONFIG_NAME);
                    Ok(config_file)
                }
                None => Ok(look_up(Path::new(DEFAULT_CONFIG_NAME))
                    .ok_or(CratePlacerError::FailedToFindConfig)?),
            },
        }
    }

    pub fn get_config(&mut self) -> Result<&Config, CratePlacerError> {
        if self.config.is_none() {
            let path = self.get_config_path()?;
            let content = fs::read_to_string(&path).file_error(&path)?;
            self.config.replace(toml::from_str(&content)?);
        }
        Ok(self.config.as_ref().unwrap())
    }

    fn get_output_dir(&mut self) -> Result<PathBuf, CratePlacerError> {
        if let Some(path) = self.output {
            Ok(path
                .parent()
                .ok_or(CratePlacerError::InvalidPath(
                    path.to_string_lossy().to_string(),
                ))?
                .to_owned())
        } else {
            if let Some(manifest) = self.manifest
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

    fn get_output_file(&mut self) -> Result<PathBuf, CratePlacerError> {
        if let Some(output) = self.output {
            Ok(output.to_owned())
        } else {
            let mut output_file = self.get_output_dir()?;
            output_file.push(DEFAULT_OUTPUT_NAME);
            Ok(output_file)
        }
    }

    pub fn get_assigned_deps(&mut self) -> Result<DepTree, CratePlacerError> {
        let mut deps = get_deps(self.manifest)?;
        let config = self.get_config()?;
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
        mangling: Option<ManglingVersion>,
    ) -> Result<String, CratePlacerError> {
        let deps = self.get_assigned_deps()?;
        let config = self.get_config()?;
        let mangling = match mangling {
            Some(mangling) => mangling,
            None => divine_mangling()?,
        };
        let mangling = match mangling {
            ManglingVersion::Legacy => generation::ManglingMatches::All,
            ManglingVersion::V0 => generation::ManglingMatches::V0,
        };
        Ok(generation::generate_script(config, &deps, mangling))
    }

    pub fn write_linkerscript(
        &mut self,
        mangling: Option<ManglingVersion>,
    ) -> Result<(), CratePlacerError> {
        let linkerscript = self.get_linkerscript(mangling)?;
        if self.stdout {
            println!("{}", linkerscript);
        } else {
            let output_file = self.get_output_file()?;
            let mut output = File::create(&output_file).file_error(&output_file)?;
            output
                .write_all(linkerscript.as_bytes())
                .file_error(&output_file)?;
        }
        Ok(())
    }

    pub fn buildscript(&mut self) -> Result<(), CratePlacerError> {
        println!(
            "cargo::rerun-if-changed={}",
            self.get_config_path()?.to_string_lossy()
        );
        println!(
            "cargo:rustc-link-search={}",
            self.get_output_dir()?.to_string_lossy()
        );

        self.write_linkerscript(None)?;
        Ok(())
    }

    pub fn validate(
        &mut self,
        output_file: &Path,
    ) -> Result<Vec<ValidationProblem>, CratePlacerError> {
        let mut deps = get_deps(self.manifest)?;
        let config = self.get_config()?;
        config.validate()?;
        assign(config, &mut deps)?;
        let ignore_list = self.get_ignorelist()?;

        Ok(validation::validate(
            output_file,
            &deps,
            config,
            &ignore_list,
        )?)
    }

    pub fn build_then_validate(&mut self) -> Result<Vec<ValidationProblem>, CratePlacerError> {
        let mut cmd = Command::new("cargo");
        cmd.args(["build", "--message-format=json"]);
        if let Some(manifest) = self.manifest {
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
