use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(junto_server::OPENAPI_DOCUMENT_PATH));
    let document = match junto_server::openapi_document() {
        Ok(document) => document,
        Err(error) => {
            eprintln!("cannot build the OpenAPI document: {error}");
            return ExitCode::FAILURE;
        }
    };
    match std::fs::write(&path, document) {
        Ok(()) => {
            println!("wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("cannot write {}: {error}", path.display());
            ExitCode::FAILURE
        }
    }
}
