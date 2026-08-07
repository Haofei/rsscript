//! Raw JSON hardening for machine-consumed execution reports.

#![no_main]

use std::sync::OnceLock;

use jsonschema::Validator;
use libfuzzer_sys::fuzz_target;

fn validator() -> &'static Validator {
    static VALIDATOR: OnceLock<Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let schema = serde_json::from_str(include_str!(
            "../../schemas/rsscript.execution_report.v1.schema.json"
        ))
        .expect("checked-in execution report schema must be JSON");
        jsonschema::validator_for(&schema).expect("checked-in execution report schema must compile")
    })
}

fuzz_target!(|data: &[u8]| {
    if data.len() > 16 * 1024 * 1024 {
        return;
    }
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        let _ = validator().is_valid(&value);
    }
});
