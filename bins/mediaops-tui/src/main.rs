use std::io::{self, Write};
use std::process::ExitCode;

use clap::Parser;
use mediaops_tui::args::{Args, EXIT_USAGE, refuse_non_interactive};
use mediaops_tui::runtime;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> ExitCode {
    let args = Args::parse();
    if let Err(message) = refuse_non_interactive() {
        let _ = writeln!(io::stderr(), "{message}");
        return ExitCode::from(u8::try_from(EXIT_USAGE).unwrap_or(2));
    }
    match runtime::run(args.api_socket.as_deref(), args.color_enabled()).await {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(err) => {
            let _ = writeln!(io::stderr(), "{err:#}");
            ExitCode::FAILURE
        }
    }
}
