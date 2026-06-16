use std::collections::HashMap;
use std::fmt;

use crate::async_runtime::{NativeAsyncPending, run_pending, spawn_tokio_native};
use crate::channel::{ChannelError, RssStream, stream_from_iterator};
use std::io::{BufRead, Read};
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
    body_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    content_type: Option<String>,
    body: Option<String>,
    timeout_ms: i64,
    attempts: i64,
    backoff_ms: i64,
}

pub fn http_request_debug_summary(request: &HttpRequest) -> String {
    format!(
        "HttpRequest {{ attempts: {}, backoff_ms: {}, body: {}, header_count: {}, method: {}, timeout_ms: {}, url: {} }}",
        request.attempts,
        request.backoff_ms,
        request.body.clone().unwrap_or_default(),
        request.headers.len(),
        request.method,
        request.timeout_ms,
        request.url
    )
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

pub fn image_debug_summary(image: &Image) -> String {
    format!(
        "Image {{ bytes: {:?}, height: {}, operations: [{}], width: {} }}",
        image.bytes,
        option_i64_debug_summary(image.height),
        image.operations.join(", "),
        option_i64_debug_summary(image.width)
    )
}

fn option_i64_debug_summary(value: Option<i64>) -> String {
    value
        .map(|value| format!("Some({value})"))
        .unwrap_or_else(|| "None".to_string())
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

impl From<crate::fs::FileError> for CsvError {
    fn from(error: crate::fs::FileError) -> Self {
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
        body_bytes: body.as_bytes().to_vec(),
    })
}

pub fn response_status(response: &Response) -> i64 {
    response.status
}

pub fn response_body(response: &Response) -> String {
    response.body.clone()
}

pub fn http_get(url: &str) -> Result<Response, HttpError> {
    run_pending(http_get_async(url))
}

pub fn http_get_async(url: &str) -> NativeAsyncPending<Result<Response, HttpError>> {
    let url = url.to_string();
    spawn_tokio_native(async move { http_request_async("GET", &url, Vec::new(), None, None).await })
}

pub fn http_request_json(url: &str, body: &str) -> HttpRequest {
    http_request_new(
        "POST",
        url,
        Some("application/json".to_string()),
        Some(body.to_string()),
    )
}

fn http_request_new(
    method: &str,
    url: &str,
    content_type: Option<String>,
    body: Option<String>,
) -> HttpRequest {
    HttpRequest {
        method: method.to_string(),
        url: url.to_string(),
        headers: Vec::new(),
        content_type,
        body,
        timeout_ms: 0,
        attempts: 1,
        backoff_ms: 0,
    }
}

pub fn http_request_with_timeout(mut request: HttpRequest, timeout_ms: i64) -> HttpRequest {
    request.timeout_ms = timeout_ms;
    request
}

pub fn http_request_with_retry(
    mut request: HttpRequest,
    attempts: i64,
    backoff_ms: i64,
) -> HttpRequest {
    request.attempts = attempts;
    request.backoff_ms = backoff_ms;
    request
}

pub fn http_request_with_header(mut request: HttpRequest, name: &str, value: &str) -> HttpRequest {
    request.headers.push((name.to_string(), value.to_string()));
    request
}

pub fn http_send_async(request: HttpRequest) -> NativeAsyncPending<Result<Response, HttpError>> {
    spawn_tokio_native(async move { http_request_retry_async(request).await })
}

pub fn http_get_timeout_async(
    url: &str,
    timeout_ms: i64,
) -> NativeAsyncPending<Result<Response, HttpError>> {
    let url = url.to_string();
    spawn_tokio_native(async move {
        http_request_timeout_async("GET", &url, Vec::new(), None, None, timeout_ms).await
    })
}

pub fn http_get_retry_async(
    url: &str,
    timeout_ms: i64,
    attempts: i64,
    backoff_ms: i64,
) -> NativeAsyncPending<Result<Response, HttpError>> {
    let mut request = http_request_new("GET", url, None, None);
    request.timeout_ms = timeout_ms;
    request.attempts = attempts;
    request.backoff_ms = backoff_ms;
    spawn_tokio_native(async move { http_request_retry_async(request).await })
}

pub fn http_post_json(url: &str, _body: &str) -> Result<Response, HttpError> {
    Err(HttpError {
        message: format!("HTTP client runtime is not configured for POST JSON {url}"),
    })
}

pub fn http_post_json_async(
    url: &str,
    body: &str,
) -> NativeAsyncPending<Result<Response, HttpError>> {
    let url = url.to_string();
    let body = body.to_string();
    spawn_tokio_native(async move {
        http_request_async(
            "POST",
            &url,
            Vec::new(),
            Some("application/json"),
            Some(body),
        )
        .await
    })
}

pub fn http_post_json_timeout_async(
    url: &str,
    body: &str,
    timeout_ms: i64,
) -> NativeAsyncPending<Result<Response, HttpError>> {
    let url = url.to_string();
    let body = body.to_string();
    spawn_tokio_native(async move {
        http_request_timeout_async(
            "POST",
            &url,
            Vec::new(),
            Some("application/json"),
            Some(body),
            timeout_ms,
        )
        .await
    })
}

pub fn http_post_json_retry_async(
    url: &str,
    body: &str,
    timeout_ms: i64,
    attempts: i64,
    backoff_ms: i64,
) -> NativeAsyncPending<Result<Response, HttpError>> {
    let mut request = http_request_new(
        "POST",
        url,
        Some("application/json".to_string()),
        Some(body.to_string()),
    );
    request.timeout_ms = timeout_ms;
    request.attempts = attempts;
    request.backoff_ms = backoff_ms;
    spawn_tokio_native(async move { http_request_retry_async(request).await })
}

pub fn http_post_json_bearer_retry_async(
    url: &str,
    body: &str,
    token: &str,
    timeout_ms: i64,
    attempts: i64,
    backoff_ms: i64,
) -> NativeAsyncPending<Result<Response, HttpError>> {
    let mut request = http_request_new(
        "POST",
        url,
        Some("application/json".to_string()),
        Some(body.to_string()),
    );
    request
        .headers
        .push(("Authorization".to_string(), format!("Bearer {token}")));
    request.timeout_ms = timeout_ms;
    request.attempts = attempts;
    request.backoff_ms = backoff_ms;
    spawn_tokio_native(async move { http_request_retry_async(request).await })
}

pub fn http_post_form(url: &str, _body: &str) -> Result<Response, HttpError> {
    Err(HttpError {
        message: format!("HTTP client runtime is not configured for POST form {url}"),
    })
}

pub fn http_post_form_async(
    url: &str,
    body: &str,
) -> NativeAsyncPending<Result<Response, HttpError>> {
    let url = url.to_string();
    let body = body.to_string();
    spawn_tokio_native(async move {
        http_request_async(
            "POST",
            &url,
            Vec::new(),
            Some("application/x-www-form-urlencoded"),
            Some(body),
        )
        .await
    })
}

async fn http_request_async(
    method: &str,
    url: &str,
    headers: Vec<(String, String)>,
    content_type: Option<&str>,
    body: Option<String>,
) -> Result<Response, HttpError> {
    let client = reqwest::Client::new();
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|error| HttpError {
        message: format!("invalid HTTP method `{method}`: {error}"),
    })?;
    let mut request = client.request(method, url);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    if let Some(content_type) = content_type {
        request = request.header(reqwest::header::CONTENT_TYPE, content_type);
    }
    if let Some(body) = body {
        request = request.body(body);
    }
    let response = request.send().await.map_err(|error| HttpError {
        message: format!("HTTP request failed for {url}: {error}"),
    })?;
    let status = i64::from(response.status().as_u16());
    let body = response.bytes().await.map_err(|error| HttpError {
        message: format!("failed to read HTTP response body from {url}: {error}"),
    })?;
    let body_bytes = body.to_vec();
    Ok(Response {
        status,
        body: String::from_utf8_lossy(&body_bytes).to_string(),
        body_bytes,
    })
}

