#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::time::{Duration, Instant};

use rsscript_abi_model::{ExternalSymbol, WireTypeId};
use rsscript_provider_api::{
    ProviderError, ProviderFunction, WireCallTypeTable, WireInterpreterFn, WireValue,
};

include!(concat!(env!("OUT_DIR"), "/provider_contract.rs"));

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Host-configured HTTP capability. Only origins explicitly supplied at
/// construction can be reached. The host supplies a preconfigured client
/// builder; this Provider installs a final redirect policy that applies the
/// same allowlist to every redirect hop.
#[derive(Clone)]
pub struct HttpProvider {
    client: reqwest::blocking::Client,
    allowed_origins: BTreeSet<String>,
    max_response_bytes: usize,
    request_slots: Arc<ConcurrencySlots>,
    worker_slots: Arc<ConcurrencySlots>,
}

/// Instance-owned bound for blocking HTTP work. Saturation fails closed with
/// `ResourceExhausted`; the synchronous Provider API never creates an unbounded
/// queue of waiting host threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpConcurrencyPolicy {
    pub max_in_flight_requests: usize,
    pub max_worker_threads: usize,
}

impl Default for HttpConcurrencyPolicy {
    fn default() -> Self {
        Self {
            max_in_flight_requests: 16,
            max_worker_threads: 16,
        }
    }
}

#[derive(Debug)]
struct ConcurrencySlots {
    active: AtomicUsize,
    limit: usize,
}

impl ConcurrencySlots {
    fn new(limit: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            limit,
        }
    }

    fn try_acquire(self: &Arc<Self>, resource: &str) -> Result<ConcurrencyPermit, ProviderError> {
        let mut active = self.active.load(Ordering::Acquire);
        loop {
            if active >= self.limit {
                return Err(ProviderError::resource_exhausted(format!(
                    "HTTP Provider {resource} concurrency limit ({}) is exhausted",
                    self.limit
                )));
            }
            match self.active.compare_exchange_weak(
                active,
                active + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(ConcurrencyPermit(Arc::clone(self))),
                Err(observed) => active = observed,
            }
        }
    }
}

#[derive(Debug)]
struct ConcurrencyPermit(Arc<ConcurrencySlots>);

impl Drop for ConcurrencyPermit {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpNetworkPolicy {
    pub https_only: bool,
    pub allow_private_addresses: bool,
}

impl HttpNetworkPolicy {
    pub const fn production() -> Self {
        Self {
            https_only: true,
            allow_private_addresses: false,
        }
    }

    /// Explicit local-development policy for loopback test servers.
    pub const fn local_development() -> Self {
        Self {
            https_only: false,
            allow_private_addresses: true,
        }
    }
}

impl HttpProvider {
    pub fn new(
        client: reqwest::blocking::ClientBuilder,
        allowed_origins: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Self, ProviderError> {
        Self::new_with_policy(client, allowed_origins, HttpNetworkPolicy::production())
    }

    pub fn new_with_policy(
        mut client: reqwest::blocking::ClientBuilder,
        allowed_origins: impl IntoIterator<Item = impl AsRef<str>>,
        policy: HttpNetworkPolicy,
    ) -> Result<Self, ProviderError> {
        let parsed_origins = allowed_origins
            .into_iter()
            .map(|origin| parse_allowed_origin(origin.as_ref(), policy))
            .collect::<Result<Vec<_>, _>>()?;
        for (url, addresses) in &parsed_origins {
            let host = url.host_str().expect("validated origin has a host");
            let resolve_host = host
                .strip_prefix('[')
                .and_then(|host| host.strip_suffix(']'))
                .unwrap_or(host);
            client = client.resolve_to_addrs(resolve_host, addresses);
        }
        let allowed_origins = parsed_origins
            .into_iter()
            .map(|(url, _)| url.origin().ascii_serialization())
            .collect::<BTreeSet<_>>();
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
            request_slots: Arc::new(ConcurrencySlots::new(
                HttpConcurrencyPolicy::default().max_in_flight_requests,
            )),
            worker_slots: Arc::new(ConcurrencySlots::new(
                HttpConcurrencyPolicy::default().max_worker_threads,
            )),
        })
    }

