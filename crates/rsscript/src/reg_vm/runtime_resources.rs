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

#[derive(Debug, Clone)]
pub(super) struct VmDbConnection {
    pub(super) url: String,
    pub(super) queries: Vec<String>,
}

impl VmDbConnection {
    pub(super) fn to_value(&self) -> VmValue {
        db_connection_value(self.url.clone(), self.queries.clone())
    }
}

pub(super) struct VmProcessCapture {
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
    pub(super) merged: Vec<u8>,
    pub(super) cap: Option<usize>,
    pub(super) used: usize,
    pub(super) merge_stderr: bool,
    pub(super) truncated: bool,
}

impl VmProcessCapture {
    pub(super) fn new(cap: Option<usize>, merge_stderr: bool) -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            merged: Vec::new(),
            cap,
            used: 0,
            merge_stderr,
            truncated: false,
        }
    }

    pub(super) fn push(&mut self, stderr: bool, bytes: &[u8]) {
        let bytes = self.capped_bytes(bytes).to_vec();
        if bytes.is_empty() {
            return;
        }
        if stderr {
            self.stderr.extend_from_slice(&bytes);
        } else {
            self.stdout.extend_from_slice(&bytes);
        }
        if self.merge_stderr || !stderr {
            self.merged.extend_from_slice(&bytes);
        }
    }

    fn capped_bytes<'a>(&mut self, bytes: &'a [u8]) -> &'a [u8] {
        let Some(cap) = self.cap else {
            return bytes;
        };
        if self.used >= cap {
            self.truncated = true;
            return &bytes[..0];
        }
        let remaining = cap - self.used;
        if bytes.len() > remaining {
            self.truncated = true;
            self.used = cap;
            &bytes[..remaining]
        } else {
            self.used += bytes.len();
            bytes
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct VmFileState {
    pub(super) path: String,
    pub(super) mode: String,
    pub(super) cursor: u64,
}

impl VmFileState {
    pub(super) fn to_value(&self) -> VmValue {
        file_value(&self.path, &self.mode, self.cursor)
    }
}

pub(super) struct ImageState {
    pub(super) bytes: Vec<u8>,
    pub(super) width: Option<i64>,
    pub(super) height: Option<i64>,
    pub(super) operations: Vec<String>,
}

impl ImageState {
    pub(super) fn to_value(&self) -> VmValue {
        image_value(
            self.bytes.clone(),
            self.width,
            self.height,
            self.operations.clone(),
        )
    }

    pub(super) fn saved_bytes(&self) -> Vec<u8> {
        let mut bytes = self.bytes.clone();
        bytes.extend_from_slice(b"\n# rsscript-image-ops:");
        bytes.extend_from_slice(self.operations.join(",").as_bytes());
        if let (Some(width), Some(height)) = (self.width, self.height) {
            bytes.extend_from_slice(format!(";size={width}x{height}").as_bytes());
        }
        bytes
    }

    pub(super) fn inspect_line(&self) -> String {
        let size = self
            .width
            .zip(self.height)
            .map(|(width, height)| format!("{width}x{height}"))
            .unwrap_or_else(|| "unknown".to_string());
        format!(
            "image bytes={} size={} ops={}",
            self.bytes.len(),
            size,
            self.operations.join(",")
        )
    }
}
