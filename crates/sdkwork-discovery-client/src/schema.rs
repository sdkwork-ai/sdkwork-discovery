//! Registry table DDL owned by `sdkwork-discovery`.
//!
//! The table is identical to `discovery_service_instance` in the discovery
//! baseline (`database/ddl/baseline/postgres/0001_discovery_baseline.sql`). A
//! backend that points its SDK clients at the discovery database never executes
//! this; a backend that hosts the registry in its own schema calls
//! [`crate::HttpRegistry::ensure_schema`] once at startup. The statement is
//! idempotent and safe to run on every boot.
pub const REGISTRY_TABLE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS discovery_service_instance (
    id BIGSERIAL NOT NULL,
    uuid VARCHAR(64) NOT NULL,
    namespace VARCHAR(128) NOT NULL,
    environment VARCHAR(64) NOT NULL,
    service_name VARCHAR(256) NOT NULL,
    instance_id VARCHAR(256) NOT NULL,
    endpoint TEXT NOT NULL,
    protocol VARCHAR(32) NOT NULL,
    service_version VARCHAR(128) NOT NULL,
    region VARCHAR(128) NOT NULL,
    zone VARCHAR(128) NOT NULL,
    weight INTEGER NOT NULL,
    priority INTEGER NOT NULL,
    status VARCHAR(32) NOT NULL,
    metadata_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    lease_id VARCHAR(128) NOT NULL,
    expires_at_ms BIGINT NOT NULL,
    revision BIGINT NOT NULL,
    health_check_json JSONB,
    health_check_state_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    version BIGINT NOT NULL DEFAULT 0,
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (id),
    CONSTRAINT uk_discovery_service_instance_identity UNIQUE (namespace, environment, service_name, instance_id),
    CONSTRAINT uk_discovery_service_instance_lease UNIQUE (lease_id),
    CONSTRAINT uk_discovery_service_instance_uuid UNIQUE (uuid)
);

CREATE INDEX IF NOT EXISTS idx_discovery_service_instance_discover
    ON discovery_service_instance (namespace, environment, service_name, protocol, status, expires_at_ms);

CREATE INDEX IF NOT EXISTS idx_discovery_service_instance_expiry
    ON discovery_service_instance (expires_at_ms, namespace, environment, service_name, instance_id)
    WHERE deleted_at IS NULL;
"#;