    pub fn with_max_response_bytes(mut self, limit: usize) -> Self {
        self.max_response_bytes = limit.min(MAX_RESPONSE_BYTES);
        self
    }

    pub fn with_concurrency_policy(mut self, policy: HttpConcurrencyPolicy) -> Self {
        self.request_slots = Arc::new(ConcurrencySlots::new(policy.max_in_flight_requests));
        self.worker_slots = Arc::new(ConcurrencySlots::new(policy.max_worker_threads));
        self
    }

    pub fn functions(&self) -> BTreeMap<ExternalSymbol, ProviderFunction<WireInterpreterFn>> {
        let contract = descriptor();
        let function = contract.functions.into_iter().next().unwrap();
        let response_type = WireCallTypeTable::for_signature(&function.signature)
            .and_then(|types| types.with_record_layouts(contract.record_layouts))
            .expect("generated HTTP descriptor has a valid wire layout")
            .type_id(&rsscript_abi_model::WireType::from(
                "host.http.HttpResponse",
            ))
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
                    let limit = context
                        .remaining_byte_budget
                        .into_iter()
                        .chain(context.remaining_output_budget)
                        .chain([provider.max_response_bytes])
                        .min()
                        .unwrap_or(provider.max_response_bytes);
                    let request_permit = provider.request_slots.try_acquire("request")?;
                    if context.cancellation.is_some() {
                        let worker_permit = provider.worker_slots.try_acquire("worker")?;
                        let request = PendingGet {
                            provider: provider.clone(),
                            url,
                            timeout,
                            deadline_controls_timeout,
                            limit,
                            response_type,
                        };
                        execute_get_cancellable(context, request, (request_permit, worker_permit))
                    } else {
                        let _request_permit = request_permit;
                        execute_get(
                            &provider,
                            url,
                            timeout,
                            deadline_controls_timeout,
                            limit,
                            response_type,
                        )
                    }
                }),
            },
        )])
    }
}

struct PendingGet {
    provider: HttpProvider,
    url: reqwest::Url,
    timeout: Duration,
    deadline_controls_timeout: bool,
    limit: usize,
    response_type: WireTypeId,
}

/// Run a blocking transport on an owned worker when a cancellation token is
/// present. This makes cancellation observable by the VM without waiting for
/// the transport timeout. The abandoned worker remains bounded by the request
/// timeout and response-size limit and cannot publish a late result.
fn execute_get_cancellable(
    context: &rsscript_provider_api::ProviderCallContext<'_>,
    request: PendingGet,
    permits: (ConcurrencyPermit, ConcurrencyPermit),
) -> Result<WireValue, ProviderError> {
    let (sender, receiver) = sync_channel(1);
    std::thread::Builder::new()
        .name("rsscript-http-provider".into())
        .spawn(move || {
            let (_request_permit, _worker_permit) = permits;
            let result = execute_get(
                &request.provider,
                request.url,
                request.timeout,
                request.deadline_controls_timeout,
                request.limit,
                request.response_type,
            );
            let _ = sender.send(result);
        })
        .map_err(|error| ProviderError::internal(format!("start HTTP worker: {error}")))?;

    loop {
        context.check_cancelled()?;
        match receiver.recv_timeout(CANCELLATION_POLL_INTERVAL) {
            Ok(result) => {
                context.check_cancelled()?;
                return result;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(ProviderError::internal(
                    "HTTP worker exited without returning a result",
                ));
            }
        }
    }
}

