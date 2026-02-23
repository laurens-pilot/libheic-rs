use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::Path;

pub mod isobmff;

/// Errors returned by the decoder entry points.
#[derive(Debug)]
pub enum DecodeError {
    Io(std::io::Error),
    Unsupported(String),
}

impl Display for DecodeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Io(err) => write!(f, "I/O error: {err}"),
            DecodeError::Unsupported(msg) => write!(f, "{msg}"),
        }
    }
}

impl Error for DecodeError {}

impl From<std::io::Error> for DecodeError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Decode a HEIF/HEIC/AVIF image from `input_path` and write a PNG to `output_path`.
///
/// This is a placeholder entry point that establishes the public API surface for
/// upcoming implementation work.
pub fn decode_file_to_png(input_path: &Path, output_path: &Path) -> Result<(), DecodeError> {
    if !input_path.exists() {
        return Err(DecodeError::Unsupported(format!(
            "Input file does not exist: {}",
            input_path.display()
        )));
    }

    let _ = output_path;
    Err(DecodeError::Unsupported(
        "Decoder not implemented yet.".to_string(),
    ))
}
