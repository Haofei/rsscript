use super::*;

/// One activation record on the explicit, suspendable VM call stack.
pub(super) struct Frame {
    pub(super) func: Rc<RegFunction>,
    #[cfg(feature = "native-jit")]
    pub(super) function_ordinal: usize,
    pub(super) ip: usize,
    pub(super) base: usize,
    pub(super) ret_dst: usize,
    pub(super) mut_writeback: Vec<(usize, usize)>,
    pub(super) tail_calls: usize,
}

/// Resource limits for one register-VM execution. These are resilience
/// controls, not an operating-system isolation boundary.
#[derive(Debug, Clone)]
pub struct VmLimits {
    /// Maximum simultaneous RSScript call frames.
    pub max_depth: usize,
    /// Maximum executed source instructions; `None` is unbounded.
    pub step_budget: Option<u64>,
    /// Best-effort cumulative VM allocation quota. This is not a live-memory measurement.
    pub allocation_budget: Option<usize>,
    /// Maximum reachable RSScript value storage at an instruction boundary.
    pub live_memory_limit: Option<usize>,
    /// Host-controlled cooperative cancellation token.
    pub cancel: Option<rsscript_operation::CancellationToken>,
    /// Monotonic execution deadline shared with Provider calls.
    pub deadline: Option<rsscript_operation::MonotonicDeadline>,
    /// Maximum bytes written to captured stdout/stderr.
    pub stdout_budget: Option<usize>,
    /// Maximum deterministic runtime intrinsic calls.
    pub intrinsic_call_budget: Option<u64>,
    /// Maximum explicitly linked external Provider calls.
    pub provider_call_budget: Option<u64>,
    /// Maximum simultaneously live Provider-owned resources.
    pub resource_limit: Option<usize>,
    /// Whether synchronous execution may call a Provider declared as blocking.
    pub allow_blocking_provider_calls: bool,
}

pub(super) const DEFAULT_MAX_DEPTH: usize = 16_384;
pub(super) const CANCEL_POLL_INTERVAL: u64 = 1024;
pub(super) const MAP_ENTRY_BYTES: usize = std::mem::size_of::<VmValue>() * 2;
pub(super) const MAX_INTRINSIC_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

impl Default for VmLimits {
    fn default() -> Self {
        Self {
            max_depth: 4_096,
            step_budget: Some(50_000_000),
            allocation_budget: Some(256 * 1024 * 1024),
            live_memory_limit: Some(128 * 1024 * 1024),
            cancel: None,
            deadline: None,
            stdout_budget: Some(4 * 1024 * 1024),
            intrinsic_call_budget: Some(1_000_000),
            provider_call_budget: Some(100_000),
            resource_limit: Some(4_096),
            allow_blocking_provider_calls: false,
        }
    }
}

impl VmLimits {
    /// Disable accounting limits for an explicitly trusted host workload.
    pub fn unbounded_for_trusted_host() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            step_budget: None,
            allocation_budget: None,
            live_memory_limit: None,
            cancel: None,
            deadline: None,
            stdout_budget: None,
            intrinsic_call_budget: None,
            provider_call_budget: None,
            resource_limit: None,
            allow_blocking_provider_calls: true,
        }
    }
}