async fn http_request_timeout_async(
    method: &str,
    url: &str,
    headers: Vec<(String, String)>,
    content_type: Option<&str>,
    body: Option<String>,
    timeout_ms: i64,
) -> Result<Response, HttpError> {
    if timeout_ms <= 0 {
        return http_request_async(method, url, headers, content_type, body).await;
    }
    tokio::time::timeout(
        std::time::Duration::from_millis(timeout_ms as u64),
        http_request_async(method, url, headers, content_type, body),
    )
    .await
    .map_err(|_| HttpError {
        message: format!("HTTP request to {url} timed out after {timeout_ms}ms"),
    })?
}

async fn http_request_retry_async(request: HttpRequest) -> Result<Response, HttpError> {
    let attempts = request.attempts.max(1);
    let backoff_ms = request.backoff_ms.max(0) as u64;
    let mut last_error = None;
    let mut last_response = None;
    for attempt in 0..attempts {
        match http_request_timeout_async(
            &request.method,
            &request.url,
            request.headers.clone(),
            request.content_type.as_deref(),
            request.body.clone(),
            request.timeout_ms,
        )
        .await
        {
            Ok(response) => {
                if response.status < 500 {
                    return Ok(response);
                }
                last_response = Some(response);
            }
            Err(error) => last_error = Some(error),
        }
        if attempt + 1 < attempts && backoff_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
        }
    }
    if let Some(response) = last_response {
        return Ok(response);
    }
    Err(last_error.unwrap_or_else(|| HttpError {
        message: format!("HTTP request to {} failed", request.url),
    }))
}

