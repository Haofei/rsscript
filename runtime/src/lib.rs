#![forbid(unsafe_code)]

use std::cell::{RefCell, RefMut};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::io::{Read, Write};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::str::Utf8Error;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError, Weak};

pub const RUNTIME_DIAGNOSTIC_PREFIX: &str = "RSSCRIPT_RUNTIME_DIAGNOSTIC:";

pub trait ManagedValue {}

impl<T: 'static> ManagedValue for T {}

pub trait Resource {}

pub fn install_runtime_diagnostic_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        if let Some(payload) = panic_payload_as_str(info.payload())
            && payload.starts_with(RUNTIME_DIAGNOSTIC_PREFIX)
        {
            eprintln!("{payload}");
            return;
        }
        eprintln!("{info}");
    }));
}

fn panic_payload_as_str(payload: &(dyn std::any::Any + Send)) -> Option<&str> {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

pub struct File {
    inner: std::fs::File,
}

impl Resource for File {}

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
    bytes: Vec<u8>,
    width: Option<i64>,
    height: Option<i64>,
    operations: Vec<&'static str>,
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

pub trait RuntimePath {
    fn as_path(&self) -> &std::path::Path;
}

impl RuntimePath for PathBuf {
    fn as_path(&self) -> &std::path::Path {
        self.as_path()
    }
}

impl RuntimePath for String {
    fn as_path(&self) -> &std::path::Path {
        std::path::Path::new(self)
    }
}

impl RuntimePath for str {
    fn as_path(&self) -> &std::path::Path {
        std::path::Path::new(self)
    }
}

impl<T: RuntimePath + ?Sized> RuntimePath for &T {
    fn as_path(&self) -> &std::path::Path {
        (*self).as_path()
    }
}

pub fn path_from_string(value: &str) -> PathBuf {
    PathBuf::from(value)
}

pub trait RuntimeBytes {
    fn as_bytes_slice(&self) -> &[u8];
}

impl RuntimeBytes for Vec<u8> {
    fn as_bytes_slice(&self) -> &[u8] {
        self.as_slice()
    }
}

impl RuntimeBytes for String {
    fn as_bytes_slice(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl RuntimeBytes for str {
    fn as_bytes_slice(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl<T: RuntimeBytes + ?Sized> RuntimeBytes for &T {
    fn as_bytes_slice(&self) -> &[u8] {
        (*self).as_bytes_slice()
    }
}

pub fn file_open<P: RuntimePath + ?Sized>(path: &P) -> std::io::Result<File> {
    file_open_read(path)
}

pub fn file_open_read<P: RuntimePath + ?Sized>(path: &P) -> std::io::Result<File> {
    std::fs::File::open(path.as_path()).map(|inner| File { inner })
}

pub fn file_open_write<P: RuntimePath + ?Sized>(path: &P) -> std::io::Result<File> {
    std::fs::File::create(path.as_path()).map(|inner| File { inner })
}

pub fn file_read_all(file: &mut File) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.inner.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub fn file_read_into(file: &mut File, buffer: &mut Vec<u8>) -> std::io::Result<bool> {
    buffer.clear();
    let bytes_read = file.inner.read_to_end(buffer)?;
    Ok(bytes_read > 0)
}

pub fn file_write<B: RuntimeBytes + ?Sized>(file: &mut File, data: &B) -> std::io::Result<()> {
    file.inner.write_all(data.as_bytes_slice())
}

pub fn os_close(fd: i64) {
    let _ = fd;
}

pub fn list_new<T>() -> Vec<T> {
    Vec::new()
}

pub fn list_push<T: Clone>(list: &mut Vec<T>, value: &T) {
    list.push(value.clone());
}

pub fn list_len<T>(list: &[T]) -> i64 {
    list.len() as i64
}

pub fn list_get<T: Clone>(list: &[T], index: i64) -> T {
    list[index as usize].clone()
}

pub fn list_consume<T>(list: Vec<T>) {
    drop(list);
}

pub fn buffer_new(size: i64) -> Vec<u8> {
    Vec::with_capacity(size.max(0) as usize)
}

pub fn buffer_clear(buffer: &mut Vec<u8>) {
    buffer.clear();
}

pub fn buffer_consume(buffer: Vec<u8>) {
    drop(buffer);
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

#[derive(Clone)]
pub struct Managed<T> {
    inner: Arc<RwLock<T>>,
    origin_span: Option<SourceSpan>,
}

// Compatibility alias for generated packages produced before the v0.5 runtime
// surface settled on `Managed<T>`.
pub type Gc<T> = Managed<T>;

impl<T> Managed<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(RwLock::new(value)),
            origin_span: None,
        }
    }

    pub fn new_at(value: T, span: SourceSpan) -> Self {
        Self {
            inner: Arc::new(RwLock::new(value)),
            origin_span: Some(span),
        }
    }

    pub fn try_read(&self) -> Result<ManagedRead<'_, T>, RuntimeError> {
        self.inner
            .try_read()
            .map(ManagedRead)
            .map_err(managed_read_error)
    }

    pub fn try_write(&self) -> Result<ManagedWrite<'_, T>, RuntimeError> {
        self.inner
            .try_write()
            .map(ManagedWrite)
            .map_err(managed_write_error)
    }

    pub fn try_read_at(&self, span: SourceSpan) -> Result<ManagedRead<'_, T>, RuntimeError> {
        self.try_read().map_err(|error| error.with_span(span))
    }

    pub fn try_write_at(&self, span: SourceSpan) -> Result<ManagedWrite<'_, T>, RuntimeError> {
        self.try_write().map_err(|error| error.with_span(span))
    }

    pub fn read(&self) -> ManagedRead<'_, T> {
        match self.try_read() {
            Ok(value) => value,
            Err(error) => panic_runtime_error(error),
        }
    }

    pub fn write(&self) -> ManagedWrite<'_, T> {
        match self.try_write() {
            Ok(value) => value,
            Err(error) => panic_runtime_error(error),
        }
    }

    pub fn ptr_eq(left: &Self, right: &Self) -> bool {
        Arc::ptr_eq(&left.inner, &right.inner)
    }

    pub fn origin_span(&self) -> Option<&SourceSpan> {
        self.origin_span.as_ref()
    }
}

