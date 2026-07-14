use indoc::indoc;

use crate::file_error::{FileError, IOToFileError};
use crate::{DEFAULT_CONFIG_NAME, DEFAULT_IGNORELIST_NAME, look_up};
use std::{fs::File, io::Write, path::Path};

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
    #[error("File error")]
    FileError(
        #[source]
        #[from]
        FileError,
    ),
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
    let mut memory_toml_file = File::create_new(memory_toml.clone()).write_error(&memory_toml)?;
    memory_toml_file
        .write_all(DEFAULT_MEMORY_TOML.as_bytes())
        .write_error(&memory_toml)?;

    let mut ignorelist = project_path.join(DEFAULT_IGNORELIST_NAME);
    crate::validation::IgnoreList::default()
        .to_file(&ignorelist)
        .write_error(&ignorelist)?;

    let mut build_rs = project_path.to_path_buf();
    build_rs.push("build.rs");
    if build_rs.exists() {
        return Err(InitError::BuildRsExists);
    }

    let mut build_rs_file = File::create_new(build_rs.clone()).write_error(&build_rs)?;
    build_rs_file
        .write_all(DEFAULT_BUILD_RS.as_bytes())
        .write_error(&build_rs)?;

    Ok(())
}
