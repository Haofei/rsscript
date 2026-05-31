use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::io::Read;
use std::str::Utf8Error;

use crate::diagnostics::Resource;
use crate::fs::{File, RuntimePath, file_open_read};
use crate::managed::{Managed, WeakManaged, manage, weak};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    status: i64,
    body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValue {
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigStore {
    current: ConfigValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cache {
    entries: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    name: String,
    rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalConfig {
    current: Config,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counter {
    value: i64,
}

#[derive(Clone)]
pub struct Environment {
    parent: Option<Managed<Environment>>,
    function: Option<Managed<FunctionObject>>,
}

#[derive(Clone)]
pub struct FunctionObject {
    closure: WeakManaged<Environment>,
}

impl fmt::Debug for Environment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Environment")
            .field("has_parent", &self.parent.is_some())
            .field("has_function", &self.function.is_some())
            .finish()
    }
}

impl fmt::Debug for FunctionObject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FunctionObject")
            .field("has_closure", &self.closure.upgrade().is_some())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    message: String,
}

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbConnection {
    url: String,
    queries: Vec<String>,
}

impl Resource for DbConnection {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbError {
    message: String,
}

impl DbError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for DbError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for DbError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError {
    message: String,
}

impl fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for HttpError {}

pub fn http_error_message(error: &HttpError) -> String {
    error.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimerError {
    message: String,
}

impl fmt::Display for TimerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for TimerError {}

impl From<ConfigError> for HttpError {
    fn from(error: ConfigError) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

impl From<CsvError> for HttpError {
    fn from(error: CsvError) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

impl From<DbError> for HttpError {
    fn from(error: DbError) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

impl From<ImageError> for HttpError {
    fn from(error: ImageError) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

impl From<JsonError> for HttpError {
    fn from(error: JsonError) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Image {
    pub(crate) bytes: Vec<u8>,
    pub(crate) width: Option<i64>,
    pub(crate) height: Option<i64>,
    pub(crate) operations: Vec<&'static str>,
}

#[derive(Debug, Clone)]
pub struct ImageCache {
    capacity: usize,
    entries: VecDeque<Managed<Image>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageError {
    message: String,
}

impl ImageError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for ImageError {}

impl From<std::io::Error> for ImageError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub trait RuntimeImageRef {
    fn with_image<R>(&self, f: impl FnOnce(&Image) -> R) -> R;
}

impl RuntimeImageRef for Image {
    fn with_image<R>(&self, f: impl FnOnce(&Image) -> R) -> R {
        f(self)
    }
}

impl RuntimeImageRef for Managed<Image> {
    fn with_image<R>(&self, f: impl FnOnce(&Image) -> R) -> R {
        let image = self.read();
        f(&image)
    }
}

#[derive(Debug, Clone)]
pub struct JsonValue {
    inner: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    message: String,
}

impl JsonError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for JsonError {}

impl From<serde_json::Error> for JsonError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<std::io::Error> for JsonError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub fn json_error_message(error: &JsonError) -> String {
    error.to_string()
}

#[derive(Debug, Clone)]
pub struct RowBuffer {
    bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Row {
    fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvError {
    message: String,
}

impl CsvError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CsvError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for CsvError {}

impl From<std::io::Error> for CsvError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<Utf8Error> for CsvError {
    fn from(error: Utf8Error) -> Self {
        Self::new(error.to_string())
    }
}

pub fn cache_new() -> Cache {
    Cache {
        entries: HashMap::new(),
    }
}

pub fn cache_insert(cache: &mut Cache, key: &str, value: &str) {
    cache.entries.insert(key.to_string(), value.to_string());
}

pub fn cache_lookup(cache: &Cache, key: &str) -> String {
    cache.entries.get(key).cloned().unwrap_or_default()
}

pub fn cache_get(cache: &Cache) -> Image {
    let bytes = cache
        .entries
        .values()
        .next()
        .map(|value| value.as_bytes().to_vec())
        .unwrap_or_default();
    Image {
        bytes,
        width: None,
        height: None,
        operations: vec!["cache-get"],
    }
}

pub fn request_new(path: &str) -> Request {
    Request {
        path: path.to_string(),
    }
}

pub fn request_path(request: &Request) -> String {
    request.path.clone()
}

pub fn response_ok(body: &str) -> Result<Response, HttpError> {
    Ok(Response {
        status: 200,
        body: body.to_string(),
    })
}

pub fn response_status(response: &Response) -> i64 {
    response.status
}

pub fn response_body(response: &Response) -> String {
    response.body.clone()
}

pub fn http_get(url: &str) -> Result<Response, HttpError> {
    Err(HttpError {
        message: format!("HTTP client runtime is not configured for GET {url}"),
    })
}

pub fn http_post_json(url: &str, _body: &str) -> Result<Response, HttpError> {
    Err(HttpError {
        message: format!("HTTP client runtime is not configured for POST JSON {url}"),
    })
}

pub fn http_post_form(url: &str, _body: &str) -> Result<Response, HttpError> {
    Err(HttpError {
        message: format!("HTTP client runtime is not configured for POST form {url}"),
    })
}

pub fn http_response_status(response: &Response) -> i64 {
    response.status
}

pub fn http_response_text(response: &Response) -> String {
    response.body.clone()
}

pub fn http_response_is_success(response: &Response) -> bool {
    (200..300).contains(&response.status)
}

pub fn config_load<P: RuntimePath + ?Sized>(path: &P) -> Result<ConfigValue, ConfigError> {
    let text = std::fs::read_to_string(path.as_path())?;
    let name = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("default")
        .to_string();
    Ok(ConfigValue { name })
}

pub fn config_name(value: &ConfigValue) -> String {
    value.name.clone()
}

pub fn config_store_new(value: &ConfigValue) -> ConfigStore {
    ConfigStore {
        current: value.clone(),
    }
}

pub fn config_store_replace(store: &mut ConfigStore, value: &ConfigValue) {
    store.current = value.clone();
}

pub fn config_store_name(store: &ConfigStore) -> String {
    store.current.name.clone()
}

pub fn rule_loader_load_rules<P: RuntimePath + ?Sized>(path: &P) -> Result<Vec<Rule>, ConfigError> {
    let text = std::fs::read_to_string(path.as_path())?;
    let rules = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|name| Rule {
            name: name.to_string(),
        })
        .collect();
    Ok(rules)
}

pub fn config_new(name: &str, rules: &[Rule]) -> Config {
    Config {
        name: name.to_string(),
        rules: rules.to_owned(),
    }
}

pub fn config_rule_count(config: &Config) -> i64 {
    config.rules.len() as i64
}

pub fn global_config_new(value: &Config) -> GlobalConfig {
    GlobalConfig {
        current: value.clone(),
    }
}

pub fn global_config_replace(global: &mut GlobalConfig, value: &Config) {
    global.current = value.clone();
}

pub fn global_config_rule_count(global: &GlobalConfig) -> i64 {
    global.current.rules.len() as i64
}

pub fn counter_new(value: i64) -> Counter {
    Counter { value }
}

pub fn counter_add(counter: &mut Counter, amount: i64) {
    counter.value += amount;
}

pub fn counter_value(counter: &Counter) -> i64 {
    counter.value
}

pub fn environment_root() -> Environment {
    Environment {
        parent: None,
        function: None,
    }
}

pub trait RuntimeEnvironmentHandle {
    fn managed_environment(&self) -> Managed<Environment>;
    fn environment_has_parent(&self) -> bool;
    fn environment_has_function(&self) -> bool;
}

impl RuntimeEnvironmentHandle for Environment {
    fn managed_environment(&self) -> Managed<Environment> {
        manage(self.clone())
    }

    fn environment_has_parent(&self) -> bool {
        self.parent.is_some()
    }

    fn environment_has_function(&self) -> bool {
        self.function.is_some()
    }
}

impl RuntimeEnvironmentHandle for Managed<Environment> {
    fn managed_environment(&self) -> Managed<Environment> {
        self.clone()
    }

    fn environment_has_parent(&self) -> bool {
        self.read().parent.is_some()
    }

    fn environment_has_function(&self) -> bool {
        self.read().function.is_some()
    }
}

impl<T: RuntimeEnvironmentHandle + ?Sized> RuntimeEnvironmentHandle for &T {
    fn managed_environment(&self) -> Managed<Environment> {
        (*self).managed_environment()
    }

    fn environment_has_parent(&self) -> bool {
        (*self).environment_has_parent()
    }

    fn environment_has_function(&self) -> bool {
        (*self).environment_has_function()
    }
}

pub trait RuntimeEnvironmentMut {
    fn bind_function_handle(&mut self, function: Managed<FunctionObject>);
}

impl RuntimeEnvironmentMut for Environment {
    fn bind_function_handle(&mut self, function: Managed<FunctionObject>) {
        self.function = Some(function);
    }
}

impl RuntimeEnvironmentMut for Managed<Environment> {
    fn bind_function_handle(&mut self, function: Managed<FunctionObject>) {
        self.write().function = Some(function);
    }
}

pub trait RuntimeFunctionHandle {
    fn managed_function(&self) -> Managed<FunctionObject>;
    fn function_has_closure(&self) -> bool;
}

impl RuntimeFunctionHandle for FunctionObject {
    fn managed_function(&self) -> Managed<FunctionObject> {
        manage(self.clone())
    }

    fn function_has_closure(&self) -> bool {
        self.closure.upgrade().is_some()
    }
}

impl RuntimeFunctionHandle for Managed<FunctionObject> {
    fn managed_function(&self) -> Managed<FunctionObject> {
        self.clone()
    }

    fn function_has_closure(&self) -> bool {
        self.read().closure.upgrade().is_some()
    }
}

impl<T: RuntimeFunctionHandle + ?Sized> RuntimeFunctionHandle for &T {
    fn managed_function(&self) -> Managed<FunctionObject> {
        (*self).managed_function()
    }

    fn function_has_closure(&self) -> bool {
        (*self).function_has_closure()
    }
}

pub fn environment_child(parent: &impl RuntimeEnvironmentHandle) -> Environment {
    Environment {
        parent: Some(parent.managed_environment()),
        function: None,
    }
}

pub fn environment_bind_function(
    env: &mut impl RuntimeEnvironmentMut,
    function: &impl RuntimeFunctionHandle,
) {
    env.bind_function_handle(function.managed_function());
}

pub fn environment_has_parent(env: &impl RuntimeEnvironmentHandle) -> bool {
    env.environment_has_parent()
}

pub fn environment_has_function(env: &impl RuntimeEnvironmentHandle) -> bool {
    env.environment_has_function()
}

pub fn function_object_new(closure: &impl RuntimeEnvironmentHandle) -> FunctionObject {
    FunctionObject {
        closure: weak(&closure.managed_environment()),
    }
}

pub fn function_object_has_closure(function: &impl RuntimeFunctionHandle) -> bool {
    function.function_has_closure()
}

pub fn db_connection_open(url: &str) -> DbConnection {
    DbConnection {
        url: url.to_string(),
        queries: Vec::new(),
    }
}

pub fn db_connection_try_open(url: &str) -> Result<DbConnection, DbError> {
    if url.trim().is_empty() {
        return Err(DbError::new("database URL is empty"));
    }
    Ok(db_connection_open(url))
}

pub fn db_connection_query(conn: &mut DbConnection, sql: &str) -> Result<(), DbError> {
    if sql.trim().is_empty() {
        return Err(DbError::new("SQL query is empty"));
    }
    conn.queries.push(sql.to_string());
    println!("db query on {}: {sql}", conn.url);
    Ok(())
}

pub fn db_close(fd: i64) {
    let _ = fd;
}

pub fn image_load<P: RuntimePath + ?Sized>(path: &P) -> Result<Image, ImageError> {
    let bytes = std::fs::read(path.as_path())?;
    Ok(Image {
        bytes,
        width: None,
        height: None,
        operations: vec!["load"],
    })
}

pub fn image_resize(image: &mut Image, width: i64, height: i64) {
    image.width = Some(width);
    image.height = Some(height);
    image.operations.push("resize");
}

pub fn image_normalize(image: &mut Image) {
    image.operations.push("normalize");
}

pub fn image_sharpen(image: &mut Image) {
    image.operations.push("sharpen");
}

pub fn image_save<I: RuntimeImageRef + ?Sized, P: RuntimePath + ?Sized>(
    image: &I,
    path: &P,
) -> Result<(), ImageError> {
    let bytes = image.with_image(|image| {
        let mut bytes = image.bytes.clone();
        bytes.extend_from_slice(b"\n# rsscript-image-ops:");
        bytes.extend_from_slice(image.operations.join(",").as_bytes());
        if let (Some(width), Some(height)) = (image.width, image.height) {
            bytes.extend_from_slice(format!(";size={width}x{height}").as_bytes());
        }
        bytes
    });
    std::fs::write(path.as_path(), bytes)?;
    Ok(())
}

pub fn image_inspect<I: RuntimeImageRef + ?Sized>(image: &I) {
    let summary = image.with_image(|image| {
        let size = image
            .width
            .zip(image.height)
            .map(|(width, height)| format!("{width}x{height}"))
            .unwrap_or_else(|| "unknown".to_string());
        format!(
            "image bytes={} size={} ops={}",
            image.bytes.len(),
            size,
            image.operations.join(",")
        )
    });
    println!("{summary}");
}

pub fn image_cache_new(capacity: i64) -> ImageCache {
    ImageCache {
        capacity: capacity.max(0) as usize,
        entries: VecDeque::new(),
    }
}

pub fn image_cache_store(cache: &mut ImageCache, image: &Managed<Image>) {
    if cache.capacity == 0 {
        return;
    }
    while cache.entries.len() >= cache.capacity {
        cache.entries.pop_front();
    }
    cache.entries.push_back(image.clone());
}

pub fn image_cache_len(cache: &ImageCache) -> i64 {
    cache.entries.len() as i64
}

pub fn json_parse(text: &str) -> Result<JsonValue, JsonError> {
    serde_json::from_str(text)
        .map(|inner| JsonValue { inner })
        .map_err(JsonError::from)
}

pub fn json_parse_file<P: RuntimePath + ?Sized>(path: &P) -> Result<JsonValue, JsonError> {
    let text = std::fs::read_to_string(path.as_path())?;
    json_parse(&text)
}

pub fn json_quote_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a string to JSON cannot fail")
}

pub fn toml_parse_file<P: RuntimePath + ?Sized>(path: &P) -> Result<JsonValue, JsonError> {
    let text = std::fs::read_to_string(path.as_path())?;
    let value = text
        .parse::<toml::Value>()
        .map_err(|error| JsonError::new(error.to_string()))?;
    let inner = serde_json::to_value(value)?;
    Ok(JsonValue { inner })
}

pub fn json_field(value: &JsonValue, name: &str) -> Result<JsonValue, JsonError> {
    let Some(field) = value.inner.get(name) else {
        return Err(JsonError::new(format!("missing JSON field `{name}`")));
    };
    Ok(JsonValue {
        inner: field.clone(),
    })
}

pub fn json_field_string(value: &JsonValue, name: &str) -> Result<String, JsonError> {
    let field = json_field(value, name)?;
    let Some(text) = field.inner.as_str() else {
        return Err(JsonError::new(format!(
            "JSON field `{name}` is not a string"
        )));
    };
    Ok(text.to_string())
}

pub fn json_field_int(value: &JsonValue, name: &str) -> Result<i64, JsonError> {
    let field = json_field(value, name)?;
    let Some(number) = field.inner.as_i64() else {
        return Err(JsonError::new(format!(
            "JSON field `{name}` is not an integer"
        )));
    };
    Ok(number)
}

pub fn json_field_bool(value: &JsonValue, name: &str) -> Result<bool, JsonError> {
    let field = json_field(value, name)?;
    let Some(flag) = field.inner.as_bool() else {
        return Err(JsonError::new(format!(
            "JSON field `{name}` is not a boolean"
        )));
    };
    Ok(flag)
}

pub fn json_as_string(value: &JsonValue) -> Result<String, JsonError> {
    let Some(text) = value.inner.as_str() else {
        return Err(JsonError::new("JSON value is not a string"));
    };
    Ok(text.to_string())
}

pub fn json_as_int(value: &JsonValue) -> Result<i64, JsonError> {
    let Some(number) = value.inner.as_i64() else {
        return Err(JsonError::new("JSON value is not an integer"));
    };
    Ok(number)
}

pub fn json_as_bool(value: &JsonValue) -> Result<bool, JsonError> {
    let Some(flag) = value.inner.as_bool() else {
        return Err(JsonError::new("JSON value is not a boolean"));
    };
    Ok(flag)
}

pub fn json_is_null(value: &JsonValue) -> bool {
    value.inner.is_null()
}

pub fn json_is_array(value: &JsonValue) -> bool {
    value.inner.is_array()
}

pub fn json_is_object(value: &JsonValue) -> bool {
    value.inner.is_object()
}

pub fn json_object_len(value: &JsonValue) -> Result<i64, JsonError> {
    let Some(fields) = value.inner.as_object() else {
        return Err(JsonError::new("JSON value is not an object"));
    };
    Ok(fields.len() as i64)
}

pub fn json_object_keys(value: &JsonValue) -> Result<Vec<String>, JsonError> {
    let Some(fields) = value.inner.as_object() else {
        return Err(JsonError::new("JSON value is not an object"));
    };
    let mut keys = fields.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    Ok(keys)
}

pub fn json_array_len(value: &JsonValue) -> Result<i64, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    Ok(items.len() as i64)
}

pub fn json_array_get(value: &JsonValue, index: i64) -> Result<JsonValue, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    if index < 0 {
        return Err(JsonError::new(format!(
            "JSON array index `{index}` is negative"
        )));
    }
    let Some(item) = items.get(index as usize) else {
        return Err(JsonError::new(format!(
            "JSON array index `{index}` is out of bounds"
        )));
    };
    Ok(JsonValue {
        inner: item.clone(),
    })
}

pub fn json_array_strings(value: &JsonValue) -> Result<Vec<String>, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    let mut strings = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(text) = item.as_str() else {
            return Err(JsonError::new(format!(
                "JSON array item `{index}` is not a string"
            )));
        };
        strings.push(text.to_string());
    }
    Ok(strings)
}

pub fn json_array_ints(value: &JsonValue) -> Result<Vec<i64>, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    let mut numbers = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(number) = item.as_i64() else {
            return Err(JsonError::new(format!(
                "JSON array item `{index}` is not an integer"
            )));
        };
        numbers.push(number);
    }
    Ok(numbers)
}

pub fn json_array_bools(value: &JsonValue) -> Result<Vec<bool>, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    let mut flags = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(flag) = item.as_bool() else {
            return Err(JsonError::new(format!(
                "JSON array item `{index}` is not a boolean"
            )));
        };
        flags.push(flag);
    }
    Ok(flags)
}

pub fn json_array_contains_string(value: &JsonValue, item: &str) -> Result<bool, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    Ok(items
        .iter()
        .any(|value| value.as_str().is_some_and(|text| text == item)))
}

