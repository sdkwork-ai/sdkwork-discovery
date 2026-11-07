use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::schema::REGISTRY_TABLE_DDL;

/// Errors produced by the direct-PostgreSQL registry client.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// The database rejected or could not serve the operation.
    #[error("registry database error: {0}")]
    Database(#[from] sqlx::Error),
    /// A registration input is invalid.
    #[error("invalid registry input: {0}")]
    InvalidInput(String),
    /// Metadata could not be serialized.
    #[error("registry metadata serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

/// `namespace + environment` partition of one registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryIdentity {
    /// Tenant or platform partition (`DISCOVERY_SPEC.md` section 4).
    pub namespace: String,
    /// Lifecycle environment such as `production`.
    pub environment: String,
}

impl RegistryIdentity {
    /// Builds a validated identity.
    pub fn new(namespace: impl Into<String>, environment: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            environment: environment.into(),
        }
    }
}

/// Reported serving state of a registered instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryStatus {
    /// Fully serving traffic.
    Serving,
    /// Serving but intentionally degraded (for example during warm-up).
    Degraded,
    /// Registered but not accepting traffic.
    NotServing,
}

impl RegistryStatus {
    /// Stable wire value stored in `discovery_service_instance.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Serving => "serving",
            Self::Degraded => "degraded",
            Self::NotServing => "not_serving",
        }
    }

    /// Parses a stored status; unknown values are treated as not serving so a
    /// schema drift fails closed instead of routing to an unknown state.
    pub fn from_wire(value: &str) -> Self {
        match value {
            "serving" => Self::Serving,
            "degraded" => Self::Degraded,
            _ => Self::NotServing,
        }
    }

    /// Whether discovery may return this instance.
    pub fn is_discoverable(self) -> bool {
        matches!(self, Self::Serving | Self::Degraded)
    }
}

/// One HTTP instance registration request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpInstanceRegistration {
    /// Canonical service name (for example `cloudrouter`).
    pub service_name: String,
    /// Stable per-instance identity for one process and port.
    pub instance_id: String,
    /// Absolute `http(s)` base URL of the instance, no trailing slash.
    pub endpoint: String,
    /// Service binary or contract version.
    pub version: String,
    /// Deployment region.
    pub region: String,
    /// Deployment zone.
    pub zone: String,
    /// Relative traffic weight inside the same priority group.
    pub weight: u32,
    /// Lower values are preferred.
    pub priority: u32,
    /// Reported serving state.
    pub status: RegistryStatus,
    /// Structured metadata (`deployment_profile`, `runtime_target`, ...).
    pub metadata: BTreeMap<String, String>,
    /// Lease duration; the heartbeat loop renews well before it elapses.
    pub lease_ttl_seconds: u64,
}

impl HttpInstanceRegistration {
    /// Builds a serving registration with the standard lease.
    pub fn serving(
        service_name: impl Into<String>,
        instance_id: impl Into<String>,
        endpoint: impl Into<String>,
        version: impl Into<String>,
        lease_ttl_seconds: u64,
    ) -> Self {
        Self {
            service_name: service_name.into(),
            instance_id: instance_id.into(),
            endpoint: endpoint.into(),
            version: version.into(),
            region: String::new(),
            zone: String::new(),
            weight: 100,
            priority: 100,
            status: RegistryStatus::Serving,
            metadata: BTreeMap::new(),
            lease_ttl_seconds,
        }
    }

    fn validate(&self) -> Result<(), RegistryError> {
        for (field, value) in [
            ("service_name", self.service_name.as_str()),
            ("instance_id", self.instance_id.as_str()),
            ("version", self.version.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(RegistryError::InvalidInput(format!(
                    "{field} must not be empty"
                )));
            }
            if value.chars().any(char::is_control) {
                return Err(RegistryError::InvalidInput(format!(
                    "{field} must not contain control characters"
                )));
            }
        }
        if !is_absolute_http_origin(&self.endpoint) {
            return Err(RegistryError::InvalidInput(format!(
                "endpoint must be an absolute http(s) origin, got `{}`",
                self.endpoint
            )));
        }
        if self.lease_ttl_seconds == 0 {
            return Err(RegistryError::InvalidInput(
                "lease_ttl_seconds must be greater than zero".to_owned(),
            ));
        }
        Ok(())
    }
}

