#![forbid(unsafe_code)]

mod cli;

fn main() -> std::process::ExitCode {
    cli::run()
}
