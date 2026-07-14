use std::io::Error;
use std::path::Path;

#[derive(thiserror::Error, Debug)]
pub enum FileError {
    #[error("Failed to write to file: {filename}")]
    In {
        #[source]
        error: std::io::Error,
        filename: String,
    },
    #[error("Failed to read file: {filename}")]
    Out {
        #[source]
        error: std::io::Error,
        filename: String,
    },
}

pub trait IOToFileResult<T> {
    fn file_in_result(self, path: &Path) -> Result<T, FileError>;
    fn file_out_result(self, path: &Path) -> Result<T, FileError>;
}

impl<T> IOToFileResult<T> for Result<T, Error> {
    fn file_in_result(self, path: &Path) -> Result<T, FileError> {
        self.map_err(|error| FileError::Out {
            error,
            filename: path.to_string_lossy().to_string(),
        })
    }

    fn file_out_result(self, path: &Path) -> Result<T, FileError> {
        self.map_err(|error| FileError::In {
            error,
            filename: path.to_string_lossy().to_string(),
        })
    }
}
