use std::io::Error;
use std::path::Path;

#[derive(thiserror::Error, Debug)]
pub enum FileError {
    #[error("Failed to write to file: {filename}")]
    Write {
        #[source]
        error: std::io::Error,
        filename: String,
    },
    #[error("Failed to read file: {filename}")]
    Read {
        #[source]
        error: std::io::Error,
        filename: String,
    },
}

pub trait IOToFileError<T> {
    fn read_error(self, path: &Path) -> Result<T, FileError>;
    fn write_error(self, path: &Path) -> Result<T, FileError>;
}

impl<T> IOToFileError<T> for Result<T, Error> {
    fn read_error(self, path: &Path) -> Result<T, FileError> {
        self.map_err(|error| FileError::Read {
            error,
            filename: path.to_string_lossy().to_string(),
        })
    }

    fn write_error(self, path: &Path) -> Result<T, FileError> {
        self.map_err(|error| FileError::Write {
            error,
            filename: path.to_string_lossy().to_string(),
        })
    }
}
