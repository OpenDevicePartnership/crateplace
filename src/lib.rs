mod assignment;
pub mod config;
pub mod deps;
mod generation;
pub mod init;
use crate::{
    assignment::{AssignmentError, assign},
    config::{Config, ConfigValidationError},
    deps::{DepTree, Inverted},
};
use deps::{DepsError, get_deps};
use std::{
    error::Error,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
};

const DEFAULT_CONFIG_NAME: &str = "Memory.toml";
const DEFAULT_OUTPUT_NAME: &str = "memory.x";

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
    #[error("Failed to find Memory.toml")]
    FailedToFindConfig,
    #[error("Failed to determine output path")]
    NoOutput,
    #[error("Failed to find crate: {0}")]
    DepNotFound(String),
    #[error("Invalid configuration")]
    InvalidConfig(
        #[source]
        #[from]
        ConfigValidationError,
    ),
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

#[derive(Clone, Debug)]
pub struct CratePlacer<'p> {
    manifest: Option<&'p Path>,
    config: Option<Config>,
    config_file: Option<&'p Path>,
    output: Option<&'p Path>,
    stdout: bool,
}

impl<'p> Default for CratePlacer<'p> {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn look_up(filename: &Path) -> Option<PathBuf> {
    let curdir = std::env::current_dir().ok()?;
    let mut dir = curdir.as_path();
    loop {
        let dirlist = std::fs::read_dir(dir);

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

    pub fn get_deps(&self) -> Result<DepTree, CratePlacerError> {
        Ok(get_deps(self.manifest)?)
    }

    fn get_config_path(&self) -> Result<PathBuf, CratePlacerError> {
        match self.config_file {
            Some(path) => Ok(path.to_owned()),
            None => {
                if let Some(manifest) = self.manifest
                    && let Some(manifest_dir) = manifest.parent()
                {
                    let mut config_file = manifest_dir.to_path_buf();
                    config_file.push(DEFAULT_CONFIG_NAME);
                    Ok(config_file)
                } else {
                    Ok(look_up(Path::new(DEFAULT_CONFIG_NAME))
                        .ok_or(CratePlacerError::FailedToFindConfig)?)
                }
            }
        }
    }

    pub fn get_config(&mut self) -> Result<&Config, CratePlacerError> {
        if self.config.is_none() {
            let path = self.get_config_path()?;
            let content = std::fs::read_to_string(&path).file_error(&path)?;
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
                    std::env::var_os("OUT_DIR").ok_or(CratePlacerError::NoOutput)?,
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

    pub fn get_linkerscript(&mut self) -> Result<String, CratePlacerError> {
        let deps = self.get_assigned_deps()?;
        let config = self.get_config()?;
        Ok(generation::generate_script(config, &deps))
    }

    pub fn write_linkerscript(&mut self) -> Result<(), CratePlacerError> {
        let linkerscript = self.get_linkerscript()?;
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
        self.write_linkerscript()?;
        Ok(())
    }
}
