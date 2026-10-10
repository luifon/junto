use std::path::PathBuf;
use std::process::ExitCode;

type Generator = fn() -> Result<String, serde_json::Error>;

/// Writes the generated API documents. Without arguments they go to their committed paths;
/// with one argument, into that directory.
fn main() -> ExitCode {
    let directory = std::env::args_os().nth(1).map(PathBuf::from);
    let documents: [(&str, Generator); 2] = [
        (
            junto_server::OPENAPI_DOCUMENT_PATH,
            junto_server::openapi_document,
        ),
        (
            junto_server::EVENTS_DOCUMENT_PATH,
            junto_server::events_document,
        ),
    ];
    for (committed_path, generate) in documents {
        let committed = PathBuf::from(committed_path);
        let path = match (&directory, committed.file_name()) {
            (Some(directory), Some(name)) => directory.join(name),
            _ => committed,
        };
        let document = match generate() {
            Ok(document) => document,
            Err(error) => {
                eprintln!("cannot build {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        };
        if let Err(error) = std::fs::write(&path, document) {
            eprintln!("cannot write {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
        println!("wrote {}", path.display());
    }
    ExitCode::SUCCESS
}
