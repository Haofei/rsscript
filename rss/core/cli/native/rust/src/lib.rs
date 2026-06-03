fn normalized_flag(name: &str) -> String {
    if name.starts_with('-') {
        name.to_string()
    } else {
        format!("--{name}")
    }
}

pub fn flag(args: &[String], name: &str) -> bool {
    let flag = normalized_flag(name);
    let prefix = format!("{flag}=");
    args.iter()
        .any(|arg| arg == &flag || arg.starts_with(&prefix))
}

pub fn option(args: &[String], name: &str) -> Option<String> {
    let flag = normalized_flag(name);
    let prefix = format!("{flag}=");
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix(&prefix) {
            return Some(value.to_string());
        }
        if arg == &flag {
            return iter.next().cloned();
        }
    }
    None
}

pub fn positionals(args: &[String]) -> Vec<String> {
    let mut values = Vec::new();
    let mut skip_next = false;
    let mut after_separator = false;
    for arg in args {
        if after_separator {
            values.push(arg.clone());
            continue;
        }
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--" {
            after_separator = true;
            continue;
        }
        if arg.starts_with("--") {
            if !arg.contains('=') {
                skip_next = true;
            }
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        values.push(arg.clone());
    }
    values
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_flags_options_and_positionals() {
        let args = vec![
            "input.txt".to_string(),
            "--verbose".to_string(),
            "--out=target.txt".to_string(),
            "--mode".to_string(),
            "fast".to_string(),
        ];

        assert!(super::flag(&args, &"verbose".to_string()));
        assert_eq!(
            super::option(&args, &"out".to_string()),
            Some("target.txt".to_string())
        );
        assert_eq!(
            super::option(&args, &"mode".to_string()),
            Some("fast".to_string())
        );
        assert_eq!(super::positionals(&args), vec!["input.txt".to_string()]);
    }

    #[test]
    fn positionals_treats_everything_after_double_dash_as_value() {
        let args = vec![
            "--mode".to_string(),
            "fast".to_string(),
            "input.txt".to_string(),
            "--".to_string(),
            "-x".to_string(),
            "--literal".to_string(),
        ];

        assert_eq!(
            super::positionals(&args),
            vec![
                "input.txt".to_string(),
                "-x".to_string(),
                "--literal".to_string()
            ]
        );
    }
}
