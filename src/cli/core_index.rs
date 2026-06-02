use std::fs;
use std::process::ExitCode;

use rsscript::core_package_index_json;

pub fn run_core_index(args: &[String]) -> ExitCode {
    match args {
        [] => {
            print!("{}", core_package_index_json());
            ExitCode::SUCCESS
        }
        [flag, path] if flag == "--write" => {
            if let Err(error) = fs::write(path, core_package_index_json()) {
                eprintln!("failed to write {path}: {error}");
                return ExitCode::from(2);
            }
            ExitCode::SUCCESS
        }
        [flag, path] if flag == "--check" => match fs::read_to_string(path) {
            Ok(existing) if existing == core_package_index_json() => ExitCode::SUCCESS,
            Ok(_) => {
                eprintln!("core/package index is stale: {path}");
                ExitCode::from(1)
            }
            Err(error) => {
                eprintln!("failed to read {path}: {error}");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("usage: rss core-index [--write <path>|--check <path>]");
            ExitCode::from(2)
        }
    }
}
