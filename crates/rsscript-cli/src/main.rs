#![forbid(unsafe_code)]
#![allow(clippy::question_mark)]

mod cli;

fn main() -> std::process::ExitCode {
    cli::run()
}