fn execute_get(
    provider: &HttpProvider,
    url: reqwest::Url,
    timeout: Duration,
    deadline_controls_timeout: bool,
    limit: usize,
    response_type: WireTypeId,
) -> Result<WireValue, ProviderError> {
    let mut response = provider
        .client
        .get(url)
        .timeout(timeout)
        .send()
        .map_err(|error| http_request_error(error, deadline_controls_timeout))?;
    let status = i64::from(response.status().as_u16());
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

fn parse_allowed_origin(
    value: &str,
    policy: HttpNetworkPolicy,
) -> Result<(reqwest::Url, Vec<std::net::SocketAddr>), ProviderError> {
    let url = reqwest::Url::parse(value).map_err(|error| {
        ProviderError::invalid_argument(format!("invalid HTTP origin: {error}"))
    })?;
    if url.host().is_none() || (policy.https_only && url.scheme() != "https") {
        return Err(ProviderError::invalid_argument(if policy.https_only {
            "HTTP production policy requires an https origin with a host"
        } else {
            "HTTP origin must include a host"
        }));
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ProviderError::invalid_argument(
            "HTTP origin must use http or https",
        ));
    }
    let host = url.host_str().expect("validated URL has a host");
    let port = url
        .port_or_known_default()
        .ok_or_else(|| ProviderError::invalid_argument("HTTP origin has no resolvable port"))?;
    // URL serializers bracket IPv6 literals. Resolve names through DNS, but
    // construct literal addresses directly so `[2001:db8::1]` is not handed
    // to `ToSocketAddrs` as an invalid hostname.
    let literal_host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    let addresses = match literal_host.parse::<IpAddr>() {
        Ok(address) => vec![std::net::SocketAddr::new(address, port)],
        Err(_) => (host, port)
            .to_socket_addrs()
            .map_err(|error| ProviderError::unavailable(format!("resolve HTTP origin: {error}")))?
            .collect::<Vec<_>>(),
    };
    if addresses.is_empty() {
        return Err(ProviderError::unavailable(
            "HTTP origin resolved to no addresses",
        ));
    }
    if !policy.allow_private_addresses
        && addresses
            .iter()
            .any(|address| is_private_or_special(address.ip()))
    {
        return Err(ProviderError::new(
            rsscript_provider_api::ProviderErrorCode::PermissionDenied,
            "HTTP origin resolves to a private or special-use address",
        ));
    }
    Ok((url, addresses))
}

fn is_private_or_special(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_private_or_special_v4(address),
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_private_or_special_v4(mapped);
            }
            let segments = address.segments();
            address.is_loopback()
                || address.is_unspecified()
                || address.is_multicast()
                || segments[0..6] == [0, 0, 0, 0, 0, 0] // deprecated IPv4-compatible
                || segments[0..6] == [0x0064, 0xff9b, 0, 0, 0, 0] // NAT64 well-known
                || segments[0..3] == [0x0064, 0xff9b, 0x0001] // NAT64 local-use
                || segments[0..4] == [0x0100, 0, 0, 0] // discard-only
                || (segments[0] & 0xfe00) == 0xfc00 // unique-local
                || (segments[0] & 0xffc0) == 0xfe80 // link-local
                || (segments[0] & 0xffc0) == 0xfec0 // deprecated site-local
                || (segments[0] == 0x2001 && segments[1] <= 0x01ff) // IETF assignments
                || segments[0..2] == [0x2001, 0x0db8] // documentation
                || segments[0] == 0x2002 // 6to4
                || (segments[0] & 0xfff0) == 0x3ff0 // documentation
                || segments[0] == 0x5f00 // segment-routing SIDs
        }
    }
}

