use indoc::indoc;

use crate::{DEFAULT_CONFIG_NAME, look_up};
use std::{
    fs::File,
    io::{self, Write},
    path::Path,
};

const DEFAULT_MEMORY_TOML: &str = indoc! {"
    ram = { origin = \"0x20000000\", length = \"128K\" }

    [sections]
    flash = { origin = \"0x00000000\", length = \"1M\", priority = 1 }

    [crates]
"};

const DEFAULT_BUILD_RS: &str = indoc! {"
    fn main() {
    if let Err(err) = crateplace::CratePlacer::new().buildscript() {
        crateplace::report(&err);
        std::process::exit(1);
    }
}"};

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("Invalid manifest path")]
    InvalidManifestPath,
    #[error("Failed to find manifest")]
    ManifestNotFound,
    #[error("Memory.toml already exists")]
    MemoryFileExists,
    #[error("build.rs already exists, add the contents of main to build.rs: \n{DEFAULT_BUILD_RS}")]
    BuildRsExists,
    #[error("Failed to make file: \"{path}\"")]
    FileError {
        #[source]
        err: io::Error,
        path: String,
    },
}

trait IOToInitError<T> {
    fn file_error(self, path: &Path) -> Result<T, InitError>;
}

impl<T> IOToInitError<T> for Result<T, io::Error> {
    fn file_error(self, path: &Path) -> Result<T, InitError> {
        self.map_err(|err| InitError::FileError {
            err,
            path: path.to_string_lossy().to_string(),
        })
    }
}

pub fn init(manifest: Option<&Path>) -> Result<(), InitError> {
    let found_toml;
    let project_path = match manifest {
        Some(manifest_path) => manifest_path
            .parent()
            .ok_or(InitError::InvalidManifestPath)?,
        None => {
            found_toml = look_up(Path::new("Cargo.toml")).ok_or(InitError::ManifestNotFound)?;
            found_toml.parent().ok_or(InitError::ManifestNotFound)?
        }
    };

    let mut memory_toml = project_path.to_path_buf();
    memory_toml.push(DEFAULT_CONFIG_NAME);
    if memory_toml.exists() {
        return Err(InitError::MemoryFileExists);
    }
    let mut memory_toml_file = File::create_new(memory_toml.clone()).file_error(&memory_toml)?;
    memory_toml_file
        .write_all(DEFAULT_MEMORY_TOML.as_bytes())
        .file_error(&memory_toml)?;

    let mut build_rs = project_path.to_path_buf();
    build_rs.push("build.rs");
    if build_rs.exists() {
        return Err(InitError::BuildRsExists);
    }
    let mut build_rs_file = File::create_new(build_rs.clone()).file_error(&build_rs)?;
    build_rs_file
        .write_all(DEFAULT_BUILD_RS.as_bytes())
        .file_error(&build_rs)?;
    Ok(())
}
