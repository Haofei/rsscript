use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use crate::ChannelError;

use super::{ProcessEvent, RUNTIME_PROCESS_OUTPUT_CEILING_BYTES};

pub(super) const PROCESS_STREAM_CHANNEL_CAPACITY: usize = 64;
pub(super) const PROCESS_CAPTURE_CHANNEL_CAPACITY: usize = 64;

pub(super) struct ProcessChunk {
    pub(super) stderr: bool,
    pub(super) bytes: Vec<u8>,
    pub(super) truncated: bool,
}

pub(super) struct ProcessStreamLimit {
    cap: usize,
    used: usize,
    truncated_sent: bool,
}

impl ProcessStreamLimit {
    pub(super) fn new(cap: usize) -> Self {
        Self {
            cap,
            used: 0,
            truncated_sent: false,
        }
    }

    fn take(&mut self, bytes: &[u8]) -> (Vec<u8>, bool) {
        let cap = self.cap;
        if self.used >= cap {
            if !self.truncated_sent {
                self.truncated_sent = true;
                return (Vec::new(), true);
            }
            return (Vec::new(), false);
        }
        let remaining = cap - self.used;
        if bytes.len() > remaining {
            self.used = cap;
            let truncated = !self.truncated_sent;
            self.truncated_sent = true;
            (bytes[..remaining].to_vec(), truncated)
        } else {
            self.used += bytes.len();
            (bytes.to_vec(), false)
        }
    }
}

pub(super) struct ProcessCapture {
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
    pub(super) merged: Vec<u8>,
    cap: Option<usize>,
    used: usize,
    merge_stderr: bool,
    pub(super) truncated: bool,
}

impl ProcessCapture {
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

    pub(super) fn push(&mut self, chunk: ProcessChunk) {
        self.truncated |= chunk.truncated;
        let bytes = self.capped_bytes(&chunk.bytes);
        if bytes.is_empty() {
            return;
        }
        if chunk.stderr {
            self.stderr.extend_from_slice(bytes);
        } else {
            self.stdout.extend_from_slice(bytes);
        }
        if self.merge_stderr || !chunk.stderr {
            self.merged.extend_from_slice(bytes);
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

pub(super) fn spawn_process_reader<R>(
    mut reader: R,
    stderr: bool,
    sender: mpsc::SyncSender<ProcessChunk>,
    remaining: Arc<AtomicUsize>,
    truncated: Arc<AtomicBool>,
) -> std::thread::JoinHandle<std::io::Result<()>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            let bytes = reader.read(&mut buffer)?;
            if bytes == 0 {
                return Ok(());
            }
            let retained = reserve_process_output_bytes(&remaining, bytes);
            if retained < bytes {
                truncated.store(true, Ordering::Release);
            }
            if retained == 0 {
                continue;
            }
            if sender
                .send(ProcessChunk {
                    stderr,
                    bytes: buffer[..retained].to_vec(),
                    truncated: retained < bytes,
                })
                .is_err()
            {
                return Ok(());
            }
        }
    })
}

pub(super) fn normalized_process_output_cap(requested: i64) -> usize {
    usize::try_from(requested)
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(RUNTIME_PROCESS_OUTPUT_CEILING_BYTES)
        .min(RUNTIME_PROCESS_OUTPUT_CEILING_BYTES)
}

fn reserve_process_output_bytes(remaining: &AtomicUsize, requested: usize) -> usize {
    let mut available = remaining.load(Ordering::Acquire);
    loop {
        let retained = requested.min(available);
        match remaining.compare_exchange_weak(
            available,
            available - retained,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return retained,
            Err(actual) => available = actual,
        }
    }
}

pub(super) fn spawn_process_event_reader<R>(
    mut reader: R,
    kind: &'static str,
    limit: Arc<Mutex<ProcessStreamLimit>>,
    sender: mpsc::SyncSender<Result<ProcessEvent, ChannelError>>,
) -> std::thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            let bytes = match reader.read(&mut buffer) {
                Ok(0) => return,
                Ok(bytes) => bytes,
                Err(error) => {
                    let _ = sender.send(Ok(ProcessEvent {
                        kind: "error".to_string(),
                        data: format!("failed to read {kind}: {error}"),
                        status: -1,
                    }));
                    return;
                }
            };
            let (chunk, truncated) = {
                let mut limit = limit.lock().expect("process stream limit lock poisoned");
                limit.take(&buffer[..bytes])
            };
            if !chunk.is_empty() {
                let _ = sender.send(Ok(ProcessEvent {
                    kind: kind.to_string(),
                    data: String::from_utf8_lossy(&chunk).to_string(),
                    status: -1,
                }));
            }
            if truncated {
                let _ = sender.send(Ok(ProcessEvent {
                    kind: "truncated".to_string(),
                    data: String::new(),
                    status: -1,
                }));
            }
        }
    })
}

pub(super) fn drain_process_chunks(
    receiver: &mpsc::Receiver<ProcessChunk>,
    captured: &mut ProcessCapture,
) {
    while let Ok(chunk) = receiver.try_recv() {
        captured.push(chunk);
    }
}

pub(super) fn join_process_reader(
    thread: Option<std::thread::JoinHandle<std::io::Result<()>>>,
    stream: &str,
    command: &str,
) -> Result<(), String> {
    let Some(thread) = thread else {
        return Ok(());
    };
    match thread.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(format!("failed to read {stream} for `{command}`: {error}")),
        Err(_) => Err(format!("{stream} reader panicked for `{command}`")),
    }
}
