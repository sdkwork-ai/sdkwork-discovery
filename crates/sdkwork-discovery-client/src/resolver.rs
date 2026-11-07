use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use sdkwork_utils_rust::service_base_url::{HttpServiceRegistry, RegistryHttpTarget};

use crate::registry::{now_ms, HttpRegistry, RegistryError};
use crate::selection::{select_target, TargetState};

/// Default snapshot refresh interval.
pub const DEFAULT_REFRESH_INTERVAL_MS: u64 = 5_000;
/// Default consecutive failures before a target is demoted.
pub const DEFAULT_FAILURE_THRESHOLD: u32 = 3;
/// Default demotion cooldown.
pub const DEFAULT_FAILURE_COOLDOWN_MS: u64 = 30_000;

/// Tuning for the cached registry resolver.
#[derive(Debug, Clone)]
pub struct HttpRegistryConfig {
    /// Background snapshot refresh interval.
    pub refresh_interval_ms: u64,
    /// Consecutive failures that demote a target.
    pub failure_threshold: u32,
    /// How long a demoted target is skipped.
    pub failure_cooldown_ms: u64,
    /// Optional active health-probe path (for example `/healthz`).
    ///
    /// `None` keeps availability detection purely failure-driven. When set, each
    /// snapshot refresh probes every target and demotes a failing one after
    /// `failure_threshold` consecutive probe failures.
    pub health_probe_path: Option<String>,
    /// Per-probe timeout.
    pub health_probe_timeout_ms: u64,
}

impl Default for HttpRegistryConfig {
    fn default() -> Self {
        Self {
            refresh_interval_ms: DEFAULT_REFRESH_INTERVAL_MS,
            failure_threshold: DEFAULT_FAILURE_THRESHOLD,
            failure_cooldown_ms: DEFAULT_FAILURE_COOLDOWN_MS,
            health_probe_path: None,
            health_probe_timeout_ms: 2_000,
        }
    }
}

/// Cache-first registry resolver.
///
/// [`resolve_http`](HttpServiceRegistry::resolve_http) is synchronous and reads
/// a local snapshot; the database is only touched by [`refresh_once`](#method.refresh_once)
/// and the background loop started by [`start`](#method.start). Failures are
/// tracked per target with a bounded cooldown, so a bad instance is demoted and
/// the next resolution prefers another healthy replica.
pub struct HttpRegistryResolver {
    registry: HttpRegistry,
    config: HttpRegistryConfig,
    probe_client: Option<reqwest::Client>,
    snapshot: RwLock<HashMap<String, Vec<TargetState>>>,
    last_refresh_ms: AtomicU64,
}

