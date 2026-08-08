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
