use std::process::ExitCode;

use rsscript_runner_protocol::RunnerProfileV1;
use serde_json::json;

pub(crate) fn run_profile(args: &[String]) -> ExitCode {
    let (json_output, requested) = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    let profiles = RunnerProfileV1::ALL
        .into_iter()
        .filter(|profile| requested.is_none_or(|name| profile.name() == name))
        .map(|profile| {
            let identity = profile.identity();
            json!({
                "name": profile.name(),
                "id": identity.id,
                "version": identity.version,
                "descriptor_digest": identity.descriptor_digest,
            })
        })
        .collect::<Vec<_>>();

    if let Some(name) = requested
        && profiles.is_empty()
    {
        eprintln!("unknown runner profile `{name}`");
        return ExitCode::from(2);
    }

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema": "rsscript.execution_profile.v1",
                "profiles": profiles.clone(),
            }))
            .expect("profile inventory serializes")
        );
    } else {
        for profile in profiles {
            println!(
                "{name}\t{id}\tv{version}\t{descriptor_digest}",
                name = profile["name"].as_str().expect("name string"),
                id = profile["id"].as_str().expect("id string"),
                version = profile["version"].as_u64().expect("version integer"),
                descriptor_digest = profile["descriptor_digest"]
                    .as_str()
                    .expect("digest string"),
            );
        }
    }

    ExitCode::SUCCESS
}

fn parse_args(args: &[String]) -> Result<(bool, Option<&str>), String> {
    let mut json = false;
    let mut profile = None;
    for argument in args {
        if argument == "--json" {
            json = true;
        } else if argument.starts_with("--") {
            return Err(format!("unknown argument `{argument}`."));
        } else if profile.replace(argument.as_str()).is_some() {
            return Err("`rss profile` accepts at most one profile name.".to_string());
        }
    }
    Ok((json, profile))
}

#[cfg(test)]
mod tests {
    use super::parse_args;

    #[test]
    fn profile_parser_accepts_json_and_one_name() {
        let args = vec!["--json".to_string(), "log-only".to_string()];
        assert_eq!(parse_args(&args).unwrap(), (true, Some("log-only")));
    }

    #[test]
    fn profile_parser_rejects_extra_names() {
        let args = vec!["log-only".to_string(), "no-providers".to_string()];
        assert!(parse_args(&args).is_err());
    }
}
