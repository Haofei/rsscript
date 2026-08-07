use super::*;

#[derive(Debug, Clone)]
pub(super) struct VmChannelState {
    pub(super) id: i64,
    pub(super) capacity: i64,
    pub(super) receiver_taken: bool,
}

impl VmChannelState {
    pub(super) fn to_value(&self) -> VmValue {
        channel_value(self.id, self.capacity, self.receiver_taken)
    }
}

#[derive(Debug, Clone)]
pub(super) struct VmHttpRequest {
    pub(super) method: String,
    pub(super) url: String,
    pub(super) body: String,
    pub(super) timeout_ms: i64,
    pub(super) attempts: i64,
    pub(super) backoff_ms: i64,
    pub(super) header_count: i64,
}

impl VmHttpRequest {
    pub(super) fn to_value(&self) -> VmValue {
        http_request_value(
            &self.method,
            &self.url,
            &self.body,
            self.timeout_ms,
            self.attempts,
            self.backoff_ms,
            self.header_count,
        )
    }
}