fn is_private_or_special_v4(address: std::net::Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_multicast()
        || address.is_broadcast()
        || a == 0
        || (a == 100 && (64..=127).contains(&b)) // shared address space
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2) // TEST-NET-1
        || (a == 192 && b == 88 && c == 99) // deprecated 6to4 relay anycast
        || (a == 198 && (b == 18 || b == 19)) // benchmark networks
        || (a == 198 && b == 51 && c == 100) // TEST-NET-2
        || (a == 203 && b == 0 && c == 113) // TEST-NET-3
        || a >= 240 // reserved/future use
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
    fn instance_concurrency_slots_fail_closed_and_release() {
        let slots = Arc::new(ConcurrencySlots::new(1));
        let permit = slots.try_acquire("request").expect("first slot");
        let error = slots
            .try_acquire("request")
            .expect_err("second slot must fail");
        assert_eq!(
            error.code,
            rsscript_provider_api::ProviderErrorCode::ResourceExhausted
        );
        drop(permit);
        slots.try_acquire("request").expect("released slot");
    }

    #[test]
    fn ipv6_literals_are_parsed_without_dns_hostname_resolution() {
        let (url, addresses) = parse_allowed_origin(
            "https://[2606:4700:4700::1111]",
            HttpNetworkPolicy::production(),
        )
        .expect("public IPv6 literal");
        assert_eq!(url.host_str(), Some("[2606:4700:4700::1111]"));
        assert_eq!(
            addresses[0].ip(),
            "2606:4700:4700::1111".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn special_use_and_ipv4_mapped_addresses_are_rejected() {
        for address in [
            "100.64.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "::ffff:127.0.0.1",
            "64:ff9b::808:808",
            "64:ff9b:1::1",
            "100::1",
            "fec0::1",
            "2001:2::1",
            "2001:db8::1",
            "2002:0808:0808::1",
            "3fff::1",
            "5f00::1",
        ] {
            assert!(
                is_private_or_special(address.parse().unwrap()),
                "{address} must be denied by production policy"
            );
        }
        for address in ["8.8.8.8", "2606:4700:4700::1111"] {
            assert!(
                !is_private_or_special(address.parse().unwrap()),
                "{address} must remain available to production policy"
            );
        }
    }

    #[test]
    fn conforms_to_provider_contract_without_network_access() {
        let provider = HttpProvider::new_with_policy(
            reqwest::blocking::Client::builder(),
            ["http://127.0.0.1:8080"],
            HttpNetworkPolicy::local_development(),
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
        let provider = HttpProvider::new_with_policy(
            reqwest::blocking::Client::builder(),
            ["http://127.0.0.1:8080"],
            HttpNetworkPolicy::local_development(),
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
                    value: "http://127.0.0.1:8081/path".into(),
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
        let provider = HttpProvider::new_with_policy(
            reqwest::blocking::Client::builder(),
            [&origin],
            HttpNetworkPolicy::local_development(),
        )
        .unwrap();
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

    #[test]
    fn in_flight_request_observes_cancellation_promptly() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(500));
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        });
        let origin = format!("http://{address}");
        let provider = HttpProvider::new_with_policy(
            reqwest::blocking::Client::builder(),
            [&origin],
            HttpNetworkPolicy::local_development(),
        )
        .unwrap();
        let function = provider.functions().into_values().next().unwrap();
        let cancellation = rsscript_provider_api::CancellationToken::new();
        let cancellation_request = cancellation.clone();
        let cancel = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            cancellation_request.cancel();
        });
        let mut context = rsscript_provider_api::ProviderCallContext {
            cancellation: Some(&cancellation),
            blocking_allowed: true,
            ..rsscript_provider_api::ProviderCallContext::default()
        };
        let started = Instant::now();
        let error = function
            .callable
            .call_with_context(
                &mut context,
                vec![WireValue::String {
                    value: format!("{origin}/slow"),
                }],
            )
            .unwrap_err();
        let elapsed = started.elapsed();
        assert_eq!(
            error.code,
            rsscript_provider_api::ProviderErrorCode::Cancelled
        );
        assert!(
            elapsed < Duration::from_millis(300),
            "cancellation took {elapsed:?}"
        );
        cancel.join().unwrap();
        server.join().unwrap();
    }
}
