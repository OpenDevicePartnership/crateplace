use std::{ffi::OsStr, io::Write};

static TEST_PROGRAM: &str = "#![no_std] pub fn test() {}";

#[derive(thiserror::Error, Debug)]
pub enum ManglingDetectionError {
    #[error("Failed to find a version o rustc to check")]
    NoRustc,
    #[error("Error while calling rustc: {0}")]
    RustcError(std::io::Error),
    #[error("Failed to communicate with rustc")]
    NoRustcIO,
    #[error("Did not recognize mangling scheme: {0}")]
    UnrecognizedMangling(String),
    #[error("Failed to parse rustc output")]
    LlvmIrParseError,
}

#[derive(Copy, Clone, Debug)]
pub enum ManglingVersion {
    Legacy,
    V0,
}

impl ManglingVersion {
    pub fn from_mangling_string_prefix(string: &str) -> Option<Self> {
        if string.starts_with("_R") {
            Some(Self::V0)
        } else if string.starts_with("_ZN") {
            Some(Self::Legacy)
        } else {
            None
        }
    }
}

impl std::fmt::Display for ManglingVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManglingVersion::Legacy => write!(f, "legacy"),
            ManglingVersion::V0 => write!(f, "v0"),
        }
    }
}

pub fn rustc_mangling_version<I, S>(
    rustc: Option<&str>,
    arguments: I,
) -> Result<ManglingVersion, ManglingDetectionError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let program = rustc.unwrap_or("rustc");
    let mut rustc = std::process::Command::new(program)
        .args([
            "-",
            "-o",
            "-",
            "--emit",
            "llvm-ir",
            "--crate-type",
            "lib",
            "--crate-name",
            "test",
        ])
        .args(arguments)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(ManglingDetectionError::RustcError)?;

    {
        let mut stdin = rustc
            .stdin
            .take()
            .ok_or(ManglingDetectionError::NoRustcIO)?;
        stdin
            .write_all(TEST_PROGRAM.as_bytes())
            .map_err(ManglingDetectionError::RustcError)?;
    }
    let out = rustc
        .wait_with_output()
        .map_err(ManglingDetectionError::RustcError)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find(|line| line.contains("define") && line.contains("test"))
        .ok_or(ManglingDetectionError::LlvmIrParseError)?;
    let mangling_string = line
        .split_once("@")
        .ok_or(ManglingDetectionError::LlvmIrParseError)?
        .1
        .split_once("test")
        .ok_or(ManglingDetectionError::LlvmIrParseError)?
        .0;
    ManglingVersion::from_mangling_string_prefix(mangling_string)
        .ok_or_else(|| ManglingDetectionError::UnrecognizedMangling(mangling_string.to_string()))
}