pub fn http_response_status(response: &Response) -> i64 {
    response.status
}

pub fn http_response_text(response: &Response) -> String {
    response.body.clone()
}

pub fn http_response_bytes(response: &Response) -> Vec<u8> {
    response.body_bytes.clone()
}

pub fn http_response_lines(response: &Response) -> Vec<String> {
    response.body.lines().map(str::to_string).collect()
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

pub fn json_parse(text: &str) -> Result<JsonValue, JsonError> {
    serde_json::from_str(text)
        .map(|inner| JsonValue { inner })
        .map_err(JsonError::from)
}

pub fn json_value(value: &str) -> JsonValue {
    serde_json::from_str(value)
        .map(|inner| JsonValue { inner })
        .expect("compiler-generated JSON literal should be valid JSON")
}

pub fn json_clone(value: &JsonValue) -> JsonValue {
    value.clone()
}

pub fn json_decode_value<T>(value: &JsonValue) -> Result<T, JsonError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(value.inner.clone()).map_err(JsonError::from)
}

pub fn json_decode_text<T>(text: &str) -> Result<T, JsonError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(text).map_err(JsonError::from)
}

pub fn json_parse_file<P: RuntimePath + ?Sized>(path: &P) -> Result<JsonValue, JsonError> {
    let text = std::fs::read_to_string(path.as_path())?;
    json_parse(&text)
}

pub fn json_quote_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a string to JSON cannot fail")
}

pub fn json_to_string(value: &JsonValue) -> String {
    serde_json::to_string(&value.inner).expect("serializing a JSON value cannot fail")
}

pub fn json_string_field(name: &str, value: &str) -> String {
    format!("{}:{}", json_quote_string(name), json_quote_string(value))
}

pub fn json_int_field(name: &str, value: i64) -> String {
    format!("{}:{value}", json_quote_string(name))
}

pub fn json_bool_field(name: &str, value: bool) -> String {
    format!("{}:{value}", json_quote_string(name))
}

pub fn json_raw_field(name: &str, value: &str) -> String {
    format!("{}:{value}", json_quote_string(name))
}

pub fn json_object(fields: &[String]) -> String {
    format!("{{{}}}", fields.join(","))
}

pub fn json_array(items: &[String]) -> String {
    format!("[{}]", items.join(","))
}

pub fn json_string_array(items: &[String]) -> String {
    let quoted = items
        .iter()
        .map(|item| json_quote_string(item))
        .collect::<Vec<_>>();
    json_array(&quoted)
}

pub fn json_strings(items: &[String]) -> JsonValue {
    JsonValue {
        inner: serde_json::Value::Array(
            items
                .iter()
                .map(|item| serde_json::Value::String(item.clone()))
                .collect(),
        ),
    }
}

pub fn json_values(items: &[JsonValue]) -> JsonValue {
    JsonValue {
        inner: serde_json::Value::Array(items.iter().map(|item| item.inner.clone()).collect()),
    }
}

pub fn toml_parse_file<P: RuntimePath + ?Sized>(path: &P) -> Result<JsonValue, JsonError> {
    let text = std::fs::read_to_string(path.as_path())?;
    let value = text
        .parse::<toml::Value>()
        .map_err(|error| JsonError::new(error.to_string()))?;
    let inner = serde_json::to_value(value)?;
    Ok(JsonValue { inner })
}

