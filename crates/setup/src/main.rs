use std::process::ExitCode;

use junto_setup::cli::{Invocation, run};

fn main() -> ExitCode {
    let invocation = Invocation {
        arguments: std::env::args().skip(1).collect(),
        token: std::env::var(junto_setup::cli::TOKEN_VARIABLE).ok(),
        server: std::env::var(junto_setup::cli::SERVER_VARIABLE).ok(),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("cannot start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
    ExitCode::from(runtime.block_on(run(invocation, &mut out, &mut err)))
}