#[derive(Clone)]
pub struct WeakManaged<T> {
    inner: Weak<RwLock<T>>,
    origin_span: Option<SourceSpan>,
}

impl<T> WeakManaged<T> {
    pub fn upgrade(&self) -> Option<Managed<T>> {
        self.inner.upgrade().map(|inner| Managed {
            inner,
            origin_span: self.origin_span.clone(),
        })
    }

    pub fn origin_span(&self) -> Option<&SourceSpan> {
        self.origin_span.as_ref()
    }
}

impl<T: fmt::Debug> fmt::Debug for WeakManaged<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.upgrade() {
            Some(value) => formatter
                .debug_tuple("WeakManaged")
                .field(&value.read())
                .finish(),
            None => formatter.write_str("WeakManaged(<dropped>)"),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Managed<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Managed")
            .field(&self.read())
            .finish()
    }
}

pub fn manage<T>(value: T) -> Managed<T> {
    Managed::new(value)
}

pub fn manage_at<T>(value: T, span: SourceSpan) -> Managed<T> {
    Managed::new_at(value, span)
}

pub fn weak<T>(value: &Managed<T>) -> WeakManaged<T> {
    WeakManaged {
        inner: Arc::downgrade(&value.inner),
        origin_span: value.origin_span.clone(),
    }
}

pub fn unwrap_runtime<T>(result: Result<T, RuntimeError>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic_runtime_error(error),
    }
}

pub fn log_write(message: &str) {
    println!("{message}");
}

pub fn assert_equal(left: &str, right: &str) {
    assert_eq!(left, right);
}

pub fn assert_equal_int(left: i64, right: i64) {
    assert_eq!(left, right);
}

pub fn assert_equal_bool(left: bool, right: bool) {
    assert_eq!(left, right);
}

pub struct ManagedRead<'a, T>(RwLockReadGuard<'a, T>);

pub type GcRead<'a, T> = ManagedRead<'a, T>;

