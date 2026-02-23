use std::path::Path;
use std::process::ExitCode;

fn usage(program_name: &str) {
    eprintln!("Usage: {program_name} <input.heif|.heic|.avif> <output.png>");
}

fn format_decode_failure(err: &libheic_rs::DecodeError) -> String {
    format!(
        "Decode failed [category={}]: {err}",
        err.category().as_str()
    )
}

fn main() -> ExitCode {
    let mut args = std::env::args();
    let program_name = args.next().unwrap_or_else(|| "heif-decode".to_string());
    let input_path = args.next();
    let output_path = args.next();

    if input_path.is_none() || output_path.is_none() || args.next().is_some() {
        usage(&program_name);
        return ExitCode::from(2);
    }

    let input_path = Path::new(input_path.as_deref().unwrap_or_default());
    let output_path = Path::new(output_path.as_deref().unwrap_or_default());

    match libheic_rs::decode_file_to_png(input_path, output_path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{}", format_decode_failure(&err));
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::format_decode_failure;
    use libheic_rs::{DecodeError, DecodeHeicError};

    #[test]
    fn formats_decode_failure_with_structured_category() {
        let err = DecodeError::Unsupported("unsupported extension".to_string());
        assert_eq!(
            format_decode_failure(&err),
            "Decode failed [category=unsupported-feature]: unsupported extension"
        );
    }

    #[test]
    fn formats_nested_decode_failure_with_malformed_category() {
        let err = DecodeError::HeicDecode(DecodeHeicError::MissingSpsNalUnit);
        assert_eq!(
            format_decode_failure(&err),
            "Decode failed [category=malformed-input]: length-prefixed HEVC stream does not contain an SPS NAL unit"
        );
    }
}
