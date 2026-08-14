#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::time::{Duration, Instant};

use rsscript_abi_model::ExternalSymbol;
use rsscript_provider_api::{
    ProviderError, ProviderFunction, WireCallTypeTable, WireInterpreterFn, WireValue,
};

include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Host-configured HTTP capability. Only origins explicitly supplied at
/// construction can be reached. The host supplies a preconfigured client
/// builder; this Provider installs a final redirect policy that applies the
/// same allowlist to every redirect hop.
#[derive(Clone)]
pub struct HttpProvider {
    client: reqwest::blocking::Client,
    allowed_origins: BTreeSet<String>,
    max_response_bytes: usize,
}

impl HttpProvider {
    pub fn new(
        client: reqwest::blocking::ClientBuilder,
        allowed_origins: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Self, ProviderError> {
        let allowed_origins = allowed_origins
            .into_iter()
            .map(|origin| parse_origin(origin.as_ref()))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let redirect_origins = allowed_origins.clone();
        let client = client
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                let origin = attempt.url().origin().ascii_serialization();
                if redirect_origins.contains(&origin) {
                    attempt.follow()
                } else {
                    attempt.error(format!("redirect origin `{origin}` is not allowed"))
                }
            }))
            .build()
            .map_err(|error| ProviderError::internal(format!("build HTTP client: {error}")))?;
        Ok(Self {
            client,
            allowed_origins,
            max_response_bytes: MAX_RESPONSE_BYTES,
        })
    }

    pub fn with_max_response_bytes(mut self, limit: usize) -> Self {
        self.max_response_bytes = limit.min(MAX_RESPONSE_BYTES);
        self
    }

    pub fn functions(&self) -> BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>> {
        let contract = descriptor();
        let function = contract.functions.into_iter().next().unwrap();
        let response_type = WireCallTypeTable::for_signature(&function.signature)
            .and_then(|types| types.with_record_layouts(contract.record_layouts))
            .expect("generated HTTP descriptor has a valid wire layout")
            .type_id(&rsscript_abi_model::WireType::from("host.http.HttpResponse"))
            .expect("HTTP response record is present in the generated wire layout");
        let provider = self.clone();
        BTreeMap::from([(
            function.symbol,
            ProviderFunction {
                signature: function.signature,
                callable: WireInterpreterFn::new_contextual(move |context, mut values| {
                    context.check_cancelled()?;
                    let [WireValue::String { value: url }] = values.as_mut_slice() else {
                        return Err(ProviderError::invalid_argument("url must be String"));
                    };
                    let url = reqwest::Url::parse(url).map_err(|error| {
                        ProviderError::invalid_argument(format!("invalid HTTP URL: {error}"))
                    })?;
                    let origin = url.origin().ascii_serialization();
                    if !provider.allowed_origins.contains(&origin) {
                        return Err(ProviderError::new(
                            rsscript_provider_api::ProviderErrorCode::PermissionDenied,
                            format!("HTTP origin `{origin}` is not configured for this Provider"),
                        ));
                    }
                    let deadline_timeout = context.deadline.map(|deadline| {
                        deadline.instant().saturating_duration_since(Instant::now())
                    });
                    let timeout = deadline_timeout
                        .unwrap_or(DEFAULT_REQUEST_TIMEOUT)
                        .min(DEFAULT_REQUEST_TIMEOUT);
                    let deadline_controls_timeout = deadline_timeout
                        .is_some_and(|deadline| deadline <= DEFAULT_REQUEST_TIMEOUT);
                    let mut response = provider
                        .client
                        .get(url)
                        .timeout(timeout)
                        .send()
                        .map_err(|error| http_request_error(error, deadline_controls_timeout))?;
                    let status = i64::from(response.status().as_u16());
                    let limit = context
                        .remaining_byte_budget
                        .into_iter()
                        .chain(context.remaining_output_budget)
                        .chain([provider.max_response_bytes])
                        .min()
                        .unwrap_or(provider.max_response_bytes);
                    if response
                        .content_length()
                        .is_some_and(|length| length > limit as u64)
                    {
                        return Err(response_too_large(limit));
                    }
                    let body = read_response_bounded(&mut response, limit)?;
                    Ok(WireValue::Record {
                        type_id: response_type,
                        fields: vec![
                            WireValue::Int { value: status },
                            WireValue::String { value: body },
                        ],
                    })
                }),
            },
        )])
    }
}

