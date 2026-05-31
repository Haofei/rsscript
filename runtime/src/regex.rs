use regex::Regex;

#[derive(Debug, Clone)]
pub struct RssRegex {
    inner: Regex,
}

#[derive(Debug, Clone)]
pub struct RegexError {
    pub message: String,
}

pub fn regex_compile(pattern: &str) -> Result<RssRegex, RegexError> {
    Regex::new(pattern)
        .map(|inner| RssRegex { inner })
        .map_err(|e| RegexError {
            message: e.to_string(),
        })
}

pub fn regex_is_match(regex: &RssRegex, value: &str) -> bool {
    regex.inner.is_match(value)
}

pub fn regex_find(regex: &RssRegex, value: &str) -> Option<String> {
    regex.inner.find(value).map(|m| m.as_str().to_string())
}

pub fn regex_captures(regex: &RssRegex, value: &str) -> Vec<String> {
    regex
        .inner
        .captures(value)
        .map(|caps| {
            caps.iter()
                .filter_map(|m| m.map(|m| m.as_str().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

pub fn regex_replace_all(regex: &RssRegex, value: &str, replacement: &str) -> String {
    regex.inner.replace_all(value, replacement).to_string()
}

pub fn regex_split(regex: &RssRegex, value: &str) -> Vec<String> {
    regex.inner.split(value).map(|s| s.to_string()).collect()
}

pub fn regex_error_message(error: &RegexError) -> String {
    error.message.clone()
}