impl<T> Deref for ManagedRead<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: fmt::Debug> fmt::Debug for ManagedRead<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, formatter)
    }
}

pub struct ManagedWrite<'a, T>(RwLockWriteGuard<'a, T>);

pub type GcWrite<'a, T> = ManagedWrite<'a, T>;

impl<T> Deref for ManagedWrite<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for ManagedWrite<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: fmt::Debug> fmt::Debug for ManagedWrite<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeErrorKind {
    ManagedReadConflict,
    ManagedWriteConflict,
    ResourcePoolBorrowConflict,
    ResourcePoolEmpty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
    pub message: String,
    pub span: Option<SourceSpan>,
}

impl RuntimeError {
    pub fn with_span(mut self, span: SourceSpan) -> Self {
        self.span = Some(span);
        self
    }

    pub fn diagnostic_json(&self) -> String {
        let span = self
            .span
            .clone()
            .unwrap_or_else(|| SourceSpan::new("<runtime>", 1, 1, 1));
        serde_json::json!({
            "code": "RS1201",
            "severity": "error",
            "summary": format!("RSScript runtime error: {}", self.message),
            "file": span.file,
            "line": span.line,
            "column": span.column,
            "length": span.length,
            "label": self.message,
            "kind": self.kind.as_str(),
        })
        .to_string()
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for RuntimeError {}

impl RuntimeErrorKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::ManagedReadConflict => "managed_read_conflict",
            Self::ManagedWriteConflict => "managed_write_conflict",
            Self::ResourcePoolBorrowConflict => "resource_pool_borrow_conflict",
            Self::ResourcePoolEmpty => "resource_pool_empty",
        }
    }
}

fn panic_runtime_error(error: RuntimeError) -> ! {
    panic!("{}{}", RUNTIME_DIAGNOSTIC_PREFIX, error.diagnostic_json())
}

fn managed_read_error<T>(error: TryLockError<RwLockReadGuard<'_, T>>) -> RuntimeError {
    let message = match error {
        TryLockError::WouldBlock => "managed value is already being written",
        TryLockError::Poisoned(_) => "managed value lock is poisoned after a previous panic",
    };
    RuntimeError {
        kind: RuntimeErrorKind::ManagedReadConflict,
        message: message.to_string(),
        span: None,
    }
}

fn managed_write_error<T>(error: TryLockError<RwLockWriteGuard<'_, T>>) -> RuntimeError {
    let message = match error {
        TryLockError::WouldBlock => "managed value is already being read or written",
        TryLockError::Poisoned(_) => "managed value lock is poisoned after a previous panic",
    };
    RuntimeError {
        kind: RuntimeErrorKind::ManagedWriteConflict,
        message: message.to_string(),
        span: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpan {
    pub file: &'static str,
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

impl SourceSpan {
    pub const fn new(file: &'static str, line: usize, column: usize, length: usize) -> Self {
        Self {
            file,
            line,
            column,
            length,
        }
    }
}

#[derive(Debug)]
pub struct ResourcePool<T: Resource> {
    values: RefCell<Vec<T>>,
}

impl<T: Resource> ResourcePool<T> {
    pub fn new(values: Vec<T>) -> Self {
        Self {
            values: RefCell::new(values),
        }
    }

    pub fn from_factory<F>(max_size: i64, mut create: F) -> Self
    where
        F: FnMut() -> T,
    {
        let count = max_size.max(0) as usize;
        let values = (0..count).map(|_| create()).collect();
        Self::new(values)
    }

    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    pub fn push(&self, value: T) {
        self.values.borrow_mut().push(value);
    }

    pub fn try_borrow(&self) -> Result<ResourceLease<'_, T>, RuntimeError> {
        let values = self.values.try_borrow_mut().map_err(|_| RuntimeError {
            kind: RuntimeErrorKind::ResourcePoolBorrowConflict,
            message: "resource pool is already borrowed".to_string(),
            span: None,
        })?;
        if values.is_empty() {
            return Err(RuntimeError {
                kind: RuntimeErrorKind::ResourcePoolEmpty,
                message: "resource pool has no available resources".to_string(),
                span: None,
            });
        }
        Ok(ResourceLease { values, index: 0 })
    }

    pub fn try_borrow_at(&self, span: SourceSpan) -> Result<ResourceLease<'_, T>, RuntimeError> {
        self.try_borrow().map_err(|error| error.with_span(span))
    }

    pub fn borrow_at(pool: &Self, span: SourceSpan) -> Result<ResourceLease<'_, T>, RuntimeError> {
        pool.try_borrow_at(span)
    }

    pub fn borrow(&self) -> ResourceLease<'_, T> {
        self.try_borrow()
            .expect("RSScript resource pool conflict should be reported through diagnostics")
    }
}

