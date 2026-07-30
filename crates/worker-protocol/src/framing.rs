use std::io::{self, Cursor, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{ProtocolError, Request, Response};

pub const MAGIC: [u8; 4] = *b"RSSW";
pub const PROTOCOL_VERSION: u16 = 1;
pub const FRAME_HEADER_BYTES: usize = 12;
pub const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum FrameKind {
    Request = 1,
    Response = 2,
}

impl FrameKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Response => "response",
        }
    }
}

pub fn write_request<W: Write>(writer: &mut W, request: &Request) -> Result<(), ProtocolError> {
    request.validate()?;
    write_frame(writer, FrameKind::Request, request, MAX_REQUEST_BYTES)
}

pub fn read_request<R: Read>(reader: &mut R) -> Result<Request, ProtocolError> {
    let request: Request = read_frame(
        reader,
        FrameKind::Request,
        MAX_REQUEST_BYTES,
        "request payload",
    )?;
    request.validate()?;
    Ok(request)
}

pub fn write_response<W: Write>(writer: &mut W, response: &Response) -> Result<(), ProtocolError> {
    response.validate()?;
    write_frame(writer, FrameKind::Response, response, MAX_RESPONSE_BYTES)
}

pub fn read_response<R: Read>(reader: &mut R) -> Result<Response, ProtocolError> {
    let response: Response = read_frame(
        reader,
        FrameKind::Response,
        MAX_RESPONSE_BYTES,
        "response payload",
    )?;
    response.validate()?;
    Ok(response)
}

pub fn encode_request(request: &Request) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = Vec::new();
    write_request(&mut bytes, request)?;
    Ok(bytes)
}

pub fn decode_request(bytes: &[u8]) -> Result<Request, ProtocolError> {
    decode_exact(bytes, |reader| read_request(reader))
}

pub fn encode_response(response: &Response) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = Vec::new();
    write_response(&mut bytes, response)?;
    Ok(bytes)
}

pub fn decode_response(bytes: &[u8]) -> Result<Response, ProtocolError> {
    decode_exact(bytes, |reader| read_response(reader))
}

fn decode_exact<T>(
    bytes: &[u8],
    decode: impl FnOnce(&mut Cursor<&[u8]>) -> Result<T, ProtocolError>,
) -> Result<T, ProtocolError> {
    let mut reader = Cursor::new(bytes);
    let value = decode(&mut reader)?;
    let consumed = usize::try_from(reader.position()).unwrap_or(usize::MAX);
    if consumed != bytes.len() {
        return Err(ProtocolError::TrailingData {
            bytes: bytes.len() - consumed,
        });
    }
    Ok(value)
}

fn write_frame<W: Write, T: Serialize>(
    writer: &mut W,
    kind: FrameKind,
    value: &T,
    max_bytes: usize,
) -> Result<(), ProtocolError> {
    let payload = serialize_bounded(value, kind, max_bytes)?;
    let length = u32::try_from(payload.len()).map_err(|_| ProtocolError::PayloadTooLarge {
        kind,
        actual: u32::MAX,
        limit: max_bytes,
    })?;

    let mut header = [0_u8; FRAME_HEADER_BYTES];
    header[..4].copy_from_slice(&MAGIC);
    header[4..6].copy_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    header[6..8].copy_from_slice(&(kind as u16).to_be_bytes());
    header[8..12].copy_from_slice(&length.to_be_bytes());
    writer.write_all(&header).map_err(ProtocolError::Io)?;
    writer.write_all(&payload).map_err(ProtocolError::Io)?;
    writer.flush().map_err(ProtocolError::Io)
}

fn read_frame<R: Read, T: DeserializeOwned>(
    reader: &mut R,
    expected_kind: FrameKind,
    max_bytes: usize,
    payload_section: &'static str,
) -> Result<T, ProtocolError> {
    let mut header = [0_u8; FRAME_HEADER_BYTES];
    read_exact_section(reader, &mut header, "header")?;

    let actual_magic = [header[0], header[1], header[2], header[3]];
    if actual_magic != MAGIC {
        return Err(ProtocolError::BadMagic {
            actual: actual_magic,
        });
    }
    let version = u16::from_be_bytes([header[4], header[5]]);
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion { actual: version });
    }
    let kind = u16::from_be_bytes([header[6], header[7]]);
    if kind != expected_kind as u16 {
        return Err(ProtocolError::UnexpectedKind {
            expected: expected_kind,
            actual: kind,
        });
    }
    let length = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
    if length as usize > max_bytes {
        return Err(ProtocolError::PayloadTooLarge {
            kind: expected_kind,
            actual: length,
            limit: max_bytes,
        });
    }

    let mut payload = vec![0_u8; length as usize];
    read_exact_section(reader, &mut payload, payload_section)?;
    serde_json::from_slice(&payload).map_err(ProtocolError::Deserialize)
}

fn read_exact_section<R: Read>(
    reader: &mut R,
    mut bytes: &mut [u8],
    section: &'static str,
) -> Result<(), ProtocolError> {
    while !bytes.is_empty() {
        match reader.read(bytes) {
            Ok(0) => return Err(ProtocolError::Truncated { section }),
            Ok(read) => bytes = &mut bytes[read..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(ProtocolError::Io(error)),
        }
    }
    Ok(())
}

struct BoundedWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
    exceeded: bool,
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|length| length > self.max_bytes)
        {
            self.exceeded = true;
            return Err(io::Error::other("worker protocol payload limit exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn serialize_bounded<T: Serialize>(
    value: &T,
    kind: FrameKind,
    max_bytes: usize,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        max_bytes,
        exceeded: false,
    };
    match serde_json::to_writer(&mut writer, value) {
        Ok(()) => Ok(writer.bytes),
        Err(_) if writer.exceeded => Err(ProtocolError::PayloadTooLarge {
            kind,
            actual: u32::try_from(max_bytes.saturating_add(1)).unwrap_or(u32::MAX),
            limit: max_bytes,
        }),
        Err(error) => Err(ProtocolError::Serialize(error)),
    }
}
