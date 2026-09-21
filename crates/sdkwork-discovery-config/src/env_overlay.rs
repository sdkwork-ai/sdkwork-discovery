use std::collections::BTreeMap;

use sdkwork_discovery_contract::{
    DiscoveryError, DiscoveryResult, RuntimeDeploymentProfile, RuntimeEnvironment, RuntimeTarget,
};

use crate::model::{
    DiscoveryRuntimeConfig, SecurityAuthMode, StorageCredentialSource, StorageFileConfig,
    StorageProvider, StorageTransportConfig,
};

pub(crate) fn validate_env_keys(env: &BTreeMap<String, String>) -> DiscoveryResult<()> {
    for key in env.keys() {
        if is_retired_database_env_key(key) {
            return Err(DiscoveryError::InvalidConfig(format!(
                "retired database env key {key}; use SDKWORK_DATABASE_* as the only database authority"
            )));
        }

        if is_forbidden_credential_env_key(key) {
            return Err(DiscoveryError::InvalidConfig(format!(
                "runtime credential env key is forbidden: {key}"
            )));
        }

        if key.starts_with("SDKWORK_DATABASE_") {
            if !is_supported_database_env_key(key) {
                return Err(DiscoveryError::InvalidConfig(format!(
                    "unsupported database env key: {key}"
                )));
            }
            continue;
        }

        if !key.starts_with("SDKWORK_DISCOVERY_") {
            return Err(DiscoveryError::InvalidConfig(format!(
                "unknown discovery env key: {key}"
            )));
        }
    }
    Ok(())
}