fn http_request_error(error: reqwest::Error, deadline_controls_timeout: bool) -> ProviderError {
    if error.is_timeout() && deadline_controls_timeout {
        ProviderError::new(
            rsscript_provider_api::ProviderErrorCode::DeadlineExceeded,
            format!("HTTP deadline exceeded: {error}"),
        )
    } else {
        ProviderError::unavailable(format!("HTTP GET: {error}"))
    }
}

fn parse_origin(value: &str) -> Result<String, ProviderError> {
    let url = reqwest::Url::parse(value).map_err(|error| {
        ProviderError::invalid_argument(format!("invalid HTTP origin: {error}"))
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(ProviderError::invalid_argument(
            "HTTP origin must use http or https and include a host",
        ));
    }
    Ok(url.origin().ascii_serialization())
}

fn read_response_bounded(response: &mut impl Read, limit: usize) -> Result<String, ProviderError> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    response
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| ProviderError::from_io("read HTTP response", error))?;
    if bytes.len() > limit {
        return Err(response_too_large(limit));
    }
    String::from_utf8(bytes).map_err(|error| {
        ProviderError::invalid_argument(format!("HTTP body is not UTF-8: {error}"))
    })
}

fn response_too_large(limit: usize) -> ProviderError {
    ProviderError::resource_exhausted(format!("HTTP response exceeds {limit} bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn conforms_to_provider_contract_without_network_access() {
        let provider = HttpProvider::new(
            reqwest::blocking::Client::builder(),
            ["https://example.com"],
        )
        .unwrap();
        let report = rsscript_provider_conformance::assert_wire_provider_conforms(
            descriptor(),
            provider.functions(),
        );
        assert_eq!(report.provider_id, "rsscript.http");
    }

    #[test]
    fn disallowed_origin_fails_before_network_access() {
        let provider = HttpProvider::new(
            reqwest::blocking::Client::builder(),
            ["https://allowed.example"],
        )
        .unwrap();
        let function = provider.functions().into_values().next().unwrap();
        let mut context = rsscript_provider_api::ProviderCallContext {
            blocking_allowed: true,
            ..rsscript_provider_api::ProviderCallContext::default()
        };
        let error = function
            .callable
            .call_with_context(
                &mut context,
                vec![WireValue::String {
                    value: "https://denied.example/path".into(),
                }],
            )
            .unwrap_err();
        assert_eq!(
            error.code,
            rsscript_provider_api::ProviderErrorCode::PermissionDenied
        );
    }

    #[test]
    fn response_reader_rejects_body_over_budget() {
        let mut body = std::io::Cursor::new(b"0123456789".to_vec());
        let error = read_response_bounded(&mut body, 4).unwrap_err();
        assert_eq!(
            error.code,
            rsscript_provider_api::ProviderErrorCode::ResourceExhausted
        );
    }

    #[test]
    fn execution_deadline_maps_to_structured_provider_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(100));
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        });
        let origin = format!("http://{address}");
        let provider = HttpProvider::new(reqwest::blocking::Client::builder(), [&origin]).unwrap();
        let function = provider.functions().into_values().next().unwrap();
        let mut context = rsscript_provider_api::ProviderCallContext {
            deadline: Some(rsscript_provider_api::MonotonicDeadline::after(
                Duration::from_millis(20),
            )),
            blocking_allowed: true,
            ..rsscript_provider_api::ProviderCallContext::default()
        };
        let error = function
            .callable
            .call_with_context(
                &mut context,
                vec![WireValue::String {
                    value: format!("{origin}/slow"),
                }],
            )
            .unwrap_err();
        assert_eq!(
            error.code,
            rsscript_provider_api::ProviderErrorCode::DeadlineExceeded
        );
        server.join().unwrap();
    }
}