/// Probes one endpoint and returns whether it answered 2xx.
///
/// Exposed for diagnostics/tests; the resolver uses it from
/// [`HttpRegistryResolver`]'s probe sweep.
pub async fn probe_http_endpoint(client: &reqwest::Client, base_url: &str, path: &str) -> bool {
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    let url = format!("{}{}", base_url.trim_end_matches('/'), path);
    match client.get(&url).send().await {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

impl HttpRegistryResolver {
    /// Creates a resolver with an empty snapshot.
    pub fn new(registry: HttpRegistry, config: HttpRegistryConfig) -> Arc<Self> {
        let probe_client = config.health_probe_path.as_ref().and_then(|_| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(
                    config.health_probe_timeout_ms.max(250),
                ))
                .build()
                .ok()
        });
        Arc::new(Self {
            registry,
            config,
            probe_client,
            snapshot: RwLock::new(HashMap::new()),
            last_refresh_ms: AtomicU64::new(0),
        })
    }

    /// The underlying registry client.
    pub fn registry(&self) -> &HttpRegistry {
        &self.registry
    }

    /// Epoch milliseconds of the last successful refresh (`0` before the first).
    pub fn last_refresh_ms(&self) -> u64 {
        self.last_refresh_ms.load(Ordering::Relaxed)
    }

    /// Refreshes every known service snapshot from the database.
    ///
    /// A failure keeps the previous snapshot: losing the database must not make
    /// a running process forget how to reach its peers. Callers should retry on
    /// the next interval.
    pub async fn refresh_once(&self) -> Result<(), RegistryError> {
        let mut services = self.registry.discover_services().await?;
        {
            let cache = self.read_snapshot();
            for service in cache.keys() {
                if !services.iter().any(|known| known == service) {
                    services.push(service.clone());
                }
            }
        }
        services.sort();
        services.dedup();

        let mut next: HashMap<String, Vec<TargetState>> = HashMap::with_capacity(services.len());
        for service in services {
            let targets = self.registry.discover(&service).await?;
            let previous = self
                .read_snapshot()
                .get(&service)
                .cloned()
                .unwrap_or_default();
            let states = targets
                .into_iter()
                .map(|target| {
                    let prior = previous
                        .iter()
                        .find(|state| state.target.instance_id == target.instance_id);
                    match prior {
                        Some(state) if state.target.endpoint == target.endpoint => state.clone(),
                        Some(state) => TargetState {
                            target,
                            consecutive_failures: state.consecutive_failures,
                            down_until_ms: state.down_until_ms,
                        },
                        None => TargetState::healthy(target),
                    }
                })
                .collect();
            next.insert(service, states);
        }
        self.probe_targets(&mut next).await;
        *self.write_snapshot() = next;
        self.last_refresh_ms
            .store(now_ms().max(0) as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Actively probes every target when a health-probe path is configured.
    ///
    /// A transport error or non-2xx response increments the same failure
    /// counter used by call-site reports; at `failure_threshold` the target is
    /// demoted for the configured cooldown. A 2xx resets the streak.
    async fn probe_targets(&self, next: &mut HashMap<String, Vec<TargetState>>) {
        let (Some(client), Some(path)) = (&self.probe_client, &self.config.health_probe_path)
        else {
            return;
        };
        let path = if path.starts_with('/') {
            path.clone()
        } else {
            format!("/{path}")
        };
        let now = now_ms().max(0) as u64;
        for (service, states) in next.iter_mut() {
            for state in states.iter_mut() {
                let healthy = probe_http_endpoint(client, &state.target.endpoint, &path).await;
                if healthy {
                    state.consecutive_failures = 0;
                    state.down_until_ms = 0;
                    continue;
                }
                state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                if state.consecutive_failures >= self.config.failure_threshold.max(1) {
                    state.down_until_ms =
                        now.saturating_add(self.config.failure_cooldown_ms.max(1_000));
                    tracing::warn!(
                        target: "sdkwork.discovery.http_registry",
                        service = %service,
                        instance_id = %state.target.instance_id,
                        base_url = %state.target.endpoint,
                        failures = state.consecutive_failures,
                        "service registry health probe failed; target demoted"
                    );
                }
            }
        }
    }

    /// Starts the background refresh loop.
    /// The returned handle is owned by the caller; aborting it stops refreshes
    /// but leaves the last snapshot in place.
    pub fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let resolver = Arc::clone(self);
        tokio::spawn(async move {
            let interval_ms = resolver.config.refresh_interval_ms.max(250);
            let mut ticker = tokio::time::interval(std::time::Duration::from_millis(interval_ms));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if let Err(error) = resolver.refresh_once().await {
                    tracing::warn!(
                        target: "sdkwork.discovery.http_registry",
                        %error,
                        "service registry refresh failed; keeping the last snapshot"
                    );
                }
            }
        })
    }

    fn read_snapshot(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, Vec<TargetState>>> {
        self.snapshot
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_snapshot(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Vec<TargetState>>> {
        self.snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl HttpServiceRegistry for HttpRegistryResolver {
    fn resolve_http(&self, service: &str) -> Option<RegistryHttpTarget> {
        let cache = self.read_snapshot();
        let states = cache.get(service)?;
        select_target(states, now_ms().max(0) as u64).map(|target| RegistryHttpTarget {
            service: service.to_owned(),
            instance_id: target.instance_id,
            base_url: target.endpoint,
        })
    }

    fn report_failure(&self, target: &RegistryHttpTarget, reason: &str) {
        let mut cache = self.write_snapshot();
        let Some(states) = cache.get_mut(&target.service) else {
            return;
        };
        let Some(state) = states
            .iter_mut()
            .find(|state| state.target.instance_id == target.instance_id)
        else {
            return;
        };
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if state.consecutive_failures >= self.config.failure_threshold.max(1) {
            state.down_until_ms =
                (now_ms().max(0) as u64).saturating_add(self.config.failure_cooldown_ms.max(1_000));
            tracing::warn!(
                target: "sdkwork.discovery.http_registry",
                service = %target.service,
                instance_id = %target.instance_id,
                base_url = %target.base_url,
                reason,
                failures = state.consecutive_failures,
                cooldown_ms = self.config.failure_cooldown_ms,
                "service registry target demoted after repeated failures"
            );
        }
    }

    fn report_success(&self, target: &RegistryHttpTarget) {
        let mut cache = self.write_snapshot();
        let Some(states) = cache.get_mut(&target.service) else {
            return;
        };
        let Some(state) = states
            .iter_mut()
            .find(|state| state.target.instance_id == target.instance_id)
        else {
            return;
        };
        state.consecutive_failures = 0;
        state.down_until_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{HttpTarget, RegistryIdentity, RegistryStatus};
    use crate::selection::TargetState;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_stub(status: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("stub listener binds");
        let address = listener.local_addr().expect("stub address");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("stub accepts");
            let mut buffer = [0_u8; 1024];
            let _ = socket.read(&mut buffer).await;
            let response =
                format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = socket.write_all(response.as_bytes()).await;
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn probe_reports_2xx_healthy_and_5xx_unhealthy() {
        let client = reqwest::Client::new();
        let healthy = spawn_stub("200 OK").await;
        assert!(probe_http_endpoint(&client, &healthy, "/healthz").await);
        let unhealthy = spawn_stub("503 Service Unavailable").await;
        assert!(!probe_http_endpoint(&client, &unhealthy, "/healthz").await);
        assert!(!probe_http_endpoint(&client, "http://127.0.0.1:1", "/healthz").await);
    }

    fn resolver(failure_threshold: u32) -> Arc<HttpRegistryResolver> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://registry.invalid:5432/registry")
            .expect("a lazy PostgreSQL pool parses without connecting");
        HttpRegistryResolver::new(
            HttpRegistry::new(pool, RegistryIdentity::new("platform", "test")),
            HttpRegistryConfig {
                failure_threshold,
                failure_cooldown_ms: 60_000,
                refresh_interval_ms: 60_000,
                ..Default::default()
            },
        )
    }

    fn target(instance: &str, priority: u32, weight: u32) -> HttpTarget {
        HttpTarget {
            instance_id: instance.to_owned(),
            endpoint: format!("https://{instance}.example.com"),
            priority,
            weight,
            status: RegistryStatus::Serving,
        }
    }

    fn seed(resolver: &HttpRegistryResolver, service: &str, targets: Vec<HttpTarget>) {
        let states = targets.into_iter().map(TargetState::healthy).collect();
        let mut snapshot = resolver
            .snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.insert(service.to_owned(), states);
    }

    #[tokio::test]
    async fn resolver_reads_the_cached_snapshot_without_database_io() {
        let resolver = resolver(1);
        seed(
            &resolver,
            "cloudrouter",
            vec![target("a", 10, 100), target("b", 10, 500)],
        );

        let selected = resolver
            .resolve_http("cloudrouter")
            .expect("a cached target resolves");
        assert_eq!(selected.instance_id, "b");
        assert_eq!(selected.base_url, "https://b.example.com");
        assert!(resolver.resolve_http("missing").is_none());
    }

    #[tokio::test]
    async fn repeated_failures_demote_a_target_until_the_cooldown_ends() {
        let resolver = resolver(2);
        seed(
            &resolver,
            "cloudrouter",
            vec![target("a", 10, 500), target("b", 10, 100)],
        );

        let first = resolver.resolve_http("cloudrouter").expect("first target");
        assert_eq!(first.instance_id, "a");
        resolver.report_failure(&first, "connection refused");
        assert_eq!(
            resolver
                .resolve_http("cloudrouter")
                .expect("still healthy")
                .instance_id,
            "a",
            "one failure is below the threshold"
        );

        resolver.report_failure(&first, "connection refused");
        assert_eq!(
            resolver
                .resolve_http("cloudrouter")
                .expect("failover target")
                .instance_id,
            "b",
            "the demoted target is skipped"
        );
    }

    #[tokio::test]
    async fn a_success_resets_the_failure_streak() {
        let resolver = resolver(2);
        seed(
            &resolver,
            "cloudrouter",
            vec![target("a", 10, 500), target("b", 10, 100)],
        );

        let first = resolver.resolve_http("cloudrouter").expect("first target");
        resolver.report_failure(&first, "timeout");
        resolver.report_success(&first);
        resolver.report_failure(&first, "timeout");
        assert_eq!(
            resolver
                .resolve_http("cloudrouter")
                .expect("target")
                .instance_id,
            "a",
            "a success resets the consecutive-failure counter"
        );
    }
}