/// A registration that owns a lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredHttpInstance {
    /// Lease id to renew with [`HttpRegistry::heartbeat`].
    pub lease_id: String,
    /// Instance identity the lease belongs to.
    pub instance_id: String,
    /// Registered endpoint.
    pub endpoint: String,
    /// Absolute lease expiry in epoch milliseconds.
    pub expires_at_ms: u64,
}

/// One discoverable HTTP target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpTarget {
    /// Instance identity.
    pub instance_id: String,
    /// Absolute `http(s)` base URL.
    pub endpoint: String,
    /// Lower values are preferred.
    pub priority: u32,
    /// Relative weight inside the priority group.
    pub weight: u32,
    /// Reported serving state.
    pub status: RegistryStatus,
}

/// Shorthand alias used by callers that read as `register(...)`.
pub type RegisterHttpInstance = HttpInstanceRegistration;

/// Direct-PostgreSQL HTTP service registry.
///
/// Every method is a small, explicit SQL operation against
/// `discovery_service_instance`; there is no in-process state here. Caching and
/// failure demotion live in [`crate::HttpRegistryResolver`].
#[derive(Debug, Clone)]
pub struct HttpRegistry {
    pool: PgPool,
    identity: RegistryIdentity,
}

impl HttpRegistry {
    /// Wraps a PostgreSQL pool for one namespace/environment partition.
    pub fn new(pool: PgPool, identity: RegistryIdentity) -> Self {
        Self { pool, identity }
    }

    /// Connects to the registry database.
    ///
    /// A backend whose main pool uses a different SQLx major version, or whose
    /// registry lives in a separate database, uses this constructor instead of
    /// sharing its application pool. The pool is intentionally small: the client
    /// is registration/heartbeat plus a periodic snapshot refresh, not a
    /// per-request data path.
    pub async fn connect(
        database_url: &str,
        identity: RegistryIdentity,
        max_connections: u32,
    ) -> Result<Self, RegistryError> {
        if database_url.trim().is_empty() {
            return Err(RegistryError::InvalidInput(
                "registry database URL must not be empty".to_owned(),
            ));
        }
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(max_connections.max(1))
            .connect(database_url)
            .await?;
        Ok(Self { pool, identity })
    }

    /// The partition this client reads and writes.
    pub fn identity(&self) -> &RegistryIdentity {
        &self.identity
    }