pub struct ResourceLease<'a, T: Resource> {
    values: RefMut<'a, Vec<T>>,
    index: usize,
}

impl<T: Resource> Deref for ResourceLease<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.values[self.index]
    }
}

impl<T: Resource> DerefMut for ResourceLease<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.values[self.index]
    }
}

impl<T: Resource> fmt::Debug for ResourceLease<'_, T>
where
    T: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ResourceLease")
            .field(&self.values[self.index])
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{Resource, ResourcePool, RuntimeErrorKind, manage};

    #[derive(Debug)]
    struct FileHandle(i32);

    impl Resource for FileHandle {}

    #[test]
    fn manage_wraps_value_in_managed_handle() {
        let value = manage(String::from("cached"));

        assert_eq!(&*value.read(), "cached");
    }

    #[test]
    fn weak_handles_upgrade_while_managed_value_is_alive() {
        let value = manage(String::from("cached"));
        let weak = super::weak(&value);

        assert_eq!(
            &*weak.upgrade().expect("value should still be live").read(),
            "cached"
        );

        drop(value);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn managed_aliases_observe_mutation() {
        let left = manage(String::from("cached"));
        let right = left.clone();

        right.write().push_str("-updated");

        assert_eq!(&*left.read(), "cached-updated");
        assert!(super::Managed::ptr_eq(&left, &right));
    }

    #[test]
    fn managed_handles_are_thread_shareable_for_send_values() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<super::Managed<String>>();
        assert_send_sync::<super::WeakManaged<String>>();

        let value = manage(String::from("cached"));
        let worker_value = value.clone();
        std::thread::spawn(move || {
            worker_value.write().push_str("-from-thread");
        })
        .join()
        .expect("managed value should move across thread");

        assert_eq!(&*value.read(), "cached-from-thread");
    }

    #[test]
    fn managed_conflicts_report_runtime_errors() {
        let value = manage(String::from("cached"));
        let _write = value.try_write().expect("initial write should succeed");
        let error = value
            .try_read()
            .expect_err("read should conflict with write");

        assert_eq!(error.kind, RuntimeErrorKind::ManagedReadConflict);
    }

    #[test]
    fn managed_handles_keep_origin_span() {
        let span = super::SourceSpan::new("cache.rss", 3, 9, 6);
        let value = super::manage_at(String::from("cached"), span.clone());

        assert_eq!(value.origin_span(), Some(&span));
    }

    #[test]
    fn path_runtime_hook_builds_pathbuf_from_string() {
        let path = super::path_from_string("rsscript-path.txt");

        assert_eq!(path, std::path::PathBuf::from("rsscript-path.txt"));
    }

    #[test]
    fn managed_conflicts_can_attach_operation_span() {
        let value = manage(String::from("cached"));
        let _write = value.try_write().expect("initial write should succeed");
        let span = super::SourceSpan::new("cache.rss", 4, 12, 5);
        let error = value
            .try_read_at(span.clone())
            .expect_err("read should conflict with write");

        assert_eq!(error.span, Some(span));
    }

    #[test]
    fn resource_pool_borrows_scoped_resource() {
        let pool = ResourcePool::new(vec![FileHandle(7)]);
        let lease = pool.borrow();

        assert_eq!(lease.0, 7);
    }

    #[test]
    fn resource_pool_reports_empty_pool() {
        let pool = ResourcePool::<FileHandle>::empty();
        let error = pool.try_borrow().expect_err("empty pool should error");

        assert_eq!(error.kind, RuntimeErrorKind::ResourcePoolEmpty);
    }

    #[test]
    fn resource_pool_reports_borrow_conflict() {
        let pool = ResourcePool::new(vec![FileHandle(7)]);
        let _lease = pool.try_borrow().expect("initial borrow should succeed");
        let error = pool
            .try_borrow()
            .expect_err("second borrow should conflict");

        assert_eq!(error.kind, RuntimeErrorKind::ResourcePoolBorrowConflict);
    }

    #[test]
    fn file_runtime_hooks_write_and_read_bytes() {
        let path =
            std::env::temp_dir().join(format!("rsscript-runtime-file-{}.txt", std::process::id()));

        {
            let mut file = super::file_open_write(&path).expect("file should open for write");
            super::file_write(&mut file, &"hello file").expect("write should succeed");
        }

        let mut file = super::file_open_read(&path).expect("file should open for read");
        let bytes = super::file_read_all(&mut file).expect("read should succeed");

        assert_eq!(bytes, b"hello file");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn file_runtime_hooks_read_into_reusable_buffer() {
        let path = std::env::temp_dir().join(format!(
            "rsscript-runtime-file-buffer-{}.txt",
            std::process::id()
        ));

        {
            let mut file = super::file_open_write(&path).expect("file should open for write");
            super::file_write(&mut file, &"hello buffer").expect("write should succeed");
        }

        let mut file = super::file_open_read(&path).expect("file should open for read");
        let mut buffer = super::buffer_new(64);
        assert!(super::file_read_into(&mut file, &mut buffer).expect("read should succeed"));
        assert_eq!(buffer, b"hello buffer");
        assert!(!super::file_read_into(&mut file, &mut buffer).expect("EOF should succeed"));
        assert!(buffer.is_empty());
        super::buffer_clear(&mut buffer);
        assert!(buffer.is_empty());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn json_runtime_hooks_parse_nested_fields() {
        let value =
            super::json_parse(r#"{"profile":{"name":"RSScript"}}"#).expect("JSON should parse");
        let profile = super::json_field(&value, "profile").expect("profile field should exist");
        let name =
            super::json_field_string(&profile, "name").expect("name field should be a string");

        assert_eq!(name, "RSScript");
    }

    #[test]
    fn csv_runtime_hooks_read_and_parse_row() {
        let path =
            std::env::temp_dir().join(format!("rsscript-runtime-csv-{}.csv", std::process::id()));

        {
            let mut file = super::file_open_write(&path).expect("file should open for write");
            super::file_write(&mut file, &"name,amount\nRSScript,42\n")
                .expect("write should succeed");
        }

        let mut file = super::file_open_read(&path).expect("file should open for read");
        let mut buffer = super::row_buffer_new(4096);
        super::csv_read_into(&mut file, &mut buffer).expect("CSV read should succeed");
        let row = super::csv_parse_row(&buffer).expect("CSV row should parse");
        let name = super::row_field_string(&row, 0).expect("field should exist");

        assert_eq!(name, "RSScript");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn counter_runtime_hooks_mutate_counter() {
        let mut counter = super::counter_new(1);

        super::counter_add(&mut counter, 2);

        assert_eq!(super::counter_value(&counter), 3);
    }

    #[test]
    fn consume_runtime_hooks_take_collections() {
        super::list_consume(vec![1_i64, 2, 3]);
        super::buffer_consume(b"bytes".to_vec());
        super::os_close(0);
    }

    #[test]
    fn cache_runtime_hooks_insert_and_lookup_values() {
        let mut cache = super::cache_new();

        super::cache_insert(&mut cache, "/users", "handled /users");

        assert_eq!(super::cache_lookup(&cache, "/users"), "handled /users");
        assert_eq!(super::cache_get(&cache).bytes, b"handled /users");
    }

    #[test]
    fn interpreter_runtime_hooks_link_environment_function_cycle() {
        let root = super::manage(super::environment_root());
        let child = super::manage(super::environment_child(&root));
        let function = super::manage(super::function_object_new(&child));
        let mut child_handle = child.clone();

        super::environment_bind_function(&mut child_handle, &function);

        assert!(super::environment_has_parent(&child));
        assert!(super::environment_has_function(&child));
        assert!(super::function_object_has_closure(&function));

        drop(child);
        drop(child_handle);

        assert!(!super::function_object_has_closure(&function));
    }

    #[test]
    fn image_runtime_hooks_load_transform_and_save() {
        let input = std::env::temp_dir().join(format!(
            "rsscript-runtime-image-input-{}.bin",
            std::process::id()
        ));
        let output = std::env::temp_dir().join(format!(
            "rsscript-runtime-image-output-{}.bin",
            std::process::id()
        ));
        std::fs::write(&input, b"image-bytes").expect("input image should be writable");

        let mut image = super::image_load(&input).expect("image should load");
        super::image_resize(&mut image, 320, 240);
        super::image_normalize(&mut image);
        super::image_sharpen(&mut image);
        super::image_save(&image, &output).expect("image should save");

        let saved = std::fs::read_to_string(&output).expect("saved image should be readable");
        assert!(saved.contains("rsscript-image-ops:load,resize,normalize,sharpen;size=320x240"));

        let _ = std::fs::remove_file(input);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn image_cache_runtime_hooks_retain_managed_images_with_capacity() {
        let mut cache = super::image_cache_new(1);
        let first = super::manage(super::Image {
            bytes: b"first".to_vec(),
            width: None,
            height: None,
            operations: vec!["test"],
        });
        let second = super::manage(super::Image {
            bytes: b"second".to_vec(),
            width: None,
            height: None,
            operations: vec!["test"],
        });

        super::image_cache_store(&mut cache, &first);
        super::image_cache_store(&mut cache, &second);

        assert_eq!(super::image_cache_len(&cache), 1);
    }

    #[test]
    fn http_runtime_hooks_create_request_and_response() {
        let request = super::request_new("/users");
        let path = super::request_path(&request);
        let response =
            super::response_ok(&format!("handled {path}")).expect("response should build");

        assert_eq!(path, "/users");
        assert_eq!(super::response_status(&response), 200);
        assert_eq!(super::response_body(&response), "handled /users");
    }

    #[test]
    fn db_runtime_hooks_pool_connections() {
        let pool = ResourcePool::from_factory(2, || super::db_connection_open("db://local"));
        {
            let mut conn = pool.borrow();
            super::db_connection_query(&mut conn, "select 1").expect("query should run");
        }

        assert_eq!(pool.values.borrow().len(), 2);
    }

    #[test]
    fn config_runtime_hooks_load_and_replace_store() {
        let first = std::env::temp_dir().join(format!(
            "rsscript-runtime-config-first-{}.txt",
            std::process::id()
        ));
        let second = std::env::temp_dir().join(format!(
            "rsscript-runtime-config-second-{}.txt",
            std::process::id()
        ));
        std::fs::write(&first, "initial\n").expect("first config should write");
        std::fs::write(&second, "reloaded\n").expect("second config should write");

        let initial = super::config_load(&first).expect("initial config should load");
        let mut store = super::config_store_new(&initial);
        let reloaded = super::config_load(&second).expect("reloaded config should load");
        super::config_store_replace(&mut store, &reloaded);

        assert_eq!(super::config_store_name(&store), "reloaded");

        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }

    #[test]
    fn rules_config_runtime_hooks_load_and_replace_global() {
        let first = std::env::temp_dir().join(format!(
            "rsscript-runtime-rules-first-{}.txt",
            std::process::id()
        ));
        let second = std::env::temp_dir().join(format!(
            "rsscript-runtime-rules-second-{}.txt",
            std::process::id()
        ));
        std::fs::write(&first, "alpha\nbeta\n").expect("first rules should write");
        std::fs::write(&second, "gamma\n").expect("second rules should write");

        let first_rules = super::rule_loader_load_rules(&first).expect("first rules should load");
        let initial = super::config_new("initial", &first_rules);
        let mut global = super::global_config_new(&initial);
        let second_rules =
            super::rule_loader_load_rules(&second).expect("second rules should load");
        let next = super::config_new("next", &second_rules);
        super::global_config_replace(&mut global, &next);

        assert_eq!(super::global_config_rule_count(&global), 1);

        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }
}