pub fn json_array_contains_substring(value: &JsonValue, text: &str) -> Result<bool, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    Ok(items
        .iter()
        .any(|value| value.as_str().is_some_and(|item| item.contains(text))))
}

pub fn json_array_contains_prefix(value: &JsonValue, prefix: &str) -> Result<bool, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    Ok(items
        .iter()
        .any(|value| value.as_str().is_some_and(|item| item.starts_with(prefix))))
}

pub fn json_array_count_where(
    value: &JsonValue,
    mut predicate: impl FnMut(JsonValue) -> Result<bool, JsonError>,
) -> Result<i64, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    let mut count = 0_i64;
    for item in items {
        if predicate(JsonValue {
            inner: item.clone(),
        })? {
            count += 1;
        }
    }
    Ok(count)
}

pub fn json_array_fold<T: Clone>(
    value: &JsonValue,
    initial: &T,
    mut folder: impl FnMut(T, JsonValue) -> Result<T, JsonError>,
) -> Result<T, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    let mut state = initial.clone();
    for item in items {
        state = folder(
            state,
            JsonValue {
                inner: item.clone(),
            },
        )?;
    }
    Ok(state)
}

pub fn row_buffer_new(size: i64) -> RowBuffer {
    RowBuffer {
        bytes: Vec::with_capacity(size.max(0) as usize),
    }
}

pub fn csv_read_into(file: &mut File, buffer: &mut RowBuffer) -> Result<(), CsvError> {
    buffer.bytes.clear();
    file.inner.read_to_end(&mut buffer.bytes)?;
    Ok(())
}

pub fn csv_open_read<P: RuntimePath + ?Sized>(path: &P) -> Result<File, CsvError> {
    file_open_read(path).map_err(CsvError::from)
}

pub fn csv_parse_row(buffer: &RowBuffer) -> Result<Row, CsvError> {
    let text = std::str::from_utf8(&buffer.bytes)?;
    let Some(line) = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .nth(1)
        .or_else(|| text.lines().map(str::trim).find(|line| !line.is_empty()))
    else {
        return Err(CsvError::new("CSV buffer is empty"));
    };
    Ok(Row {
        fields: line
            .split(',')
            .map(|field| field.trim().to_string())
            .collect(),
    })
}

pub fn row_field_string(row: &Row, index: i64) -> Result<String, CsvError> {
    let index = usize::try_from(index).map_err(|_| CsvError::new("negative CSV field index"))?;
    row.fields
        .get(index)
        .cloned()
        .ok_or_else(|| CsvError::new(format!("CSV field index `{index}` is out of bounds")))
}