pub(crate) fn apply_env_overlay(
    config: &mut DiscoveryRuntimeConfig,
    env: &BTreeMap<String, String>,
) -> DiscoveryResult<()> {
    apply_runtime_identity_overlay(config, env)?;
    validate_storage_provider_database_engine(env)?;
    let effective_storage_provider = effective_storage_provider(config.storage.provider, env)?;

    for (key, value) in env {
        match key.as_str() {
            "SDKWORK_DISCOVERY_ENVIRONMENT" | "SDKWORK_DISCOVERY_CONFIG_PROFILE" => {}
            "SDKWORK_DISCOVERY_SERVICE_LAYOUT" | "SDKWORK_DISCOVERY_PROFILE_ID" => {}
            "SDKWORK_DISCOVERY_APPLICATION_PUBLIC_GRPC_URL"
            | "SDKWORK_DISCOVERY_OPERATIONS_CONTROL_GRPC_URL" => {}
            "SDKWORK_DISCOVERY_APPLICATION_PUBLIC_INGRESS_BIND" => {
                apply_public_ingress_bind(config, value)?;
            }
            "SDKWORK_DISCOVERY_OPERATIONS_CONTROL_INGRESS_BIND" => {
                apply_operations_control_bind(config, value)?;
            }
            "SDKWORK_DISCOVERY_DEPLOYMENT_PROFILE" => {
                config.runtime.deployment_profile = RuntimeDeploymentProfile::parse(value)
                    .ok_or_else(|| {
                        DiscoveryError::InvalidConfig(format!(
                            "unknown deployment profile: {value}"
                        ))
                    })?;
            }
            "SDKWORK_DISCOVERY_RUNTIME_TARGET" => {
                config.runtime.runtime_target = RuntimeTarget::parse(value).ok_or_else(|| {
                    DiscoveryError::InvalidConfig(format!("unknown runtime target: {value}"))
                })?;
            }
            "SDKWORK_DISCOVERY_STORAGE_PROVIDER" => {
                config.storage.provider = StorageProvider::parse(value).ok_or_else(|| {
                    DiscoveryError::InvalidConfig(format!("unknown storage provider: {value}"))
                })?;
            }
            key if key.starts_with("SDKWORK_DATABASE_") => {
                apply_database_overlay(config, effective_storage_provider, key, value)?;
            }
            key if key.starts_with("SDKWORK_DISCOVERY_STORAGE_REDIS_") => {
                apply_transport_storage_overlay(
                    config
                        .storage
                        .redis
                        .get_or_insert_with(default_storage_transport_config),
                    "redis",
                    false,
                    key,
                    value,
                    "SDKWORK_DISCOVERY_STORAGE_REDIS_",
                )?;
            }
            key if key.starts_with("SDKWORK_DISCOVERY_STORAGE_ETCD_") => {
                apply_transport_storage_overlay(
                    config
                        .storage
                        .etcd
                        .get_or_insert_with(default_storage_transport_config),
                    "etcd",
                    false,
                    key,
                    value,
                    "SDKWORK_DISCOVERY_STORAGE_ETCD_",
                )?;
            }
            key if key.starts_with("SDKWORK_DISCOVERY_STORAGE_CONSUL_") => {
                apply_transport_storage_overlay(
                    config
                        .storage
                        .consul
                        .get_or_insert_with(default_storage_transport_config),
                    "consul",
                    false,
                    key,
                    value,
                    "SDKWORK_DISCOVERY_STORAGE_CONSUL_",
                )?;
            }
            "SDKWORK_DISCOVERY_RPC_DEFAULT_DEADLINE_MS" => {
                config.server.default_deadline_ms = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_CONFIG_REGISTRY_ENABLED" => {
                config.config_registry.enabled = parse_bool_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_CONFIG_REGISTRY_MAX_CONFIG_BODY_BYTES" => {
                config.config_registry.max_config_body_bytes = parse_usize_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_CONFIG_REGISTRY_REQUIRE_PUBLISH_FOR_READS" => {
                config.config_registry.require_publish_for_reads = parse_bool_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_CONFIG_REGISTRY_ALLOW_SECRET_VALUES" => {
                config.config_registry.allow_secret_values = parse_bool_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_CONFIG_REGISTRY_ALLOW_SECRET_REFS" => {
                config.config_registry.allow_secret_refs = parse_bool_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_REGISTRY_DEFAULT_LEASE_TTL_SECONDS" => {
                config.registry.default_lease_ttl_seconds = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_REGISTRY_MIN_LEASE_TTL_SECONDS" => {
                config.registry.min_lease_ttl_seconds = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_REGISTRY_MAX_LEASE_TTL_SECONDS" => {
                config.registry.max_lease_ttl_seconds = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_REGISTRY_EXPIRY_SCAN_INTERVAL_MS" => {
                config.registry.expiry_scan_interval_ms = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_REGISTRY_EXPIRY_SCAN_BATCH_SIZE" => {
                config.registry.expiry_scan_batch_size = parse_usize_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_REGISTRY_HEALTH_CHECK_SCAN_INTERVAL_MS" => {
                config.registry.health_check_scan_interval_ms = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_WATCH_ENABLED" => {
                config.watch.enabled = parse_bool_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_WATCH_MAX_STREAMS" => {
                config.watch.max_streams = parse_u32_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_WATCH_EVENT_BUFFER_SIZE" => {
                config.watch.event_buffer_size = parse_usize_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_WATCH_HEARTBEAT_INTERVAL_MS" => {
                config.watch.heartbeat_interval_ms = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_WATCH_DURABLE_POLL_INTERVAL_MS" => {
                config.watch.durable_poll_interval_ms = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_WATCH_DURABLE_REPLAY_BATCH_SIZE" => {
                config.watch.durable_replay_batch_size = parse_usize_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_WATCH_EVENT_GC_INTERVAL_MS" => {
                config.watch.event_gc_interval_ms = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_WATCH_EVENT_GC_RETENTION_COUNT" => {
                config.watch.event_gc_retention_count = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_WATCH_EVENT_GC_BATCH_SIZE" => {
                config.watch.event_gc_batch_size = parse_usize_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RESILIENCE_CIRCUIT_BREAKER_ENABLED" => {
                config.resilience.circuit_breaker.enabled = parse_bool_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RESILIENCE_CIRCUIT_BREAKER_FAILURE_THRESHOLD" => {
                config.resilience.circuit_breaker.failure_threshold = parse_u32_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RESILIENCE_CIRCUIT_BREAKER_RECOVERY_TIMEOUT_MS" => {
                config.resilience.circuit_breaker.recovery_timeout_ms = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RESILIENCE_CIRCUIT_BREAKER_HALF_OPEN_MAX_REQUESTS" => {
                config.resilience.circuit_breaker.half_open_max_requests =
                    parse_u32_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RESILIENCE_RATE_LIMIT_ENABLED" => {
                config.resilience.rate_limit.enabled = parse_bool_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RESILIENCE_RATE_LIMIT_REQUESTS_PER_SECOND" => {
                config.resilience.rate_limit.requests_per_second = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RESILIENCE_RATE_LIMIT_BURST_CAPACITY" => {
                config.resilience.rate_limit.burst_capacity = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RESILIENCE_DEGRADATION_READ_ONLY_ON_STORAGE_FAILURE" => {
                config.resilience.degradation.read_only_on_storage_failure =
                    parse_bool_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RESILIENCE_DEGRADATION_STALE_READ_MAX_AGE_MS" => {
                config.resilience.degradation.stale_read_max_age_ms = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RPC_AUTH_MODE" => {
                config.security.auth_mode = SecurityAuthMode::parse(value).ok_or_else(|| {
                    DiscoveryError::InvalidConfig(format!(
                        "unsupported RPC auth mode for {key}: {value}; expected service-token"
                    ))
                })?;
            }
            "SDKWORK_DISCOVERY_RPC_ALLOW_UNSIGNED_LOCAL_CONTEXT" => {
                config.security.allow_unsigned_local_context = parse_bool_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_HMAC_SECRET_FILE" => {
                config.security.service_token.hmac_secret_file = Some(value.clone());
            }
            "SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_ISSUER" => {
                config.security.service_token.issuer = value.clone();
            }
            "SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_AUDIENCE" => {
                config.security.service_token.audience = value.clone();
            }
            "SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_MAX_TTL_SECONDS" => {
                config.security.service_token.max_token_ttl_seconds = parse_u64_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RPC_TLS_ENABLED" => {
                config.security.tls_enabled = parse_bool_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RPC_MTLS_ENABLED" => {
                config.security.mtls_enabled = parse_bool_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RPC_SERVER_TLS_CERT_FILE" => {
                config.security.server_tls_cert_file = Some(value.clone());
            }
            "SDKWORK_DISCOVERY_RPC_SERVER_TLS_KEY_FILE" => {
                config.security.server_tls_key_file = Some(value.clone());
            }
            "SDKWORK_DISCOVERY_RPC_CLIENT_CA_CERT_FILE" => {
                config.security.client_ca_cert_file = Some(value.clone());
            }
            "SDKWORK_DISCOVERY_RPC_REFLECTION_ENABLED" => {
                config.server.enable_reflection = parse_bool_env(key, value)?;
            }
            "SDKWORK_DISCOVERY_RPC_HEALTH_ENABLED" => {
                config.server.enable_health = parse_bool_env(key, value)?;
            }
            _ => {
                return Err(DiscoveryError::InvalidConfig(format!(
                    "unsupported discovery env key: {key}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_storage_provider_database_engine(
    env: &BTreeMap<String, String>,
) -> DiscoveryResult<()> {
    let has_database_overlay = env.keys().any(|key| is_supported_database_env_key(key));
    if has_database_overlay
        && !env.contains_key("SDKWORK_DATABASE_ENGINE")
        && !matches!(
            env.get("SDKWORK_DISCOVERY_STORAGE_PROVIDER")
                .map(String::as_str),
            Some("postgres" | "sqlite")
        )
    {
        return Err(DiscoveryError::InvalidConfig(
            "database env overlay requires SDKWORK_DATABASE_ENGINE or a matching storage provider"
                .to_string(),
        ));
    }

    let Some(storage_provider) = env.get("SDKWORK_DISCOVERY_STORAGE_PROVIDER") else {
        return Ok(());
    };
    let Some(database_engine) = env.get("SDKWORK_DATABASE_ENGINE") else {
        return Ok(());
    };

    let storage_provider = StorageProvider::parse(storage_provider).ok_or_else(|| {
        DiscoveryError::InvalidConfig(format!("unknown storage provider: {storage_provider}"))
    })?;
    let database_provider = parse_database_engine("SDKWORK_DATABASE_ENGINE", database_engine)?;

    if storage_provider != database_provider {
        return Err(DiscoveryError::InvalidConfig(format!(
            "storage provider and database engine conflict: storage={} database={}",
            storage_provider.as_str(),
            database_provider.as_str()
        )));
    }

    Ok(())
}

fn effective_storage_provider(
    current: StorageProvider,
    env: &BTreeMap<String, String>,
) -> DiscoveryResult<StorageProvider> {
    if let Some(database_engine) = env.get("SDKWORK_DATABASE_ENGINE") {
        return parse_database_engine("SDKWORK_DATABASE_ENGINE", database_engine);
    }

    if let Some(storage_provider) = env.get("SDKWORK_DISCOVERY_STORAGE_PROVIDER") {
        return StorageProvider::parse(storage_provider).ok_or_else(|| {
            DiscoveryError::InvalidConfig(format!("unknown storage provider: {storage_provider}"))
        });
    }

    Ok(current)
}

fn is_forbidden_credential_env_key(key: &str) -> bool {
    if matches!(
        key,
        "SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_HMAC_SECRET_FILE"
            | "SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_ISSUER"
            | "SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_AUDIENCE"
            | "SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_MAX_TTL_SECONDS"
    ) {
        return false;
    }

    if key.ends_with("_PASSWORD_FILE") {
        return false;
    }

    if matches!(
        key,
        "SDKWORK_DISCOVERY_CONFIG_REGISTRY_ALLOW_SECRET_VALUES"
            | "SDKWORK_DISCOVERY_CONFIG_REGISTRY_ALLOW_SECRET_REFS"
    ) {
        return false;
    }

    key.contains("TOKEN")
        || key.contains("SECRET")
        || key.contains("API_KEY")
        || key.contains("PASSWORD")
}

fn is_retired_database_env_key(key: &str) -> bool {
    key.starts_with("SDKWORK_DISCOVERY_STORAGE_POSTGRES_")
        || key.starts_with("SDKWORK_DISCOVERY_STORAGE_SQLITE_")
        || (key.starts_with("SDKWORK_")
            && !key.starts_with("SDKWORK_DATABASE_")
            && key.contains("_DATABASE_"))
}

fn is_supported_database_env_key(key: &str) -> bool {
    matches!(
        key,
        "SDKWORK_DATABASE_ENGINE"
            | "SDKWORK_DATABASE_HOST"
            | "SDKWORK_DATABASE_PORT"
            | "SDKWORK_DATABASE_NAME"
            | "SDKWORK_DATABASE_SCHEMA"
            | "SDKWORK_DATABASE_USERNAME"
            | "SDKWORK_DATABASE_PASSWORD_FILE"
            | "SDKWORK_DATABASE_PASSWORD"
            | "SDKWORK_DATABASE_SSL_MODE"
            | "SDKWORK_DATABASE_MAX_CONNECTIONS"
            | "SDKWORK_DATABASE_ACQUIRE_TIMEOUT"
            | "SDKWORK_DATABASE_FILE"
    )
}

fn apply_runtime_identity_overlay(
    config: &mut DiscoveryRuntimeConfig,
    env: &BTreeMap<String, String>,
) -> DiscoveryResult<()> {
    let explicit_environment = match env.get("SDKWORK_DISCOVERY_ENVIRONMENT") {
        Some(value) => Some(RuntimeEnvironment::parse(value).ok_or_else(|| {
            DiscoveryError::InvalidConfig(format!("unknown environment: {value}"))
        })?),
        None => None,
    };
    let profile = env
        .get("SDKWORK_DISCOVERY_CONFIG_PROFILE")
        .cloned()
        .or_else(|| config.runtime.config_profile.clone());
    let profile_environment = match profile.as_deref() {
        Some(value) => Some(RuntimeEnvironment::from_profile(value).ok_or_else(|| {
            DiscoveryError::InvalidConfig(format!("unknown config profile: {value}"))
        })?),
        None => None,
    };

    let effective_environment = explicit_environment
        .clone()
        .unwrap_or_else(|| config.runtime.environment.clone());
    if let Some(profile_environment) = profile_environment {
        if effective_environment != profile_environment {
            return Err(DiscoveryError::InvalidConfig(
                "environment and config profile describe different lifecycle stages".to_string(),
            ));
        }
    }

    config.runtime.environment = effective_environment;
    if let Some(profile) = env.get("SDKWORK_DISCOVERY_CONFIG_PROFILE") {
        config.runtime.config_profile = Some(profile.clone());
    }
    if let Some(profile) = env.get("SDKWORK_DISCOVERY_DEPLOYMENT_PROFILE") {
        config.runtime.deployment_profile =
            RuntimeDeploymentProfile::parse(profile).ok_or_else(|| {
                DiscoveryError::InvalidConfig(format!("unknown deployment profile: {profile}"))
            })?;
    }

    Ok(())
}

fn apply_transport_storage_overlay(
    transport: &mut StorageTransportConfig,
    provider: &str,
    supports_schema: bool,
    key: &str,
    value: &str,
    prefix: &str,
) -> DiscoveryResult<()> {
    let field = key.strip_prefix(prefix).ok_or_else(|| {
        DiscoveryError::InvalidConfig(format!("unsupported discovery env key: {key}"))
    })?;

    match field {
        "HOST" => {
            transport.host = value.to_string();
        }
        "PORT" => {
            transport.port = parse_u16_env(key, value)?;
        }
        "DATABASE" => {
            transport.database = optional_string(value);
        }
        "SCHEMA" => {
            if !supports_schema {
                return Err(DiscoveryError::InvalidConfig(format!(
                    "storage provider {provider} does not support schema; schema is postgres-only"
                )));
            }
            transport.schema = optional_string(value);
        }
        "USERNAME" => {
            transport.username = optional_string(value);
        }
        "PASSWORD_FILE" => {
            transport.credential_source = StorageCredentialSource::PasswordFile(value.to_string());
        }
        "PASSWORD" => {
            return Err(DiscoveryError::InvalidConfig(format!(
                "storage provider {provider} must use password_file instead of direct password values"
            )));
        }
        "TLS_ENABLED" => {
            transport.tls_enabled = parse_bool_env(key, value)?;
        }
        "CONNECT_TIMEOUT_MS" => {
            transport.connect_timeout_ms = parse_u64_env(key, value)?;
        }
        "MAX_CONNECTIONS" => {
            transport.max_connections = parse_u32_env(key, value)?;
        }
        _ => {
            return Err(DiscoveryError::InvalidConfig(format!(
                "unsupported discovery env key: {key}"
            )));
        }
    }

    Ok(())
}

fn apply_database_overlay(
    config: &mut DiscoveryRuntimeConfig,
    effective_provider: StorageProvider,
    key: &str,
    value: &str,
) -> DiscoveryResult<()> {
    let field = key.strip_prefix("SDKWORK_DATABASE_").ok_or_else(|| {
        DiscoveryError::InvalidConfig(format!("unsupported database env key: {key}"))
    })?;
    validate_database_field_for_provider(effective_provider, field, key)?;

    match field {
        "ENGINE" => {
            config.storage.provider = parse_database_engine(key, value)?;
        }
        "HOST" => {
            config
                .storage
                .postgres
                .get_or_insert_with(default_storage_transport_config)
                .host = value.to_string();
        }
        "PORT" => {
            config
                .storage
                .postgres
                .get_or_insert_with(default_storage_transport_config)
                .port = parse_u16_env(key, value)?;
        }
        "NAME" => {
            config
                .storage
                .postgres
                .get_or_insert_with(default_storage_transport_config)
                .database = optional_string(value);
        }
        "SCHEMA" => {
            config
                .storage
                .postgres
                .get_or_insert_with(default_storage_transport_config)
                .schema = optional_string(value);
        }
        "USERNAME" => {
            config
                .storage
                .postgres
                .get_or_insert_with(default_storage_transport_config)
                .username = optional_string(value);
        }
        "PASSWORD_FILE" => {
            config
                .storage
                .postgres
                .get_or_insert_with(default_storage_transport_config)
                .credential_source = StorageCredentialSource::PasswordFile(value.to_string());
        }
        "PASSWORD" => {
            return Err(DiscoveryError::InvalidConfig(
                "database storage must use password_file instead of direct password values"
                    .to_string(),
            ));
        }
        "SSL_MODE" => {
            config
                .storage
                .postgres
                .get_or_insert_with(default_storage_transport_config)
                .tls_enabled = parse_database_ssl_mode(key, value)?;
        }
        "MAX_CONNECTIONS" => match config.storage.provider {
            _ if effective_provider == StorageProvider::Sqlite => {
                config
                    .storage
                    .sqlite
                    .get_or_insert_with(default_storage_file_config)
                    .max_connections = parse_u32_env(key, value)?;
            }
            _ => {
                config
                    .storage
                    .postgres
                    .get_or_insert_with(default_storage_transport_config)
                    .max_connections = parse_u32_env(key, value)?;
            }
        },
        "ACQUIRE_TIMEOUT" => {
            config
                .storage
                .postgres
                .get_or_insert_with(default_storage_transport_config)
                .connect_timeout_ms = parse_timeout_seconds_as_ms(key, value)?;
        }
        "FILE" => {
            config
                .storage
                .sqlite
                .get_or_insert_with(default_storage_file_config)
                .file = value.to_string();
        }
        _ => {
            return Err(DiscoveryError::InvalidConfig(format!(
                "unsupported discovery env key: {key}"
            )));
        }
    }

    Ok(())
}

fn validate_database_field_for_provider(
    provider: StorageProvider,
    field: &str,
    key: &str,
) -> DiscoveryResult<()> {
    match provider {
        StorageProvider::Postgres => match field {
            "ENGINE" | "HOST" | "PORT" | "NAME" | "SCHEMA" | "USERNAME" | "PASSWORD_FILE"
            | "PASSWORD" | "SSL_MODE" | "MAX_CONNECTIONS" | "ACQUIRE_TIMEOUT" => Ok(()),
            "FILE" => Err(DiscoveryError::InvalidConfig(format!(
                "database env key {key} is a sqlite field but the effective database provider is postgres"
            ))),
            _ => Err(DiscoveryError::InvalidConfig(format!(
                "unsupported discovery env key: {key}"
            ))),
        },
        StorageProvider::Sqlite => match field {
            "ENGINE" | "FILE" | "MAX_CONNECTIONS" => Ok(()),
            "HOST" | "PORT" | "NAME" | "SCHEMA" | "USERNAME" | "PASSWORD_FILE" | "PASSWORD"
            | "SSL_MODE" | "ACQUIRE_TIMEOUT" => Err(DiscoveryError::InvalidConfig(format!(
                "database env key {key} is a postgres field but the effective database provider is sqlite"
            ))),
            _ => Err(DiscoveryError::InvalidConfig(format!(
                "unsupported discovery env key: {key}"
            ))),
        },
        _ => Err(DiscoveryError::InvalidConfig(format!(
            "database env key {key} requires postgres or sqlite storage"
        ))),
    }
}

fn parse_database_engine(key: &str, value: &str) -> DiscoveryResult<StorageProvider> {
    match value {
        "postgres" | "postgresql" => Ok(StorageProvider::Postgres),
        "sqlite" => Ok(StorageProvider::Sqlite),
        _ => Err(DiscoveryError::InvalidConfig(format!(
            "invalid database engine for {key}: {value}"
        ))),
    }
}

fn parse_database_ssl_mode(key: &str, value: &str) -> DiscoveryResult<bool> {
    match value {
        "disable" => Ok(false),
        "require" | "verify-ca" | "verify-full" => Ok(true),
        _ => Err(DiscoveryError::InvalidConfig(format!(
            "invalid database ssl mode for {key}: {value}"
        ))),
    }
}

fn optional_string(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn default_storage_transport_config() -> StorageTransportConfig {
    StorageTransportConfig {
        host: String::new(),
        port: 0,
        database: None,
        schema: None,
        username: None,
        credential_source: StorageCredentialSource::None,
        tls_enabled: false,
        connect_timeout_ms: 0,
        max_connections: 0,
    }
}

fn default_storage_file_config() -> StorageFileConfig {
    StorageFileConfig {
        file: String::new(),
        max_connections: 0,
    }
}

fn parse_u16_env(key: &str, value: &str) -> DiscoveryResult<u16> {
    value.parse::<u16>().map_err(|error| {
        DiscoveryError::InvalidConfig(format!("invalid integer for {key}: {error}"))
    })
}

fn parse_u64_env(key: &str, value: &str) -> DiscoveryResult<u64> {
    value.parse::<u64>().map_err(|error| {
        DiscoveryError::InvalidConfig(format!("invalid integer for {key}: {error}"))
    })
}

fn parse_timeout_seconds_as_ms(key: &str, value: &str) -> DiscoveryResult<u64> {
    parse_u64_env(key, value)?
        .checked_mul(1_000)
        .ok_or_else(|| DiscoveryError::InvalidConfig(format!("timeout for {key} is too large")))
}

fn parse_u32_env(key: &str, value: &str) -> DiscoveryResult<u32> {
    value.parse::<u32>().map_err(|error| {
        DiscoveryError::InvalidConfig(format!("invalid integer for {key}: {error}"))
    })
}

fn parse_usize_env(key: &str, value: &str) -> DiscoveryResult<usize> {
    value.parse::<usize>().map_err(|error| {
        DiscoveryError::InvalidConfig(format!("invalid integer for {key}: {error}"))
    })
}

fn apply_public_ingress_bind(
    config: &mut DiscoveryRuntimeConfig,
    bind: &str,
) -> DiscoveryResult<()> {
    let (host, port) =
        parse_host_port_bind("SDKWORK_DISCOVERY_APPLICATION_PUBLIC_INGRESS_BIND", bind)?;
    config.server.grpc_bind_host = host;
    config.server.grpc_port = port;
    Ok(())
}

fn apply_operations_control_bind(
    config: &mut DiscoveryRuntimeConfig,
    bind: &str,
) -> DiscoveryResult<()> {
    let (host, port) =
        parse_host_port_bind("SDKWORK_DISCOVERY_OPERATIONS_CONTROL_INGRESS_BIND", bind)?;
    if config.server.grpc_bind_host.is_empty() {
        config.server.grpc_bind_host = host;
    }
    config.server.admin_grpc_port = port;
    Ok(())
}

fn parse_host_port_bind(key: &str, bind: &str) -> DiscoveryResult<(String, u16)> {
    let normalized = bind.trim();
    if normalized.is_empty() {
        return Err(DiscoveryError::InvalidConfig(format!(
            "invalid bind address for {key}: empty value"
        )));
    }

    // The split is done once, by the shared parser, so this listener cannot
    // drift from the others in the workspace. Only the diagnostics are local;
    // an empty host or a missing port is reported as the same `host:port`
    // contract violation the operator sees today.
    let invalid = || {
        DiscoveryError::InvalidConfig(format!(
            "invalid bind address for {key}: expected host:port"
        ))
    };
    let port = sdkwork_utils_rust::service_base_url::bind_port(normalized).ok_or_else(invalid)?;
    let host = normalized
        .rsplit_once(':')
        .map(|(host, _)| host)
        .filter(|host| !host.is_empty())
        .ok_or_else(invalid)?;

    Ok((host.to_owned(), port))
}

fn parse_bool_env(key: &str, value: &str) -> DiscoveryResult<bool> {
    match value {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(DiscoveryError::InvalidConfig(format!(
            "invalid boolean for {key}: {value}"
        ))),
    }
}