    /// Closes the underlying pool. Test/diagnostic helper; production clients
    /// live for the process lifetime.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Creates the canonical registry table and indexes when a backend hosts
    /// the registry in its own schema. Idempotent.
    pub async fn ensure_schema(&self) -> Result<(), RegistryError> {
        sqlx::raw_sql(REGISTRY_TABLE_DDL)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Idempotent registration.
    ///
    /// The unique identity `(namespace, environment, service_name,
    /// instance_id)` makes repeated startup a no-op row-wise. A still-live
    /// instance keeps its existing lease id, so a heartbeat loop that races a
    /// re-register does not invalidate the lease it holds.
    pub async fn register(
        &self,
        registration: &HttpInstanceRegistration,
    ) -> Result<RegisteredHttpInstance, RegistryError> {
        registration.validate()?;
        let now_ms = now_ms();
        let ttl_ms = i64::try_from(registration.lease_ttl_seconds)
            .ok()
            .and_then(|seconds| seconds.checked_mul(1000))
            .ok_or_else(|| {
                RegistryError::InvalidInput("lease_ttl_seconds is too large".to_owned())
            })?;
        let expires_at_ms = now_ms.saturating_add(ttl_ms);
        let metadata_json = serde_json::to_string(&registration.metadata)?;
        let row = sqlx::query(REGISTER_HTTP_INSTANCE)
            .bind(Uuid::new_v4().to_string())
            .bind(&self.identity.namespace)
            .bind(&self.identity.environment)
            .bind(&registration.service_name)
            .bind(&registration.instance_id)
            .bind(&registration.endpoint)
            .bind(registration.version.as_str())
            .bind(registration.region.as_str())
            .bind(registration.zone.as_str())
            .bind(i64::from(registration.weight))
            .bind(i64::from(registration.priority))
            .bind(registration.status.as_str())
            .bind(metadata_json)
            .bind(new_lease_id())
            .bind(expires_at_ms)
            .bind(now_ms)
            .bind(now_ms)
            .fetch_one(&self.pool)
            .await?;
        registered_from_row(&row)
    }

    /// Renews a lease.
    ///
    /// Returns `None` when the lease no longer exists or already expired; the
    /// caller must then re-register (the instance row may have been reaped).
    pub async fn heartbeat(
        &self,
        lease_id: &str,
        lease_ttl_seconds: u64,
    ) -> Result<Option<RegisteredHttpInstance>, RegistryError> {
        if lease_id.trim().is_empty() {
            return Err(RegistryError::InvalidInput(
                "lease_id must not be empty".to_owned(),
            ));
        }
        let now_ms = now_ms();
        let ttl_ms = i64::try_from(lease_ttl_seconds)
            .ok()
            .and_then(|seconds| seconds.checked_mul(1000))
            .ok_or_else(|| {
                RegistryError::InvalidInput("lease_ttl_seconds is too large".to_owned())
            })?;
        let expires_at_ms = now_ms.saturating_add(ttl_ms);
        let row = sqlx::query(HEARTBEAT_HTTP_INSTANCE)
            .bind(lease_id)
            .bind(expires_at_ms)
            .bind(now_ms)
            .bind(now_ms)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(registered_from_row).transpose()
    }

    /// Soft-deletes a bounded batch of expired rows.
    ///
    /// The discovery control plane remains the authoritative reaper
    /// (`DISCOVERY_SPEC.md` §4); this producer-side sweep keeps an embedded
    /// deployment from accumulating dead rows when no control plane runs.
    pub async fn reap_expired(&self, limit: u32) -> Result<u64, RegistryError> {
        let result = sqlx::query(REAP_EXPIRED_HTTP_INSTANCES)
            .bind(now_ms())
            .bind(i64::from(limit.max(1)))
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Idempotent soft deregistration; returns whether a live row was changed.
    pub async fn deregister(
        &self,
        service_name: &str,
        instance_id: &str,
    ) -> Result<bool, RegistryError> {
        if service_name.trim().is_empty() || instance_id.trim().is_empty() {
            return Err(RegistryError::InvalidInput(
                "service_name and instance_id must not be empty".to_owned(),
            ));
        }
        let result = sqlx::query(DEREGISTER_HTTP_INSTANCE)
            .bind(&self.identity.namespace)
            .bind(&self.identity.environment)
            .bind(service_name)
            .bind(instance_id)
            .bind(now_ms())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Reports the serving status of one instance (availability transition).
    pub async fn report_status(
        &self,
        service_name: &str,
        instance_id: &str,
        status: RegistryStatus,
    ) -> Result<bool, RegistryError> {
        let result = sqlx::query(REPORT_HTTP_STATUS)
            .bind(&self.identity.namespace)
            .bind(&self.identity.environment)
            .bind(service_name)
            .bind(instance_id)
            .bind(status.as_str())
            .bind(now_ms())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Returns every live, discoverable HTTP target for `service_name`,
    /// ordered by QoS preference (priority ascending, weight descending).
    pub async fn discover(&self, service_name: &str) -> Result<Vec<HttpTarget>, RegistryError> {
        if service_name.trim().is_empty() {
            return Err(RegistryError::InvalidInput(
                "service_name must not be empty".to_owned(),
            ));
        }
        let rows = sqlx::query(DISCOVER_HTTP_INSTANCES)
            .bind(&self.identity.namespace)
            .bind(&self.identity.environment)
            .bind(service_name)
            .bind(now_ms())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(target_from_row).collect()
    }

    /// Lists every service name with at least one live HTTP instance.
    pub async fn discover_services(&self) -> Result<Vec<String>, RegistryError> {
        let rows = sqlx::query(DISCOVER_HTTP_SERVICES)
            .bind(&self.identity.namespace)
            .bind(&self.identity.environment)
            .bind(now_ms())
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| row.try_get::<String, _>("service_name").map_err(Into::into))
            .collect()
    }
}

fn registered_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<RegisteredHttpInstance, RegistryError> {
    Ok(RegisteredHttpInstance {
        lease_id: row.try_get("lease_id")?,
        instance_id: row.try_get("instance_id")?,
        endpoint: row.try_get("endpoint")?,
        expires_at_ms: u64::try_from(row.try_get::<i64, _>("expires_at_ms")?).unwrap_or(0),
    })
}

fn target_from_row(row: &sqlx::postgres::PgRow) -> Result<HttpTarget, RegistryError> {
    let status: String = row.try_get("status")?;
    Ok(HttpTarget {
        instance_id: row.try_get("instance_id")?,
        endpoint: row.try_get("endpoint")?,
        priority: u32::try_from(row.try_get::<i32, _>("priority")?).unwrap_or(100),
        weight: u32::try_from(row.try_get::<i32, _>("weight")?).unwrap_or(100),
        status: RegistryStatus::from_wire(&status),
    })
}

/// Epoch milliseconds, saturating at zero before the Unix epoch.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn new_lease_id() -> String {
    format!("lease-{}", Uuid::new_v4())
}

fn is_absolute_http_origin(value: &str) -> bool {
    let trimmed = value.trim();
    let Some(rest) = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
    else {
        return false;
    };
    !rest.is_empty()
        && !rest.starts_with('/')
        && !rest.chars().any(char::is_whitespace)
        && !trimmed.ends_with('/')
}

const REGISTER_HTTP_INSTANCE: &str = r#"
INSERT INTO discovery_service_instance (
    uuid,
    namespace,
    environment,
    service_name,
    instance_id,
    endpoint,
    protocol,
    service_version,
    region,
    zone,
    weight,
    priority,
    status,
    metadata_json,
    lease_id,
    expires_at_ms,
    revision
) VALUES (
    $1, $2, $3, $4, $5, $6, 'http', $7, $8, $9, $10, $11, $12, $13::jsonb, $14, $15, $16
)
ON CONFLICT ON CONSTRAINT uk_discovery_service_instance_identity
DO UPDATE SET
    endpoint = EXCLUDED.endpoint,
    protocol = EXCLUDED.protocol,
    service_version = EXCLUDED.service_version,
    region = EXCLUDED.region,
    zone = EXCLUDED.zone,
    weight = EXCLUDED.weight,
    priority = EXCLUDED.priority,
    status = EXCLUDED.status,
    metadata_json = EXCLUDED.metadata_json,
    lease_id = CASE
        WHEN discovery_service_instance.deleted_at IS NULL
         AND discovery_service_instance.expires_at_ms > $17
        THEN discovery_service_instance.lease_id
        ELSE EXCLUDED.lease_id
    END,
    expires_at_ms = EXCLUDED.expires_at_ms,
    revision = GREATEST(discovery_service_instance.revision + 1, EXCLUDED.revision),
    updated_at = now(),
    version = discovery_service_instance.version + 1,
    deleted_at = NULL
RETURNING lease_id, instance_id, endpoint, expires_at_ms
"#;

const HEARTBEAT_HTTP_INSTANCE: &str = r#"
UPDATE discovery_service_instance
SET expires_at_ms = $2,
    revision = GREATEST(revision + 1, $3),
    updated_at = now(),
    version = version + 1
WHERE lease_id = $1
  AND deleted_at IS NULL
  AND expires_at_ms > $4
RETURNING lease_id, instance_id, endpoint, expires_at_ms
"#;

const DEREGISTER_HTTP_INSTANCE: &str = r#"
UPDATE discovery_service_instance
SET deleted_at = now(),
    revision = GREATEST(revision + 1, $5),
    updated_at = now(),
    version = version + 1
WHERE namespace = $1
  AND environment = $2
  AND service_name = $3
  AND instance_id = $4
  AND deleted_at IS NULL
"#;

const REPORT_HTTP_STATUS: &str = r#"
UPDATE discovery_service_instance
SET status = $5,
    revision = GREATEST(revision + 1, $6),
    updated_at = now(),
    version = version + 1
WHERE namespace = $1
  AND environment = $2
  AND service_name = $3
  AND instance_id = $4
  AND deleted_at IS NULL
"#;

const DISCOVER_HTTP_INSTANCES: &str = r#"
SELECT instance_id, endpoint, priority, weight, status
FROM discovery_service_instance
WHERE namespace = $1
  AND environment = $2
  AND service_name = $3
  AND protocol = 'http'
  AND deleted_at IS NULL
  AND expires_at_ms > $4
  AND status IN ('serving', 'degraded')
ORDER BY priority ASC, weight DESC, instance_id ASC
"#;

const DISCOVER_HTTP_SERVICES: &str = r#"
SELECT DISTINCT service_name
FROM discovery_service_instance
WHERE namespace = $1
  AND environment = $2
  AND protocol = 'http'
  AND deleted_at IS NULL
  AND expires_at_ms > $3
  AND status IN ('serving', 'degraded')
ORDER BY service_name ASC
"#;

const REAP_EXPIRED_HTTP_INSTANCES: &str = r#"
UPDATE discovery_service_instance
SET deleted_at = now(),
    updated_at = now(),
    version = version + 1
WHERE id IN (
    SELECT id
    FROM discovery_service_instance
    WHERE deleted_at IS NULL
      AND expires_at_ms <= $1
    ORDER BY expires_at_ms ASC
    LIMIT $2
)
"#;