pub fn yaml_parse(text: &str) -> Result<JsonValue, JsonError> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text).map_err(|error| JsonError::new(error.to_string()))?;
    let inner = serde_json::to_value(value)?;
    Ok(JsonValue { inner })
}

pub fn yaml_parse_file<P: RuntimePath + ?Sized>(path: &P) -> Result<JsonValue, JsonError> {
    let text = std::fs::read_to_string(path.as_path())?;
    yaml_parse(&text)
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

pub fn json_field_optional(value: &JsonValue, name: &str) -> Result<Option<JsonValue>, JsonError> {
    match value.inner.get(name) {
        Some(field) if field.is_null() => Ok(None),
        Some(field) => Ok(Some(JsonValue {
            inner: field.clone(),
        })),
        None => Ok(None),
    }
}

pub fn json_field_optional_string(
    value: &JsonValue,
    name: &str,
) -> Result<Option<String>, JsonError> {
    let Some(field) = value.inner.get(name) else {
        return Ok(None);
    };
    if field.is_null() {
        return Ok(None);
    }
    let Some(text) = field.as_str() else {
        return Err(JsonError::new(format!(
            "JSON field `{name}` is not a string"
        )));
    };
    Ok(Some(text.to_string()))
}

pub fn json_field_optional_int(value: &JsonValue, name: &str) -> Result<Option<i64>, JsonError> {
    let Some(field) = value.inner.get(name) else {
        return Ok(None);
    };
    if field.is_null() {
        return Ok(None);
    }
    let Some(number) = field.as_i64() else {
        return Err(JsonError::new(format!(
            "JSON field `{name}` is not an integer"
        )));
    };
    Ok(Some(number))
}

pub fn json_field_optional_bool(value: &JsonValue, name: &str) -> Result<Option<bool>, JsonError> {
    let Some(field) = value.inner.get(name) else {
        return Ok(None);
    };
    if field.is_null() {
        return Ok(None);
    }
    let Some(flag) = field.as_bool() else {
        return Err(JsonError::new(format!(
            "JSON field `{name}` is not a boolean"
        )));
    };
    Ok(Some(flag))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum JsonPathPart {
    Field(String),
    Index(usize),
}

fn parse_json_path(path: &str) -> Result<Vec<JsonPathPart>, JsonError> {
    let path = path.strip_prefix("$.").unwrap_or(path);
    let path = path.strip_prefix('$').unwrap_or(path);
    if path.is_empty() {
        return Ok(Vec::new());
    }

    let chars = path.chars().collect::<Vec<_>>();
    let mut parts = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '.' {
            index += 1;
            continue;
        }

        if chars[index] == '[' {
            index += 1;
            let start = index;
            while index < chars.len() && chars[index] != ']' {
                index += 1;
            }
            if index == chars.len() {
                return Err(JsonError::new(format!(
                    "JSON path `{path}` has an unterminated array index"
                )));
            }
            let raw_index = chars[start..index].iter().collect::<String>();
            let item_index = raw_index.parse::<usize>().map_err(|_| {
                JsonError::new(format!(
                    "JSON path `{path}` has invalid array index `{raw_index}`"
                ))
            })?;
            parts.push(JsonPathPart::Index(item_index));
            index += 1;
            continue;
        }

        let start = index;
        while index < chars.len() && chars[index] != '.' && chars[index] != '[' {
            index += 1;
        }
        let field = chars[start..index].iter().collect::<String>();
        if field.is_empty() {
            return Err(JsonError::new(format!(
                "JSON path `{path}` contains an empty field"
            )));
        }
        parts.push(JsonPathPart::Field(field));
    }

    Ok(parts)
}

pub fn json_value_at(value: &JsonValue, path: &str) -> Result<JsonValue, JsonError> {
    let mut current = &value.inner;
    for part in parse_json_path(path)? {
        match part {
            JsonPathPart::Field(name) => {
                let Some(next) = current.get(&name) else {
                    return Err(JsonError::new(format!(
                        "missing JSON field `{name}` at path `{path}`"
                    )));
                };
                current = next;
            }
            JsonPathPart::Index(index) => {
                let Some(items) = current.as_array() else {
                    return Err(JsonError::new(format!(
                        "JSON path `{path}` expected an array before index `{index}`"
                    )));
                };
                let Some(next) = items.get(index) else {
                    return Err(JsonError::new(format!(
                        "JSON array index `{index}` is out of bounds at path `{path}`"
                    )));
                };
                current = next;
            }
        }
    }
    Ok(JsonValue {
        inner: current.clone(),
    })
}

pub fn json_at(value: &JsonValue, path: &str) -> Result<JsonValue, JsonError> {
    json_value_at(value, path)
}

pub fn json_at_or(value: &JsonValue, path: &str, fallback: &JsonValue) -> JsonValue {
    json_value_at(value, path).unwrap_or_else(|_| fallback.clone())
}

pub fn json_at_string(value: &JsonValue, path: &str) -> Result<String, JsonError> {
    let item = json_value_at(value, path)?;
    json_as_string(&item)
}

pub fn json_at_int(value: &JsonValue, path: &str) -> Result<i64, JsonError> {
    let item = json_value_at(value, path)?;
    json_as_int(&item)
}

pub fn json_at_bool(value: &JsonValue, path: &str) -> Result<bool, JsonError> {
    let item = json_value_at(value, path)?;
    json_as_bool(&item)
}

pub fn json_at_optional(value: &JsonValue, path: &str) -> Result<Option<JsonValue>, JsonError> {
    match json_value_at(value, path) {
        Ok(item) if item.inner.is_null() => Ok(None),
        Ok(item) => Ok(Some(item)),
        Err(_) => Ok(None),
    }
}

pub fn json_at_optional_string(value: &JsonValue, path: &str) -> Result<Option<String>, JsonError> {
    match json_value_at(value, path) {
        Ok(item) if item.inner.is_null() => Ok(None),
        Ok(item) => json_as_string(&item).map(Some),
        Err(_) => Ok(None),
    }
}

pub fn json_at_optional_int(value: &JsonValue, path: &str) -> Result<Option<i64>, JsonError> {
    match json_value_at(value, path) {
        Ok(item) if item.inner.is_null() => Ok(None),
        Ok(item) => json_as_int(&item).map(Some),
        Err(_) => Ok(None),
    }
}

pub fn json_at_optional_bool(value: &JsonValue, path: &str) -> Result<Option<bool>, JsonError> {
    match json_value_at(value, path) {
        Ok(item) if item.inner.is_null() => Ok(None),
        Ok(item) => json_as_bool(&item).map(Some),
        Err(_) => Ok(None),
    }
}

pub fn json_at_string_or(value: &JsonValue, path: &str, fallback: &str) -> String {
    json_at_string(value, path).unwrap_or_else(|_| fallback.to_string())
}

pub fn json_at_int_or(value: &JsonValue, path: &str, fallback: i64) -> i64 {
    json_at_int(value, path).unwrap_or(fallback)
}

pub fn json_at_bool_or(value: &JsonValue, path: &str, fallback: bool) -> bool {
    json_at_bool(value, path).unwrap_or(fallback)
}

pub fn json_at_to_string(value: &JsonValue, path: &str) -> Result<String, JsonError> {
    let item = json_value_at(value, path)?;
    Ok(json_to_string(&item))
}

pub fn json_at_to_string_or(value: &JsonValue, path: &str, fallback: &str) -> String {
    json_at_to_string(value, path).unwrap_or_else(|_| fallback.to_string())
}

pub fn json_string_at(text: &str, path: &str) -> Result<String, JsonError> {
    let value = json_parse(text)?;
    let item = json_value_at(&value, path)?;
    json_as_string(&item)
}

pub fn json_int_at(text: &str, path: &str) -> Result<i64, JsonError> {
    let value = json_parse(text)?;
    let item = json_value_at(&value, path)?;
    json_as_int(&item)
}

pub fn json_bool_at(text: &str, path: &str) -> Result<bool, JsonError> {
    let value = json_parse(text)?;
    let item = json_value_at(&value, path)?;
    json_as_bool(&item)
}

pub fn json_to_string_at(text: &str, path: &str) -> Result<String, JsonError> {
    let value = json_parse(text)?;
    let item = json_value_at(&value, path)?;
    Ok(json_to_string(&item))
}

pub fn json_string_at_or(text: &str, path: &str, fallback: &str) -> String {
    json_string_at(text, path).unwrap_or_else(|_| fallback.to_string())
}

pub fn json_int_at_or(text: &str, path: &str, fallback: i64) -> i64 {
    json_int_at(text, path).unwrap_or(fallback)
}

pub fn json_bool_at_or(text: &str, path: &str, fallback: bool) -> bool {
    json_bool_at(text, path).unwrap_or(fallback)
}

pub fn json_to_string_at_or(text: &str, path: &str, fallback: &str) -> String {
    json_to_string_at(text, path).unwrap_or_else(|_| fallback.to_string())
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

pub fn json_kind(value: &JsonValue) -> String {
    match &value.inner {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(number) if number.is_i64() || number.is_u64() => "int",
        serde_json::Value::Number(_) => "float",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
    .to_string()
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

pub fn csv_rows<P: RuntimePath + ?Sized>(
    path: &P,
    buffer_size: i64,
) -> Result<RssStream<Row>, ChannelError> {
    let file = std::fs::File::open(path.as_path())
        .map_err(|error| ChannelError::new(&format!("CSV row stream open failed: {error}")))?;
    let capacity = buffer_size.max(1) as usize;
    Ok(stream_from_iterator(CsvRowsIterator {
        reader: std::io::BufReader::with_capacity(capacity, file),
        line: String::new(),
        skipped_header: false,
        done: false,
    }))
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

struct CsvRowsIterator {
    reader: std::io::BufReader<std::fs::File>,
    line: String,
    skipped_header: bool,
    done: bool,
}

impl Iterator for CsvRowsIterator {
    type Item = Result<Row, ChannelError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        loop {
            self.line.clear();
            match self.reader.read_line(&mut self.line) {
                Ok(0) => {
                    self.done = true;
                    return None;
                }
                Ok(_) => {
                    let line = self.line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if !self.skipped_header {
                        self.skipped_header = true;
                        continue;
                    }
                    return Some(Ok(parse_csv_row_line(line)));
                }
                Err(error) => {
                    self.done = true;
                    return Some(Err(ChannelError::new(&format!(
                        "CSV row stream read failed: {error}"
                    ))));
                }
            }
        }
    }
}

fn parse_csv_row_line(line: &str) -> Row {
    Row {
        fields: line
            .split(',')
            .map(|field| field.trim().to_string())
            .collect(),
    }
}

pub fn row_field_string(row: &Row, index: i64) -> Result<String, CsvError> {
    let index = usize::try_from(index).map_err(|_| CsvError::new("negative CSV field index"))?;
    row.fields
        .get(index)
        .cloned()
        .ok_or_else(|| CsvError::new(format!("CSV field index `{index}` is out of bounds")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_runtime::Executor;

    #[test]
    fn http_get_async_uses_reqwest_tokio_io() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        let addr = listener.local_addr().expect("listener addr should exist");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("client should connect");
            let mut request = [0_u8; 1024];
            let read = stream.read(&mut request).expect("request should read");
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("GET /health HTTP/1.1"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nready",
                )
                .expect("response should write");
        });

        let mut executor = Executor::new();
        let response = executor
            .run_pending(http_get_async(&format!("http://{addr}/health")))
            .expect("async HTTP GET should succeed");

        assert_eq!(response.status, 200);
        assert_eq!(response.body, "ready");
        handle.join().expect("server thread should finish");
    }

    #[test]
    fn http_get_sync_uses_reqwest_tokio_io_and_preserves_bytes() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        let addr = listener.local_addr().expect("listener addr should exist");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("client should connect");
            let mut request = [0_u8; 1024];
            let read = stream.read(&mut request).expect("request should read");
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("GET /bytes HTTP/1.1"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n\x00\xffA",
                )
                .expect("response should write");
        });

        let response =
            http_get(&format!("http://{addr}/bytes")).expect("sync HTTP GET should succeed");

        assert_eq!(response.status, 200);
        assert_eq!(http_response_bytes(&response), vec![0, 255, 65]);
        handle.join().expect("server thread should finish");
    }

    #[test]
    fn http_post_json_async_sends_body_through_reqwest_tokio_io() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        let addr = listener.local_addr().expect("listener addr should exist");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("client should connect");
            let mut request = [0_u8; 2048];
            let read = stream.read(&mut request).expect("request should read");
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("POST /users HTTP/1.1"));
            assert!(request.contains("content-type: application/json"));
            assert!(request.contains(r#"{"name":"rss"}"#));
            stream
                .write_all(
                    b"HTTP/1.1 201 Created\r\nContent-Length: 7\r\nConnection: close\r\n\r\ncreated",
                )
                .expect("response should write");
        });

        let mut executor = Executor::new();
        let response = executor
            .run_pending(http_post_json_async(
                &format!("http://{addr}/users"),
                r#"{"name":"rss"}"#,
            ))
            .expect("async HTTP POST should succeed");

        assert_eq!(response.status, 201);
        assert_eq!(response.body, "created");
        handle.join().expect("server thread should finish");
    }

    #[test]
    fn http_send_async_returns_final_server_error_response_body() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        let addr = listener.local_addr().expect("listener addr should exist");
        let handle = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().expect("client should connect");
                let mut request = [0_u8; 2048];
                let read = stream.read(&mut request).expect("request should read");
                let request = String::from_utf8_lossy(&request[..read]);
                assert!(request.starts_with("POST /agent HTTP/1.1"));
                assert!(request.contains("authorization: Bearer test_key"));
                stream
                    .write_all(
                        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"error\":\"codex\"}",
                    )
                    .expect("response should write");
            }
        });

        let mut request = http_request_json(&format!("http://{addr}/agent"), r#"{"messages":[]}"#);
        request = http_request_with_header(request, "Authorization", "Bearer test_key");
        request = http_request_with_retry(request, 2, 0);

        let mut executor = Executor::new();
        let response = executor
            .run_pending(http_send_async(request))
            .expect("HTTP status response should be returned to caller");

        assert_eq!(response.status, 500);
        assert_eq!(response.body, r#"{"error":"codex"}"#);
        handle.join().expect("server thread should finish");
    }

    #[test]
    fn http_get_async_reports_invalid_url_errors() {
        let mut executor = Executor::new();
        let error = executor
            .run_pending(http_get_async("not a url"))
            .expect_err("invalid URL should be reported");

        assert!(error.message.contains("HTTP request failed"));
    }

    #[test]
    fn yaml_parse_reuses_json_value_accessors() {
        let value = yaml_parse("name: rss\nports:\n  - 8080\n  - 9090\n")
            .expect("YAML should parse into a JsonValue");

        let name = json_field(&value, "name").expect("name field exists");
        assert_eq!(json_as_string(&name).expect("name is a string"), "rss");

        let ports = json_field(&value, "ports").expect("ports field exists");
        assert_eq!(json_array_len(&ports).expect("ports is an array"), 2);
    }

    #[test]
    fn json_path_helpers_read_nested_fields_and_arrays() {
        let text = r#"{
            "choices": [
                {
                    "message": {
                        "content": "done",
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "function": {
                                    "name": "write_file",
                                    "arguments": "{\"path\":\"hello.txt\"}"
                                }
                            }
                        ]
                    }
                }
            ]
        }"#;
        let value = json_parse(text).expect("JSON should parse");
        let literal = json_value(r#"{"ready":true,"count":2}"#);
        assert!(!json_bool_at_or(
            text,
            "choices[0].message.tool_calls[0].function.name",
            false
        ));
        assert_eq!(json_int_at_or(&json_to_string(&literal), "count", 0), 2);
        assert!(json_bool_at_or(&json_to_string(&literal), "ready", false));
        assert_eq!(
            json_string_array(&["a".to_string(), "b".to_string()]),
            r#"["a","b"]"#
        );

        assert_eq!(
            json_as_string(
                &json_value_at(&value, "choices[0].message.tool_calls[0].function.name")
                    .expect("path should resolve")
            )
            .expect("path value should be string"),
            "write_file"
        );
        assert_eq!(
            json_string_at(text, "$.choices[0].message.content")
                .expect("string path should resolve"),
            "done"
        );
        assert_eq!(
            json_string_at_or(text, "choices[0].message.missing", "fallback"),
            "fallback"
        );
        let message = json_value_at(&value, "choices[0].message").expect("message should resolve");
        assert_eq!(
            json_to_string(&json_at_or(&value, "choices[2].message", &message)),
            json_to_string(&message)
        );
        let serialized = json_to_string(&message);
        assert!(
            serialized.contains(r#""tool_calls""#),
            "serialized message should preserve tool calls"
        );
        assert!(
            json_value_at(&value, "choices[2].message")
                .expect_err("missing index should fail")
                .message
                .contains("out of bounds")
        );
    }

    #[test]
    fn yaml_parse_reports_error_text() {
        let error = yaml_parse("name: [unterminated\n").expect_err("invalid YAML should error");
        assert!(
            !json_error_message(&error).is_empty(),
            "YAML parse error should carry a message"
        );
    }
}
