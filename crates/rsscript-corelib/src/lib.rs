#![forbid(unsafe_code)]

//! Deterministic algorithms for RSScript's core library.
//!
//! This crate deliberately knows nothing about bytecode, VM values, Providers,
//! budgets, or host state. Execution backends adapt their value representation
//! at the boundary and retain responsibility for accounting.

use base64::Engine;
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

const URL_COMPONENT_SET: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub mod encoding {
    use super::*;

    pub fn base64_decode(value: &str) -> Result<Vec<u8>, String> {
        base64::engine::general_purpose::STANDARD
            .decode(value)
            .map_err(|error| error.to_string())
    }

    pub fn base64_encode(value: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(value)
    }

    pub fn hex_decode(value: &str) -> Result<Vec<u8>, String> {
        hex::decode(value).map_err(|error| error.to_string())
    }

    pub fn hex_encode(value: &[u8]) -> String {
        hex::encode(value)
    }

    pub fn url_decode_component(value: &str) -> Result<String, String> {
        percent_decode_str(value)
            .decode_utf8()
            .map(|value| value.into_owned())
            .map_err(|error| error.to_string())
    }

    pub fn url_encode_component(value: &str) -> String {
        utf8_percent_encode(value, URL_COMPONENT_SET).to_string()
    }
}

/// Pure, representation-independent collection transformations. Callers own
/// allocation accounting and choose their concrete collection layout.
pub mod collections {
    use std::collections::{HashMap, VecDeque};
    use std::hash::Hash;

    pub fn dedup<T: PartialEq>(values: impl IntoIterator<Item = T>) -> Vec<T> {
        let mut result = Vec::new();
        for value in values {
            if !result.contains(&value) {
                result.push(value);
            }
        }
        result
    }

    pub fn reverse<T>(values: impl IntoIterator<Item = T>) -> Vec<T> {
        let mut result = values.into_iter().collect::<Vec<_>>();
        result.reverse();
        result
    }

    pub fn skip<T>(values: impl IntoIterator<Item = T>, count: usize) -> Vec<T> {
        values.into_iter().skip(count).collect()
    }

    pub fn take<T>(values: impl IntoIterator<Item = T>, count: usize) -> Vec<T> {
        values.into_iter().take(count).collect()
    }

    pub fn slice<T>(values: impl IntoIterator<Item = T>, start: usize, len: usize) -> Vec<T> {
        values.into_iter().skip(start).take(len).collect()
    }

    pub fn deque_to_vec<T: Clone>(values: &VecDeque<T>) -> Vec<T> {
        values.iter().cloned().collect()
    }

