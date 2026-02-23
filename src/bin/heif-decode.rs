use std::path::Path;
use std::process::ExitCode;

fn usage(program_name: &str) {
    eprintln!("Usage: {program_name} <input.heif|.heic|.avif> <output.png>");
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
            eprintln!("Decode failed: {err}");
            ExitCode::from(1)
        }
    }
}
