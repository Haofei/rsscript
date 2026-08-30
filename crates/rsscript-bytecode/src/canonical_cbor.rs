use std::error::Error;
use std::fmt;

use half::f16;
use serde_json::{Map, Number, Value};

const MAX_NESTING: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CanonicalCborError(String);

impl CanonicalCborError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<String> for CanonicalCborError {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl fmt::Display for CanonicalCborError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CanonicalCborError {}

pub(crate) fn encode(value: &Value) -> Result<Vec<u8>, CanonicalCborError> {
    let mut output = Vec::new();
    encode_value(value, &mut output, 0)?;
    Ok(output)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Value, CanonicalCborError> {
    let mut decoder = Decoder { bytes, cursor: 0 };
    let value = decoder.value(0)?;
    if decoder.cursor != bytes.len() {
        return Err(CanonicalCborError::new("trailing CBOR bytes"));
    }
    Ok(value)
}

fn encode_value(
    value: &Value,
    output: &mut Vec<u8>,
    depth: usize,
) -> Result<(), CanonicalCborError> {
    if depth > MAX_NESTING {
        return Err(CanonicalCborError::new("CBOR nesting limit exceeded"));
    }
    match value {
        Value::Null => output.push(0xf6),
        Value::Bool(false) => output.push(0xf4),
        Value::Bool(true) => output.push(0xf5),
        Value::Number(number) => encode_number(number, output)?,
        Value::String(text) => {
            encode_argument(3, text.len() as u64, output);
            output.extend_from_slice(text.as_bytes());
        }
        Value::Array(values) => {
            encode_argument(4, values.len() as u64, output);
            for value in values {
                encode_value(value, output, depth + 1)?;
            }
        }
        Value::Object(values) => {
            encode_argument(5, values.len() as u64, output);
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|(left, _), (right, _)| {
                left.len()
                    .cmp(&right.len())
                    .then_with(|| left.as_bytes().cmp(right.as_bytes()))
            });
            for (key, value) in entries {
                encode_argument(3, key.len() as u64, output);
                output.extend_from_slice(key.as_bytes());
                encode_value(value, output, depth + 1)?;
            }
        }
    }
    Ok(())
}

fn encode_number(number: &Number, output: &mut Vec<u8>) -> Result<(), CanonicalCborError> {
    if let Some(value) = number.as_i64() {
        if value >= 0 {
            encode_argument(0, value as u64, output);
        } else {
            encode_argument(1, value.unsigned_abs() - 1, output);
        }
        return Ok(());
    }
    if let Some(value) = number.as_u64() {
        encode_argument(0, value, output);
        return Ok(());
    }
    let value = number
        .as_f64()
        .ok_or_else(|| CanonicalCborError::new("JSON number is not representable as CBOR"))?;
    let half = f16::from_f64(value);
    if half.to_f64() == value {
        output.push(0xf9);
        output.extend_from_slice(&half.to_bits().to_be_bytes());
    } else if f64::from(value as f32) == value {
        output.push(0xfa);
        output.extend_from_slice(&(value as f32).to_bits().to_be_bytes());
    } else {
        output.push(0xfb);
        output.extend_from_slice(&value.to_bits().to_be_bytes());
    }
    Ok(())
}

fn encode_argument(major: u8, value: u64, output: &mut Vec<u8>) {
    match value {
        0..=23 => output.push((major << 5) | value as u8),
        24..=0xff => output.extend_from_slice(&[(major << 5) | 24, value as u8]),
        0x100..=0xffff => {
            output.push((major << 5) | 25);
            output.extend_from_slice(&(value as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            output.push((major << 5) | 26);
            output.extend_from_slice(&(value as u32).to_be_bytes());
        }
        _ => {
            output.push((major << 5) | 27);
            output.extend_from_slice(&value.to_be_bytes());
        }
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl Decoder<'_> {
    fn value(&mut self, depth: usize) -> Result<Value, CanonicalCborError> {
        if depth > MAX_NESTING {
            return Err(CanonicalCborError::new("CBOR nesting limit exceeded"));
        }
        let initial = self.byte()?;
        let major = initial >> 5;
        let additional = initial & 0x1f;
        match major {
            0 => Ok(Value::Number(Number::from(self.argument(additional)?))),
            1 => {
                let magnitude = self
                    .argument(additional)?
                    .checked_add(1)
                    .ok_or_else(|| CanonicalCborError::new("negative CBOR integer is too small"))?;
                let magnitude = i64::try_from(magnitude)
                    .map_err(|_| CanonicalCborError::new("negative CBOR integer is too small"))?;
                Ok(Value::Number(Number::from(-magnitude)))
            }
            3 => {
                let length = self.length(additional, 1)?;
                let text = std::str::from_utf8(self.take(length)?)
                    .map_err(|_| CanonicalCborError::new("CBOR text is not UTF-8"))?;
                Ok(Value::String(text.to_owned()))
            }
            4 => {
                let length = self.length(additional, 1)?;
                let mut values = Vec::with_capacity(length);
                for _ in 0..length {
                    values.push(self.value(depth + 1)?);
                }
                Ok(Value::Array(values))
            }
            5 => {
                let length = self.length(additional, 2)?;
                let mut values = Map::new();
                for _ in 0..length {
                    let Value::String(key) = self.value(depth + 1)? else {
                        return Err(CanonicalCborError::new("CBOR object key is not text"));
                    };
                    let value = self.value(depth + 1)?;
                    if values.insert(key, value).is_some() {
                        return Err(CanonicalCborError::new("CBOR object key is duplicated"));
                    }
                }
                Ok(Value::Object(values))
            }
            7 => self.simple(additional),
            _ => Err(CanonicalCborError::new(
                "unsupported value in RSScript canonical CBOR",
            )),
        }
    }

    fn simple(&mut self, additional: u8) -> Result<Value, CanonicalCborError> {
        match additional {
            20 => Ok(Value::Bool(false)),
            21 => Ok(Value::Bool(true)),
            22 => Ok(Value::Null),
            25 => {
                let bits = u16::from_be_bytes(self.array()?);
                self.float_value(f16::from_bits(bits).to_f64())
            }
            26 => {
                let bits = u32::from_be_bytes(self.array()?);
                self.float_value(f64::from(f32::from_bits(bits)))
            }
            27 => {
                let bits = u64::from_be_bytes(self.array()?);
                self.float_value(f64::from_bits(bits))
            }
            _ => Err(CanonicalCborError::new(
                "unsupported simple value in RSScript canonical CBOR",
            )),
        }
    }

    fn float_value(&self, value: f64) -> Result<Value, CanonicalCborError> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| CanonicalCborError::new("non-finite CBOR float is unsupported"))
    }

    fn argument(&mut self, additional: u8) -> Result<u64, CanonicalCborError> {
        match additional {
            0..=23 => Ok(u64::from(additional)),
            24 => Ok(u64::from(self.byte()?)),
            25 => Ok(u64::from(u16::from_be_bytes(self.array()?))),
            26 => Ok(u64::from(u32::from_be_bytes(self.array()?))),
            27 => Ok(u64::from_be_bytes(self.array()?)),
            _ => Err(CanonicalCborError::new(
                "indefinite or reserved CBOR argument is unsupported",
            )),
        }
    }

    fn length(&mut self, additional: u8, minimum: usize) -> Result<usize, CanonicalCborError> {
        let length = usize::try_from(self.argument(additional)?)
            .map_err(|_| CanonicalCborError::new("CBOR collection is too large"))?;
        if length > self.bytes.len().saturating_sub(self.cursor) / minimum {
            return Err(CanonicalCborError::new(
                "CBOR collection length exceeds payload",
            ));
        }
        Ok(length)
    }

    fn byte(&mut self) -> Result<u8, CanonicalCborError> {
        let byte = self
            .bytes
            .get(self.cursor)
            .copied()
            .ok_or_else(|| CanonicalCborError::new("truncated CBOR payload"))?;
        self.cursor += 1;
        Ok(byte)
    }

    fn take(&mut self, length: usize) -> Result<&[u8], CanonicalCborError> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or_else(|| CanonicalCborError::new("CBOR length overflow"))?;
        let bytes = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| CanonicalCborError::new("truncated CBOR payload"))?;
        self.cursor = end;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], CanonicalCborError> {
        self.take(N)?
            .try_into()
            .map_err(|_| CanonicalCborError::new("truncated CBOR payload"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_matches_the_bytecode_v1_canonical_subset() {
        assert_eq!(encode(&serde_json::json!(null)).unwrap(), [0xf6]);
        assert_eq!(encode(&serde_json::json!(24)).unwrap(), [0x18, 0x18]);
        assert_eq!(encode(&serde_json::json!(-1)).unwrap(), [0x20]);
        assert_eq!(encode(&serde_json::json!(1.5)).unwrap(), [0xf9, 0x3e, 0x00]);
        assert_eq!(
            encode(&serde_json::json!({"aa": 1, "b": 2})).unwrap(),
            [0xa2, 0x61, b'b', 0x02, 0x62, b'a', b'a', 0x01]
        );
    }

    #[test]
    fn decoder_rejects_non_json_and_unbounded_forms() {
        assert!(decode(&[0x5f, 0xff]).is_err());
        assert!(decode(&[0xbf, 0xff]).is_err());
        assert!(decode(&[0xa1, 0x01, 0x02]).is_err());
        assert!(decode(&[0x9f, 0xff]).is_err());
    }

    #[test]
    fn representative_value_round_trips() {
        let value = serde_json::json!({"bool": true, "float": 1.1, "int": -42, "list": [null, "rss", 4_294_967_296_u64]});
        assert_eq!(decode(&encode(&value).unwrap()).unwrap(), value);
    }
}
