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