    pub fn map_difference<K: Clone + Eq + Hash, V: Clone>(
        left: &HashMap<K, V>,
        right: &HashMap<K, V>,
    ) -> HashMap<K, V> {
        left.iter()
            .filter(|(key, _)| !right.contains_key(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    pub fn map_intersection<K: Clone + Eq + Hash, V: Clone>(
        left: &HashMap<K, V>,
        right: &HashMap<K, V>,
    ) -> HashMap<K, V> {
        left.iter()
            .filter(|(key, _)| right.contains_key(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    pub fn map_union<K: Clone + Eq + Hash, V: Clone>(
        left: &HashMap<K, V>,
        right: &HashMap<K, V>,
    ) -> HashMap<K, V> {
        let mut result = left.clone();
        for (key, value) in right {
            result.entry(key.clone()).or_insert_with(|| value.clone());
        }
        result
    }

    pub fn map_is_subset<K: Eq + Hash, V>(left: &HashMap<K, V>, right: &HashMap<K, V>) -> bool {
        left.keys().all(|key| right.contains_key(key))
    }

    pub fn map_keys<K: Clone, V>(values: &HashMap<K, V>) -> Vec<K> {
        values.keys().cloned().collect()
    }

    pub fn map_values<K, V: Clone>(values: &HashMap<K, V>) -> Vec<V> {
        values.values().cloned().collect()
    }
}

/// Regex compilation and matching are deterministic library operations. The
/// execution backend owns its language-level Regex value and maps errors into
/// the language Result/Error representation.
pub mod regex {
    #[derive(Debug, Clone)]
    pub struct CompiledRegex(::regex::Regex);

    impl CompiledRegex {
        pub fn compile(pattern: &str) -> Result<Self, String> {
            ::regex::Regex::new(pattern)
                .map(Self)
                .map_err(|error| error.to_string())
        }

        pub fn is_match(&self, value: &str) -> bool {
            self.0.is_match(value)
        }

        pub fn find(&self, value: &str) -> Option<String> {
            self.0
                .find(value)
                .map(|matched| matched.as_str().to_owned())
        }

        pub fn captures(&self, value: &str) -> Vec<String> {
            self.0
                .captures(value)
                .map(|captures| {
                    captures
                        .iter()
                        .filter_map(|matched| matched.map(|matched| matched.as_str().to_owned()))
                        .collect()
                })
                .unwrap_or_default()
        }

        pub fn replace_all(&self, value: &str, replacement: &str) -> String {
            self.0.replace_all(value, replacement).to_string()
        }

        pub fn split(&self, value: &str) -> Vec<String> {
            self.0.split(value).map(str::to_owned).collect()
        }
    }
}

/// Pure UTC calendar calculations. There is deliberately no `now` operation:
/// wall-clock access remains a Provider capability.
pub mod date {
    use chrono::{DateTime, Datelike, NaiveDate, SecondsFormat, TimeZone, Timelike, Utc};

    pub const MS_PER_DAY: i64 = 86_400_000;

    fn utc_datetime(unix_ms: i64) -> DateTime<Utc> {
        Utc.timestamp_millis_opt(unix_ms)
            .single()
            .unwrap_or_else(|| {
                Utc.timestamp_millis_opt(0)
                    .single()
                    .expect("epoch is valid")
            })
    }

    pub fn add_days(unix_ms: i64, days: i64) -> i64 {
        unix_ms.saturating_add(days.saturating_mul(MS_PER_DAY))
    }

    pub fn add_ms(unix_ms: i64, ms: i64) -> i64 {
        unix_ms.saturating_add(ms)
    }

    pub fn day(unix_ms: i64) -> i64 {
        utc_datetime(unix_ms).day() as i64
    }

    pub fn days_between(start_unix_ms: i64, end_unix_ms: i64) -> i64 {
        end_unix_ms.saturating_sub(start_unix_ms) / MS_PER_DAY
    }

    pub fn days_in_month(year: i64, month: i64) -> i64 {
        let Ok(year) = i32::try_from(year) else {
            return 0;
        };
        let Ok(month) = u32::try_from(month) else {
            return 0;
        };
        let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
            return 0;
        };
        let Some(next_month) = (if month == 12 {
            year.checked_add(1)
                .and_then(|year| NaiveDate::from_ymd_opt(year, 1, 1))
        } else {
            month
                .checked_add(1)
                .and_then(|month| NaiveDate::from_ymd_opt(year, month, 1))
        }) else {
            return 0;
        };
        (next_month - first).num_days()
    }

    pub fn format_iso(unix_ms: i64) -> String {
        utc_datetime(unix_ms).to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    pub fn format_ymd(unix_ms: i64) -> String {
        utc_datetime(unix_ms).format("%Y-%m-%d").to_string()
    }

    pub fn hour(unix_ms: i64) -> i64 {
        utc_datetime(unix_ms).hour() as i64
    }

    pub fn is_leap_year(year: i64) -> bool {
        let Ok(year) = i32::try_from(year) else {
            return false;
        };
        NaiveDate::from_ymd_opt(year, 2, 29).is_some()
    }

    pub fn minute(unix_ms: i64) -> i64 {
        utc_datetime(unix_ms).minute() as i64
    }

    pub fn month(unix_ms: i64) -> i64 {
        utc_datetime(unix_ms).month() as i64
    }

    pub fn parse_iso(value: &str) -> Option<i64> {
        DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|datetime| datetime.with_timezone(&Utc).timestamp_millis())
    }

    pub fn parse_ymd(value: &str) -> Option<i64> {
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
        let datetime = date.and_hms_opt(0, 0, 0)?;
        Some(Utc.from_utc_datetime(&datetime).timestamp_millis())
    }

    pub fn second(unix_ms: i64) -> i64 {
        utc_datetime(unix_ms).second() as i64
    }

    pub fn start_of_day(unix_ms: i64) -> i64 {
        let start = utc_datetime(unix_ms)
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .expect("midnight is valid");
        Utc.from_utc_datetime(&start).timestamp_millis()
    }

    pub fn weekday(unix_ms: i64) -> i64 {
        utc_datetime(unix_ms).weekday().number_from_monday() as i64
    }

    pub fn year(unix_ms: i64) -> i64 {
        utc_datetime(unix_ms).year() as i64
    }
}

/// Deterministic digest functions. These consume explicit input only; entropy
/// and key acquisition remain separate host capabilities.
pub mod crypto {
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};
    use sha3::{
        Sha3_224, Sha3_256, Shake128,
        digest::{ExtendableOutput, Update, XofReader},
    };

    pub fn sha256_hex(value: &[u8]) -> String {
        let mut hasher = Sha256::new();
        Digest::update(&mut hasher, value);
        format!("{:x}", hasher.finalize())
    }

    pub fn sha3_224(value: &[u8]) -> Vec<u8> {
        let mut hasher = Sha3_224::new();
        Update::update(&mut hasher, value);
        hasher.finalize().to_vec()
    }

    pub fn sha3_256(value: &[u8]) -> Vec<u8> {
        let mut hasher = Sha3_256::new();
        Update::update(&mut hasher, value);
        hasher.finalize().to_vec()
    }

    pub fn shake128(value: &[u8], out_len: usize) -> Vec<u8> {
        let mut hasher = Shake128::default();
        Update::update(&mut hasher, value);
        let mut reader = hasher.finalize_xof();
        let mut out = vec![0u8; out_len];
        XofReader::read(&mut reader, &mut out);
        out
    }

    pub fn hmac_sha256_hex(key: &[u8], value: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
        Mac::update(&mut mac, value);
        format!("{:x}", mac.finalize().into_bytes())
    }
}

/// Deterministic byte transformations. The caller supplies all input and owns
/// output budgeting; this module never reads files, clocks, or host state.
pub mod compression {
    use std::io::Read;

    pub fn gzip_decompress(value: &[u8]) -> Result<Vec<u8>, String> {
        let mut decoder = flate2::read::GzDecoder::new(value);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map(|_| out)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{collections::*, compression, crypto, date, encoding::*, regex::CompiledRegex};
    use std::io::Write;

    #[test]
    fn encoding_algorithms_are_deterministic_and_round_trip() {
        let bytes = b"hello / RSScript";
        assert_eq!(base64_decode(&base64_encode(bytes)).unwrap(), bytes);
        assert_eq!(hex_decode(&hex_encode(bytes)).unwrap(), bytes);
        assert_eq!(
            url_decode_component(&url_encode_component("hello / RSScript")).unwrap(),
            "hello / RSScript"
        );
    }

    #[test]
    fn malformed_decoding_returns_data_errors_not_host_errors() {
        assert!(base64_decode("%%%not-base64%%%").is_err());
        assert!(hex_decode("xyz").is_err());
        assert!(url_decode_component("%FF").is_err());
    }

    #[test]
    fn collection_transforms_are_generic_and_preserve_order() {
        assert_eq!(dedup([1, 2, 1, 3, 2]), vec![1, 2, 3]);
        assert_eq!(reverse([1, 2, 3]), vec![3, 2, 1]);
        assert_eq!(skip([1, 2, 3], 1), vec![2, 3]);
        assert_eq!(take([1, 2, 3], 2), vec![1, 2]);
        assert_eq!(slice([1, 2, 3, 4], 1, 2), vec![2, 3]);
        assert_eq!(
            deque_to_vec(&std::collections::VecDeque::from([1, 2, 3])),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn map_set_algebra_is_representation_independent() {
        let left = std::collections::HashMap::from([("a", 1), ("b", 2)]);
        let right = std::collections::HashMap::from([("b", 20), ("c", 3)]);
        assert_eq!(
            map_difference(&left, &right),
            std::collections::HashMap::from([("a", 1)])
        );
        assert_eq!(
            map_intersection(&left, &right),
            std::collections::HashMap::from([("b", 2)])
        );
        assert_eq!(
            map_union(&left, &right),
            std::collections::HashMap::from([("a", 1), ("b", 2), ("c", 3)])
        );
        assert!(map_is_subset(
            &std::collections::HashMap::from([("a", 1)]),
            &left
        ));
        assert_eq!(map_keys(&left).len(), 2);
        assert_eq!(map_values(&left).len(), 2);
    }

    #[test]
    fn regex_operations_preserve_capture_and_replacement_semantics() {
        let regex = CompiledRegex::compile(r"(rss)-(\d+)").unwrap();
        assert!(regex.is_match("rss-42"));
        assert_eq!(regex.find("x rss-42 y"), Some("rss-42".to_owned()));
        assert_eq!(
            regex.captures("rss-42"),
            vec!["rss-42".to_owned(), "rss".to_owned(), "42".to_owned()]
        );
        assert_eq!(regex.replace_all("rss-42", "$2:$1"), "42:rss");
        assert_eq!(
            regex.split("rss-42 rss-7"),
            vec!["".to_owned(), " ".to_owned(), "".to_owned()]
        );
        assert!(CompiledRegex::compile("(").is_err());
    }

    #[test]
    fn date_calculations_are_utc_and_do_not_read_the_clock() {
        let epoch = 0;
        assert_eq!(date::add_days(epoch, 1), date::MS_PER_DAY);
        assert_eq!(date::format_ymd(epoch), "1970-01-01");
        assert_eq!(date::format_iso(epoch), "1970-01-01T00:00:00.000Z");
        assert_eq!(date::parse_ymd("1970-01-02"), Some(date::MS_PER_DAY));
        assert_eq!(date::parse_iso("1970-01-01T00:00:00Z"), Some(epoch));
        assert!(date::is_leap_year(2024));
        assert_eq!(date::days_in_month(2024, 2), 29);
        assert_eq!(date::weekday(epoch), 4);
    }

    #[test]
    fn digest_algorithms_match_stable_test_vectors() {
        assert_eq!(
            crypto::sha256_hex(b"rsscript"),
            "e92c4828dba081bc0d3df48e7b834799a51a0ee7479c2ca89622bbe5a1dcb864"
        );
        assert_eq!(crypto::sha3_224(b"rsscript").len(), 28);
        assert_eq!(crypto::sha3_256(b"rsscript").len(), 32);
        assert_eq!(crypto::shake128(b"rsscript", 17).len(), 17);
        assert_eq!(
            crypto::hmac_sha256_hex(b"key", b"rsscript"),
            "b8eea96b9c160cf61f841cac85540ab416eb5d58ebd4ade171ff9da3c7884927"
        );
    }

    #[test]
    fn gzip_decompression_is_a_pure_byte_transform() {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"rsscript corelib").unwrap();
        let encoded = encoder.finish().unwrap();
        assert_eq!(
            compression::gzip_decompress(&encoded).unwrap(),
            b"rsscript corelib"
        );
        assert!(compression::gzip_decompress(b"not gzip").is_err());
    }
}
