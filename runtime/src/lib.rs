#![forbid(unsafe_code)]

use std::cell::{BorrowError, BorrowMutError, Ref, RefCell, RefMut};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::hash::Hash;
use std::io::{Read, Write};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::str::Utf8Error;

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

impl From<std::io::Error> for JsonError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub fn json_error_message(error: &JsonError) -> String {
    error.to_string()
}

pub fn file_error_message(error: &std::io::Error) -> String {
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

pub fn path_join<P: RuntimePath + ?Sized>(base: &P, child: &str) -> PathBuf {
    base.as_path().join(child)
}

pub fn path_to_string<P: RuntimePath + ?Sized>(path: &P) -> String {
    path.as_path().to_string_lossy().to_string()
}

pub fn path_file_name<P: RuntimePath + ?Sized>(path: &P) -> Option<String> {
    path.as_path()
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
}

pub fn path_extension<P: RuntimePath + ?Sized>(path: &P) -> Option<String> {
    path.as_path()
        .extension()
        .map(|extension| extension.to_string_lossy().to_string())
}

pub fn path_parent<P: RuntimePath + ?Sized>(path: &P) -> Option<PathBuf> {
    path.as_path().parent().map(PathBuf::from)
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

pub fn file_read_all_string(file: &mut File) -> std::io::Result<String> {
    let mut text = String::new();
    file.inner.read_to_string(&mut text)?;
    Ok(text)
}

pub fn file_read_into(file: &mut File, buffer: &mut Vec<u8>) -> std::io::Result<bool> {
    buffer.clear();
    let bytes_read = file.inner.read_to_end(buffer)?;
    Ok(bytes_read > 0)
}

pub fn file_write<B: RuntimeBytes + ?Sized>(file: &mut File, data: &B) -> std::io::Result<()> {
    file.inner.write_all(data.as_bytes_slice())
}

pub fn file_write_buffer(file: &mut File, buffer: &[u8]) -> std::io::Result<()> {
    file.inner.write_all(buffer)
}

pub fn directory_list_files<P: RuntimePath + ?Sized>(path: &P) -> std::io::Result<Vec<String>> {
    let root = path.as_path();
    let mut files = Vec::new();
    collect_directory_files(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_directory_files(
    root: &std::path::Path,
    current: &std::path::Path,
    files: &mut Vec<String>,
) -> std::io::Result<()> {
    if current.is_file() {
        files.push(relative_runtime_path(root, current));
        return Ok(());
    }
    for entry in std::fs::read_dir(current)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_directory_files(root, &path, files)?;
        } else if path.is_file() {
            files.push(relative_runtime_path(root, &path));
        }
    }
    Ok(())
}

fn relative_runtime_path(root: &std::path::Path, path: &std::path::Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn os_close(fd: i64) {
    let _ = fd;
}

pub fn args_count() -> i64 {
    std::env::args().skip(1).count() as i64
}

pub fn args_get_or_default(index: i64, default: &str) -> String {
    if index < 0 {
        return default.to_string();
    }
    std::env::args()
        .skip(1)
        .nth(index as usize)
        .unwrap_or_else(|| default.to_string())
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

pub fn list_count_where<T: Clone>(list: &[T], mut predicate: impl FnMut(T) -> bool) -> i64 {
    list.iter()
        .filter(|item| predicate((*item).clone()))
        .count() as i64
}

pub fn list_any<T: Clone>(list: &[T], mut predicate: impl FnMut(T) -> bool) -> bool {
    list.iter().any(|item| predicate(item.clone()))
}

pub fn list_all<T: Clone>(list: &[T], mut predicate: impl FnMut(T) -> bool) -> bool {
    list.iter().all(|item| predicate(item.clone()))
}

pub fn list_find<T: Clone>(list: &[T], mut predicate: impl FnMut(T) -> bool) -> Option<T> {
    list.iter().find(|item| predicate((*item).clone())).cloned()
}

pub fn list_filter<T: Clone>(list: &[T], mut predicate: impl FnMut(T) -> bool) -> Vec<T> {
    list.iter()
        .filter(|item| predicate((*item).clone()))
        .cloned()
        .collect()
}

pub fn list_map<T: Clone, U>(list: &[T], mapper: impl FnMut(T) -> U) -> Vec<U> {
    list.iter().cloned().map(mapper).collect()
}

pub fn list_fold<T: Clone, U: Clone>(
    list: &[T],
    initial: &U,
    mut folder: impl FnMut(U, T) -> U,
) -> U {
    let mut state = initial.clone();
    for item in list.iter().cloned() {
        state = folder(state, item);
    }
    state
}

pub fn list_try_fold<T: Clone, U: Clone, E>(
    list: &[T],
    initial: &U,
    mut folder: impl FnMut(U, T) -> Result<U, E>,
) -> Result<U, E> {
    let mut state = initial.clone();
    for item in list.iter().cloned() {
        state = folder(state, item)?;
    }
    Ok(state)
}

pub fn list_consume<T>(list: Vec<T>) {
    drop(list);
}

pub fn map_new<K, V>() -> HashMap<K, V> {
    HashMap::new()
}

pub fn map_len<K, V>(map: &HashMap<K, V>) -> i64 {
    map.len() as i64
}

pub fn map_is_empty<K, V>(map: &HashMap<K, V>) -> bool {
    map.is_empty()
}

pub fn map_contains_key<K: Eq + Hash, V>(map: &HashMap<K, V>, key: &K) -> bool {
    map.contains_key(key)
}

pub fn map_get<K: Eq + Hash, V: Clone>(map: &HashMap<K, V>, key: &K) -> Option<V> {
    map.get(key).cloned()
}

pub fn map_insert<K: Eq + Hash + Clone, V: Clone>(map: &mut HashMap<K, V>, key: &K, value: &V) {
    map.insert(key.clone(), value.clone());
}

pub fn map_remove<K: Eq + Hash, V>(map: &mut HashMap<K, V>, key: &K) -> Option<V> {
    map.remove(key)
}

pub fn map_clear<K, V>(map: &mut HashMap<K, V>) {
    map.clear();
}

pub fn set_new<T>() -> HashSet<T> {
    HashSet::new()
}

pub fn set_len<T>(set: &HashSet<T>) -> i64 {
    set.len() as i64
}

pub fn set_is_empty<T>(set: &HashSet<T>) -> bool {
    set.is_empty()
}

pub fn set_contains<T: Eq + Hash>(set: &HashSet<T>, value: &T) -> bool {
    set.contains(value)
}

pub fn set_insert<T: Eq + Hash + Clone>(set: &mut HashSet<T>, value: &T) -> bool {
    set.insert(value.clone())
}

pub fn set_remove<T: Eq + Hash>(set: &mut HashSet<T>, value: &T) -> bool {
    set.remove(value)
}

pub fn set_clear<T>(set: &mut HashSet<T>) {
    set.clear();
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

pub fn bytes_from_string(value: &str) -> Vec<u8> {
    value.as_bytes().to_vec()
}

pub fn bytes_from_buffer(buffer: &[u8]) -> Vec<u8> {
    buffer.to_vec()
}

pub fn bytes_consume(bytes: Vec<u8>) {
    drop(bytes);
}

pub fn url_from_string(value: &str) -> String {
    value.to_string()
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

#[derive(Clone)]
pub struct Managed<T> {
    inner: Rc<RefCell<T>>,
    origin_span: Option<SourceSpan>,
}

impl<T> Managed<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Rc::new(RefCell::new(value)),
            origin_span: None,
        }
    }

    pub fn new_at(value: T, span: SourceSpan) -> Self {
        Self {
            inner: Rc::new(RefCell::new(value)),
            origin_span: Some(span),
        }
    }

    pub fn try_read(&self) -> Result<ManagedRead<'_, T>, RuntimeError> {
        self.inner
            .try_borrow()
            .map(ManagedRead)
            .map_err(managed_read_error)
    }

    pub fn try_write(&self) -> Result<ManagedWrite<'_, T>, RuntimeError> {
        self.inner
            .try_borrow_mut()
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
        Rc::ptr_eq(&left.inner, &right.inner)
    }

    pub fn origin_span(&self) -> Option<&SourceSpan> {
        self.origin_span.as_ref()
    }
}

#[derive(Clone)]
pub struct WeakManaged<T> {
    inner: Weak<RefCell<T>>,
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
        inner: Rc::downgrade(&value.inner),
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

pub fn string_from_int(value: i64) -> String {
    value.to_string()
}

pub fn string_from_bool(value: bool) -> String {
    value.to_string()
}

pub fn string_len(value: &str) -> i64 {
    value.len() as i64
}

pub fn string_is_empty(value: &str) -> bool {
    value.is_empty()
}

pub fn string_starts_with(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
}

pub fn string_ends_with(value: &str, suffix: &str) -> bool {
    value.ends_with(suffix)
}

pub fn string_contains(value: &str, needle: &str) -> bool {
    value.contains(needle)
}

pub fn string_lines(value: &str) -> Vec<String> {
    value.lines().map(str::to_string).collect()
}

pub fn string_join(parts: &[String], separator: &str) -> String {
    parts.join(separator)
}

pub fn string_strip_prefix(value: &str, prefix: &str) -> Option<String> {
    value.strip_prefix(prefix).map(str::to_string)
}

pub fn string_before(value: &str, delimiter: &str) -> Option<String> {
    let index = value.find(delimiter)?;
    Some(value[..index].to_string())
}

pub fn string_after(value: &str, delimiter: &str) -> Option<String> {
    let (_, right) = value.split_once(delimiter)?;
    Some(right.to_string())
}

pub fn string_trim(value: &str) -> String {
    value.trim().to_string()
}

pub fn string_to_lowercase(value: &str) -> String {
    value.to_lowercase()
}

pub fn string_to_uppercase(value: &str) -> String {
    value.to_uppercase()
}

pub fn string_replace(value: &str, from: &str, to: &str) -> String {
    value.replace(from, to)
}

pub fn string_split(value: &str, delimiter: &str) -> Vec<String> {
    value.split(delimiter).map(str::to_string).collect()
}

pub fn string_builder_new() -> String {
    String::new()
}

pub fn string_builder_push(builder: &mut String, value: &str) {
    builder.push_str(value);
}

pub fn string_builder_finish(builder: String) -> String {
    builder
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

pub struct ManagedRead<'a, T>(Ref<'a, T>);

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

pub struct ManagedWrite<'a, T>(RefMut<'a, T>);

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

fn managed_read_error(error: BorrowError) -> RuntimeError {
    let _ = error;
    RuntimeError {
        kind: RuntimeErrorKind::ManagedReadConflict,
        message: "managed value is already being written".to_string(),
        span: None,
    }
}

fn managed_write_error(error: BorrowMutError) -> RuntimeError {
    let _ = error;
    RuntimeError {
        kind: RuntimeErrorKind::ManagedWriteConflict,
        message: "managed value is already being read or written".to_string(),
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

    pub fn try_from_factory<E, F>(max_size: i64, mut create: F) -> Result<Self, E>
    where
        F: FnMut() -> Result<T, E>,
    {
        let count = max_size.max(0) as usize;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(create()?);
        }
        Ok(Self::new(values))
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
    fn managed_handles_are_single_isolate_values() {
        static_assertions::assert_not_impl_any!(super::Managed<String>: Send, Sync);
        static_assertions::assert_not_impl_any!(super::WeakManaged<String>: Send, Sync);
    }

    #[test]
    fn runtime_surface_has_no_legacy_gc_aliases() {
        let source = include_str!("lib.rs");
        let managed_alias = ["pub type ", "Gc"].concat();
        let read_alias = ["pub type ", "G", "c", "Read"].concat();
        let write_alias = ["pub type ", "G", "c", "Write"].concat();

        assert!(!source.contains(&managed_alias));
        assert!(!source.contains(&read_alias));
        assert!(!source.contains(&write_alias));
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
        let path = super::path_from_string("fixtures/rsscript-path.txt");

        assert_eq!(path, std::path::PathBuf::from("fixtures/rsscript-path.txt"));
        assert_eq!(
            super::path_to_string(&path),
            std::path::PathBuf::from("fixtures/rsscript-path.txt")
                .to_string_lossy()
                .to_string()
        );
        assert_eq!(
            super::path_file_name(&path).as_deref(),
            Some("rsscript-path.txt")
        );
        assert_eq!(super::path_extension(&path).as_deref(), Some("txt"));
        assert_eq!(
            super::path_parent(&path),
            Some(std::path::PathBuf::from("fixtures"))
        );
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
        let mut file = super::file_open_read(&path).expect("file should reopen for text read");
        let text = super::file_read_all_string(&mut file).expect("text read should succeed");

        assert_eq!(bytes, b"hello file");
        assert_eq!(text, "hello file");
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
        let json_text = r#"{"profiles":[{"name":"RSScript","age":1,"active":true}]}"#;
        let path =
            std::env::temp_dir().join(format!("rsscript-runtime-json-{}.json", std::process::id()));
        std::fs::write(&path, json_text).expect("JSON fixture should write");

        let value = super::json_parse(json_text).expect("JSON should parse");
        let value_from_file = super::json_parse_file(&path).expect("JSON file should parse");
        let profiles_from_file =
            super::json_field(&value_from_file, "profiles").expect("profiles should exist");
        let file_len =
            super::json_array_len(&profiles_from_file).expect("file profiles should be an array");
        let profiles = super::json_field(&value, "profiles").expect("profiles field should exist");
        let len = super::json_array_len(&profiles).expect("profiles should be an array");
        let profile = super::json_array_get(&profiles, 0).expect("first profile should exist");
        let name =
            super::json_field_string(&profile, "name").expect("name field should be a string");
        let profile_name =
            super::json_as_string(&super::json_field(&profile, "name").unwrap()).unwrap();
        let profile_name_value = super::json_field(&profile, "name").unwrap();
        let age = super::json_field_int(&profile, "age").expect("age should be an integer");
        let active = super::json_field_bool(&profile, "active").expect("active should be a bool");
        let age_value = super::json_field(&profile, "age").unwrap();
        let active_value = super::json_field(&profile, "active").unwrap();
        let age_again = super::json_as_int(&age_value).expect("age value should be an integer");
        let active_again =
            super::json_as_bool(&active_value).expect("active value should be a boolean");
        let profile_is_object = super::json_is_object(&profile);
        let profiles_is_array = super::json_is_array(&profiles);
        let profile_name_is_null = super::json_is_null(&profile_name_value);
        let reasons =
            super::json_parse(r#"["public entry point","error handling boundary"]"#).unwrap();
        let reason_strings = super::json_array_strings(&reasons).unwrap();
        let numbers = super::json_parse("[1, 2]").unwrap();
        let number_values = super::json_array_ints(&numbers).unwrap();
        let flags = super::json_parse("[true, false]").unwrap();
        let flag_values = super::json_array_bools(&flags).unwrap();
        let has_public = super::json_array_contains_string(&reasons, "public entry point").unwrap();
        let has_native = super::json_array_contains_string(&reasons, "native boundary").unwrap();
        let has_error = super::json_array_contains_substring(&reasons, "error handling").unwrap();
        let has_pool = super::json_array_contains_substring(&reasons, "ResourcePool").unwrap();
        let has_public_prefix = super::json_array_contains_prefix(&reasons, "public").unwrap();
        let public_count = super::json_array_count_where(&reasons, |item| {
            Ok(super::json_as_string(&item)?.starts_with("public"))
        })
        .unwrap();
        let folded_count = super::json_array_fold(&reasons, &0_i64, |count, item| {
            if super::json_as_string(&item)?.contains("boundary") {
                return Ok(count + 1);
            }
            Ok(count)
        })
        .unwrap();
        let name_starts_with_rss = super::string_starts_with(&name, "RSS");
        let name_ends_with_script = super::string_ends_with(&name, "Script");
        let name_contains_script = super::string_contains(&name, "Script");
        let lines = super::string_lines("pub fn Api.run()\nreturn Unit\n");
        let joined = super::string_join(&lines, " | ");
        let stripped = super::string_strip_prefix("pub fn Api.run() -> Unit", "pub fn ");
        let before_args = super::string_before("Api.run() -> Unit", "(");
        let after_return = super::string_after("Api.run() -> Unit", "-> ");
        let empty = super::string_is_empty("");
        let trimmed = super::string_trim("  review  ");
        let lower = super::string_to_lowercase("Review");
        let upper = super::string_to_uppercase("review");
        let replaced = super::string_replace("review map", "map", "plan");
        let split = super::string_split("review,map", ",");
        let mut builder = super::string_builder_new();
        super::string_builder_push(&mut builder, "selfhost ");
        super::string_builder_push(&mut builder, "summary");
        let built = super::string_builder_finish(builder);

        assert_eq!(file_len, 1);
        assert_eq!(len, 1);
        assert_eq!(name, "RSScript");
        assert_eq!(profile_name, "RSScript");
        assert_eq!(age, 1);
        assert!(active);
        assert_eq!(age_again, 1);
        assert!(active_again);
        assert!(profile_is_object);
        assert!(profiles_is_array);
        assert!(!profile_name_is_null);
        assert_eq!(
            reason_strings,
            vec!["public entry point", "error handling boundary"]
        );
        assert_eq!(number_values, vec![1, 2]);
        assert_eq!(flag_values, vec![true, false]);
        assert!(has_public);
        assert!(!has_native);
        assert!(has_error);
        assert!(!has_pool);
        assert!(has_public_prefix);
        assert_eq!(public_count, 1);
        assert_eq!(folded_count, 1);
        assert!(name_starts_with_rss);
        assert!(name_ends_with_script);
        assert!(name_contains_script);
        assert_eq!(lines, vec!["pub fn Api.run()", "return Unit"]);
        assert_eq!(joined, "pub fn Api.run() | return Unit");
        assert_eq!(stripped.as_deref(), Some("Api.run() -> Unit"));
        assert_eq!(before_args.as_deref(), Some("Api.run"));
        assert_eq!(after_return.as_deref(), Some("Unit"));
        assert!(empty);
        assert_eq!(trimmed, "review");
        assert_eq!(lower, "review");
        assert_eq!(upper, "REVIEW");
        assert_eq!(replaced, "review plan");
        assert_eq!(split, vec!["review", "map"]);
        assert_eq!(built, "selfhost summary");
        let _ = std::fs::remove_file(path);
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
    fn list_runtime_hooks_count_with_predicate() {
        let values = vec![1_i64, 2, 3, 4];

        let even = super::list_count_where(&values, |value| value % 2 == 0);
        let any_gt_three = super::list_any(&values, |value| value > 3);
        let all_positive = super::list_all(&values, |value| value > 0);
        let found_three = super::list_find(&values, |value| value == 3);
        let filtered = super::list_filter(&values, |value| value > 2);
        let mapped = super::list_map(&values, |value| value + 10);
        let sum = super::list_fold(&values, &0_i64, |state, value| state + value);
        let try_sum = super::list_try_fold(
            &values,
            &0_i64,
            |state, value| -> Result<i64, super::JsonError> { Ok(state + value) },
        )
        .unwrap();

        assert_eq!(even, 2);
        assert!(any_gt_three);
        assert!(all_positive);
        assert_eq!(found_three, Some(3));
        assert_eq!(filtered, vec![3, 4]);
        assert_eq!(mapped, vec![11, 12, 13, 14]);
        assert_eq!(sum, 10);
        assert_eq!(try_sum, 10);
    }

    #[test]
    fn map_and_set_runtime_hooks_cover_common_operations() {
        let mut map = super::map_new::<String, i64>();
        let key = "one".to_string();

        assert!(super::map_is_empty(&map));
        super::map_insert(&mut map, &key, &1);
        assert_eq!(super::map_len(&map), 1);
        assert!(super::map_contains_key(&map, &key));
        assert_eq!(super::map_get(&map, &key), Some(1));
        assert_eq!(super::map_remove(&mut map, &key), Some(1));
        assert!(super::map_is_empty(&map));
        super::map_insert(&mut map, &key, &2);
        super::map_clear(&mut map);
        assert!(super::map_is_empty(&map));

        let mut set = super::set_new::<String>();

        assert!(super::set_is_empty(&set));
        assert!(super::set_insert(&mut set, &key));
        assert_eq!(super::set_len(&set), 1);
        assert!(super::set_contains(&set, &key));
        assert!(super::set_remove(&mut set, &key));
        assert!(super::set_is_empty(&set));
        assert!(super::set_insert(&mut set, &key));
        super::set_clear(&mut set);
        assert!(super::set_is_empty(&set));
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
