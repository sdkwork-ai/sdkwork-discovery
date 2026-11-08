//! Database-backed HTTP service registry client for SDKWork backend SDKs.
//!
//! `DISCOVERY_SPEC.md` owns registry identity, lease, and discovery semantics.
//! This crate is the **direct-PostgreSQL client** for HTTP (`protocol = http`)
//! surfaces: a backend process registers its own instance/endpoint, renews the
//! lease with heartbeats, and resolves peer HTTP base URLs from the same table
//! the `sdkwork-discovery` control plane owns.
//!
//! The client is deliberately small and cache-first:
//!
//! - Registration is an upsert against
//!   `uk_discovery_service_instance_identity`, so repeated startup or a
//!   re-register after a transient database failure never duplicates a row.
//! - A live instance keeps its lease id across re-registration; the lease only
//!   rotates after the previous lease expired or was deregistered.
//! - [`HttpRegistryResolver`] refreshes a local snapshot in the background and
//!   answers [`sdkwork_utils_rust::service_base_url::HttpServiceRegistry`]
//!   synchronously, so SDK call sites never pay a database round trip.
//! - A target that fails `failure_threshold` consecutive calls is demoted for a
//!   bounded cooldown; resolution then prefers the next healthy instance and
//!   degrades to the authored topology value when none is available
//!   (`QoS` + graceful degradation).

mod advertise;
mod registry;
mod resolver;
mod schema;
mod selection;

pub use advertise::{
    advertised_http_endpoint, detect_lan_ip, select_endpoint, ENV_ADVERTISE_IP,
    ENV_GATEWAY_ADVERTISE_IP, ENV_GATEWAY_PRIVATE_HTTP_URL, ENV_PRIVATE_HTTP_URL,
};
pub use registry::{
    HttpInstanceRegistration, HttpRegistry, HttpTarget, RegisterHttpInstance,
    RegisteredHttpInstance, RegistryError, RegistryIdentity, RegistryStatus,
};
pub use resolver::{
    probe_http_endpoint, HttpRegistryConfig, HttpRegistryResolver, DEFAULT_FAILURE_COOLDOWN_MS,
    DEFAULT_FAILURE_THRESHOLD, DEFAULT_REFRESH_INTERVAL_MS,
};
pub use schema::REGISTRY_TABLE_DDL;
pub use selection::{select_target, select_target_with, LoadBalanceStrategy, TargetState};

use std::sync::Arc;

use sdkwork_utils_rust::service_base_url::install_http_service_registry;

/// Environment key for a dedicated discovery database.
pub const ENV_DISCOVERY_DATABASE_URL: &str = "SDKWORK_DISCOVERY_DATABASE_URL";
/// Environment key for the application database (registry fallback).
pub const ENV_DATABASE_URL: &str = "SDKWORK_DATABASE_URL";
/// Environment key for the registry namespace.
pub const ENV_DISCOVERY_NAMESPACE: &str = "SDKWORK_DISCOVERY_NAMESPACE";
/// Environment key for the lifecycle environment.
pub const ENV_ENVIRONMENT: &str = "SDKWORK_ENVIRONMENT";

/// Builds the registry identity from the process environment.
///
/// Namespace defaults to `platform`; environment defaults to the canonical
/// `SDKWORK_ENVIRONMENT` value and finally `development`.
pub fn registry_identity_from_env() -> RegistryIdentity {
    registry_identity_from_env_or("development")
}

/// Builds the registry identity with a caller-supplied environment fallback.
///
/// A gateway passes its resolved runtime environment so registration and
/// consumption agree even when `SDKWORK_ENVIRONMENT` is absent; consumers keep
/// the deprecated gateway aliases working during migration.
pub fn registry_identity_from_env_or(default_environment: &str) -> RegistryIdentity {
    let namespace = std::env::var(ENV_DISCOVERY_NAMESPACE)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "platform".to_owned());
    let environment = [
        ENV_ENVIRONMENT,
        "SDKWORK_API_CLOUD_GATEWAY_ENVIRONMENT",
        "GATEWAY_ENVIRONMENT",
    ]
    .into_iter()
    .find_map(|key| {
        std::env::var(key)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
    .unwrap_or_else(|| default_environment.trim().to_owned());
    RegistryIdentity::new(namespace, environment)
}

/// Installs the registry-backed resolver for a backend that **consumes** peer
/// HTTP services but does not host the registry itself.
///
/// This is the one-line consumer bootstrap mandated by `DISCOVERY_SPEC.md`
/// section 12.1: it connects to the registry database (dedicated discovery
/// database first, application database second), ensures the table exists,
/// primes one snapshot, and installs the process-global resolver. Returns `None`
/// when no database URL is configured; callers keep their authored topology
/// values (degradation) and must not fail startup.
pub async fn install_resolver_from_env(
    identity: RegistryIdentity,
    mut config: HttpRegistryConfig,
) -> Option<Arc<HttpRegistryResolver>> {
    let database_url = [ENV_DISCOVERY_DATABASE_URL, ENV_DATABASE_URL]
        .into_iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })?;
    if config.health_probe_path.is_none() {
        config.health_probe_path = std::env::var("SDKWORK_DISCOVERY_HTTP_REGISTRY_HEALTH_PATH")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
    }
    if let Ok(value) = std::env::var("SDKWORK_DISCOVERY_HTTP_REGISTRY_HEALTH_TIMEOUT_MS") {
        if let Ok(timeout_ms) = value.trim().parse::<u64>() {
            config.health_probe_timeout_ms = timeout_ms.max(250);
        }
    }
    if let Ok(value) = std::env::var("SDKWORK_DISCOVERY_HTTP_REGISTRY_LOAD_BALANCER") {
        match LoadBalanceStrategy::parse(&value) {
            Some(strategy) => config.load_balancer = strategy,
            None => tracing::warn!(
                value = %value,
                supported = ?LoadBalanceStrategy::SUPPORTED,
                "unknown service registry load balancer; using priority"
            ),
        }
    }
    let registry = HttpRegistry::connect(&database_url, identity, 2)
        .await
        .ok()?;
    if registry.ensure_schema().await.is_err() {
        return None;
    }
    let resolver = HttpRegistryResolver::new(registry, config);
    if let Err(error) = resolver.refresh_once().await {
        tracing::warn!(%error, "initial service registry snapshot failed; using topology values");
    }
    if !install_http_service_registry(resolver.clone()) {
        tracing::warn!("an HTTP service registry was already installed; keeping the first view");
    }
    let _refresh = resolver.start();
    Some(resolver)
}
