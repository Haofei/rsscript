mod cli;

fn main() -> std::process::ExitCode {
    cli::run(std::env::args())
}
