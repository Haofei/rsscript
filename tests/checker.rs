use rsscript::analyze_source;

fn codes(source: &str) -> Vec<String> {
    analyze_source("test.rss", source)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn accepts_canonical_local_thumbnail_pipeline() {
    let source = r#"
mode: uses-local

struct Image {
    pixels: Buffer
}

fn make_thumbnail(input: read Path, output: read Path) -> Result<Unit, ImageError> {
    local image = Image.load(path: read input)?

    Image.resize(image: mut image, width: 256, height: 256)
    Image.normalize(image: mut image)

    let shared = manage image

    Image.save(image: read shared, path: read output)?

    return Ok(Unit)
}
"#;

    assert_eq!(codes(source), Vec::<String>::new());
}

#[test]
fn reports_mode_and_call_style_violations() {
    let source = r#"
mode: managed

struct Image {
    pixels: Buffer
}

fn resize(image: mut Image, width: Int, height: Int) -> Unit {
    local image = Image.load(path: read "in.png")
    Image.resize(image: image, width: 800, height: 600)
    Image.save(read image, path: read "out.png")
}
"#;

    let result = codes(source);
    assert!(result.contains(&"RS0101".to_string()));
    assert!(result.contains(&"RS0201".to_string()));
    assert!(result.contains(&"RS0202".to_string()));
}

#[test]
fn reports_use_after_manage() {
    let source = r#"
mode: uses-local

struct Image {
    pixels: Buffer
}

fn publish(path: read Path) -> Unit {
    local image = Image.load(path: read path)
    let shared = manage image
    Image.save(image: read image, path: read path)
    Image.save(image: read shared, path: read path)
}
"#;

    assert!(codes(source).contains(&"RS0401".to_string()));
}

#[test]
fn reports_retaining_local_value() {
    let source = r#"
mode: uses-local

class Cache {
    entries: Map<String, Image>
}

struct Image {
    pixels: Buffer
}

fn cache_put(cache: mut Cache, value: read Image) -> Unit
    effects(retains(value))
{
}

fn run(cache: mut Cache, path: read Path) -> Unit {
    local image = Image.load(path: read path)
    cache_put(cache: mut cache, value: read image)
}
"#;

    assert!(codes(source).contains(&"RS0501".to_string()));
}

#[test]
fn reports_resource_escape_and_resource_field() {
    let source = r#"
mode: managed

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

struct Logger {
    file: File
}

fn leak(path: read Path) -> Unit {
    with File.open(path: read path) as file {
        return file
    }
}
"#;

    let result = codes(source);
    assert!(result.contains(&"RS0701".to_string()));
    assert!(result.contains(&"RS0702".to_string()));
}

#[test]
fn reports_fresh_function_returning_managed_value() {
    let source = r#"
mode: managed

struct Image {
    pixels: Buffer
}

fn cached(cache: read Cache) -> fresh Image {
    let image = Cache.get(cache: read cache)
    return image
}
"#;

    assert!(codes(source).contains(&"RS0601".to_string()));
}
