use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use crate::{
    OperationContext, ResourceBudget, RuntimeServices, cancellation_token_cancelled,
    deadline_remaining_duration,
};

pub trait NetworkTargetPolicy: Send + Sync {
    fn authorize(&self, hostname: &str, port: u16, resolved: &[IpAddr]) -> Result<(), String>;
}

#[derive(Debug, Default)]
pub struct AllowAllNetworkTargetPolicy;

impl NetworkTargetPolicy for AllowAllNetworkTargetPolicy {
    fn authorize(&self, _hostname: &str, _port: u16, _resolved: &[IpAddr]) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct DenyPrivateNetworkTargetPolicy;

impl NetworkTargetPolicy for DenyPrivateNetworkTargetPolicy {
    fn authorize(&self, _hostname: &str, _port: u16, resolved: &[IpAddr]) -> Result<(), String> {
        if resolved.is_empty() {
            return Err("network target did not resolve to an address".to_string());
        }
        if resolved
            .iter()
            .any(|address| !is_public_network_address(*address))
        {
            return Err("network target resolves to a non-public address".to_string());
        }
        Ok(())
    }
}

fn is_public_network_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !(address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_broadcast()
                || address.is_documentation()
                || address.is_multicast()
                || address.is_unspecified())
        }
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_public_network_address(IpAddr::V4(mapped));
            }
            !(address.is_loopback()
                || address.is_multicast()
                || address.is_unspecified()
                || address.is_unique_local()
                || address.is_unicast_link_local())
        }
    }
}

pub(crate) fn authorize_resolved_target(
    policy: &dyn NetworkTargetPolicy,
    endpoint: &NetworkEndpoint,
    addresses: &[SocketAddr],
) -> Result<(), String> {
    let mut resolved = addresses.iter().map(SocketAddr::ip).collect::<Vec<_>>();
    resolved.sort_unstable();
    resolved.dedup();
    policy.authorize(endpoint.hostname(), endpoint.port(), &resolved)
}

pub(crate) struct NetworkEndpoint {
    hostname: String,
    port: u16,
}

impl NetworkEndpoint {
    pub(crate) fn from_host_and_port(hostname: &str, port: i64) -> Option<Self> {
        let port = u16::try_from(port).ok().filter(|port| *port != 0)?;
        Some(Self {
            hostname: hostname.to_string(),
            port,
        })
    }

    pub(crate) fn from_optional_host(
        hostname: Option<&str>,
        port: Option<u16>,
        default_port: u16,
    ) -> Option<Self> {
        Some(Self {
            hostname: hostname?.to_string(),
            port: port.unwrap_or(default_port),
        })
    }

    pub(crate) fn hostname(&self) -> &str {
        &self.hostname
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }
}

pub(crate) struct NetworkOperationContext {
    resources: OperationContext,
}

impl NetworkOperationContext {
    pub(crate) fn new(resources: OperationContext) -> Self {
        Self { resources }
    }

    pub(crate) fn byte_budget(&self) -> &ResourceBudget {
        self.resources.byte_budget()
    }

    pub(crate) fn services(&self) -> &Arc<RuntimeServices> {
        self.resources.services()
    }

    pub(crate) async fn run<T>(&self, future: impl Future<Output = T>) -> Result<T, ControlError> {
        let remaining = deadline_remaining_duration(self.resources.deadline());
        if remaining.is_zero() {
            return Err(ControlError::DeadlineExpired);
        }
        tokio::select! {
            biased;
            _ = cancellation_token_cancelled(self.resources.cancellation()) => {
                Err(ControlError::Cancelled)
            }
            result = tokio::time::timeout(remaining, future) => {
                result.map_err(|_| ControlError::DeadlineExpired)
            }
        }
    }
}

pub(crate) enum ControlError {
    Cancelled,
    DeadlineExpired,
}

impl ControlError {
    pub(crate) fn message(&self, protocol: &str, operation: &str) -> String {
        match self {
            Self::Cancelled => format!("{protocol} {operation} was cancelled"),
            Self::DeadlineExpired => format!("{protocol} {operation} deadline expired"),
        }
    }
}
