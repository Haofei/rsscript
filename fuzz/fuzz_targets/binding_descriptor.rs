//! Raw TOML hardening for the versioned binding descriptor schema.

#![no_main]

use std::sync::OnceLock;

use jsonschema::Validator;
use libfuzzer_sys::fuzz_target;

fn validator() -> &'static Validator {
    static VALIDATOR: OnceLock<Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let schema = serde_json::from_str(include_str!(
            "../../schemas/rsscript-bindings-v1.json"
        ))
        .expect("checked-in binding schema must be JSON");
        jsonschema::validator_for(&schema).expect("checked-in binding schema must compile")
    })
}

fuzz_target!(|data: &[u8]| {
    if data.len() > 1024 * 1024 {
        return;
    }
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(value) = toml::from_str::<toml::Value>(source) {
        let json = serde_json::to_value(value).expect("TOML value must project to JSON");
        let _ = validator().is_valid(&json);
    }
});
