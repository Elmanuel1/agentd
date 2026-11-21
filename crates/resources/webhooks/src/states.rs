//! Each endpoint's health, shared by every replica in
//! `webhook_endpoint_states`: whether a run of failures is open, and
//! whether, when and why it is disabled. Live disables an endpoint at once
//! on a 3xx or a non-public address; a run of failures disables it after
//! 120 h, Svix's window, as live states none (D3).

use std::future::Future;

use shared::BackendError;
use shared::ports::BoxFuture;

/// Why an endpoint is disabled; `webhook_endpoint_states.disabled_reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisabledReason {
    /// It answered 3xx.
    Redirect,
    /// It resolves to a non-public address.
    PrivateAddress,
    /// Every attempt failed for 120 h.
    Failing,
}

impl DisabledReason {
    /// The stored value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Redirect => "redirect",
            Self::PrivateAddress => "private_address",
            Self::Failing => "failing",
        }
    }

    /// The reason `stored` names.
    pub fn parse(stored: &str) -> Option<Self> {
        [Self::Redirect, Self::PrivateAddress, Self::Failing].into_iter().find(|r| r.as_str() == stored)
    }
}

/// A disabled endpoint, as every replica reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointState {
    /// The endpoint.
    pub endpoint_key: String,
    /// Why it is disabled.
    pub disabled: DisabledReason,
}

/// An endpoint as configured, which its state row carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configured {
    /// The endpoint.
    pub endpoint_key: String,
    /// Its organization.
    pub org_id: String,
    /// The SHA-256 of its URL and secret.
    pub digest: String,
}

/// The shared endpoint states.
pub trait EndpointStates: Send + Sync {
    /// Store each endpoint's digest; one whose URL or secret changed is enabled again.
    fn configure<'a>(&'a self, endpoints: &'a [Configured]) -> BoxFuture<'a, Result<(), BackendError>>;
    /// Every disabled endpoint.
    fn read(&self) -> BoxFuture<'_, Result<Vec<EndpointState>, BackendError>>;
    /// Count a failed attempt; `true` when this one disabled it, its run having passed 120 h.
    fn failed<'a>(&'a self, endpoint: &'a Configured) -> BoxFuture<'a, Result<bool, BackendError>>;
    /// Disable the endpoint for `why`; `true` when it was not disabled yet.
    fn disable<'a>(
        &'a self,
        endpoint: &'a Configured,
        why: DisabledReason,
    ) -> BoxFuture<'a, Result<bool, BackendError>>;
}

/// What an adapter implements to provide [`EndpointStates`]; the methods are documented there.
pub trait NativeEndpointStates: Send + Sync {
    fn configure(&self, endpoints: &[Configured]) -> impl Future<Output = Result<(), BackendError>> + Send;
    fn read(&self) -> impl Future<Output = Result<Vec<EndpointState>, BackendError>> + Send;
    fn failed(&self, endpoint: &Configured) -> impl Future<Output = Result<bool, BackendError>> + Send;
    fn disable(
        &self,
        endpoint: &Configured,
        why: DisabledReason,
    ) -> impl Future<Output = Result<bool, BackendError>> + Send;
}

impl<T: NativeEndpointStates> EndpointStates for T {
    fn configure<'a>(&'a self, endpoints: &'a [Configured]) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(NativeEndpointStates::configure(self, endpoints))
    }
    fn read(&self) -> BoxFuture<'_, Result<Vec<EndpointState>, BackendError>> {
        Box::pin(NativeEndpointStates::read(self))
    }
    fn failed<'a>(&'a self, endpoint: &'a Configured) -> BoxFuture<'a, Result<bool, BackendError>> {
        Box::pin(NativeEndpointStates::failed(self, endpoint))
    }
    fn disable<'a>(
        &'a self,
        endpoint: &'a Configured,
        why: DisabledReason,
    ) -> BoxFuture<'a, Result<bool, BackendError>> {
        Box::pin(NativeEndpointStates::disable(self, endpoint, why))
    }
}
