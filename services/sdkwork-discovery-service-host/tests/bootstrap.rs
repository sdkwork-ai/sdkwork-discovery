use sdkwork_discovery_service_host::{DiscoveryServiceHostBootstrap, DiscoveryServiceHostRuntime};
use sdkwork_discovery_rpc_proto::sdkwork::discovery::backend::v3::discovery_admin_service_client::DiscoveryAdminServiceClient;
use sdkwork_discovery_rpc_proto::sdkwork::discovery::backend::v3::{
    CreateConfigDraftRequest, PublishConfigRequest,
};
use sdkwork_discovery_rpc_proto::sdkwork::discovery::common::v1 as common_proto;
use sdkwork_discovery_rpc_proto::sdkwork::discovery::common::v1::InstanceStatus as ProtoInstanceStatus;
use sdkwork_discovery_rpc_proto::sdkwork::discovery::common::v1::{
    ConfigFormat as ProtoConfigFormat, ConfigScopeType as ProtoConfigScopeType,
};
use sdkwork_discovery_rpc_proto::sdkwork::discovery::internal::v1::discovery_config_service_client::DiscoveryConfigServiceClient;
use sdkwork_discovery_rpc_proto::sdkwork::discovery::internal::v1::discovery_watch_service_client::DiscoveryWatchServiceClient;
use sdkwork_discovery_rpc_proto::sdkwork::discovery::internal::v1::registry_service_client::RegistryServiceClient;
use sdkwork_discovery_rpc_proto::sdkwork::discovery::internal::v1::{
    RegisterInstanceRequest, RetrieveEffectiveConfigRequest, WatchConfigRequest,
    WatchServiceRequest,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::time::{timeout, Duration};
use tonic::transport::Endpoint;
use tonic::Request;

static TEST_STORAGE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn parse_env_example(input: &str) -> BTreeMap<String, String> {
    input
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }

            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            if !key.starts_with("SDKWORK_DISCOVERY_")
                && (!key.starts_with("SDKWORK_DATABASE_")
                    || key.starts_with("SDKWORK_DATABASE_ADMIN_"))
            {
                return None;
            }
            Some((key.to_string(), value.trim().to_string()))
        })
        .collect()
}

fn surface_bind_env(public_port: u16, operations_port: u16) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "SDKWORK_DISCOVERY_APPLICATION_PUBLIC_INGRESS_BIND".to_string(),
            format!("127.0.0.1:{public_port}"),
        ),
        (
            "SDKWORK_DISCOVERY_OPERATIONS_CONTROL_INGRESS_BIND".to_string(),
            format!("127.0.0.1:{operations_port}"),
        ),
    ])
}

fn public_surface_bind_env(public_port: u16) -> BTreeMap<String, String> {
    BTreeMap::from([(
        "SDKWORK_DISCOVERY_APPLICATION_PUBLIC_INGRESS_BIND".to_string(),
        format!("127.0.0.1:{public_port}"),
    )])
}

#[test]
fn service_host_bootstrap_builds_control_plane_from_runtime_config() {
    let mut env = BTreeMap::new();
    env.insert(
        "SDKWORK_DISCOVERY_STORAGE_PROVIDER".to_string(),
        "memory".to_string(),
    );

    let bootstrap = DiscoveryServiceHostBootstrap::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    assert_eq!(bootstrap.storage_provider_name(), "memory");
    assert_eq!(bootstrap.config().server.grpc_port, 19090);
}

#[test]
fn service_host_bootstrap_loads_resilience_config_from_env_overlay() {
    let env = BTreeMap::from([
        (
            "SDKWORK_DISCOVERY_STORAGE_PROVIDER".to_string(),
            "memory".to_string(),
        ),
        (
            "SDKWORK_DISCOVERY_RESILIENCE_RATE_LIMIT_ENABLED".to_string(),
            "true".to_string(),
        ),
        (
            "SDKWORK_DISCOVERY_RESILIENCE_RATE_LIMIT_REQUESTS_PER_SECOND".to_string(),
            "25".to_string(),
        ),
        (
            "SDKWORK_DISCOVERY_RESILIENCE_RATE_LIMIT_BURST_CAPACITY".to_string(),
            "50".to_string(),
        ),
        (
            "SDKWORK_DISCOVERY_WATCH_EVENT_GC_INTERVAL_MS".to_string(),
            "45000".to_string(),
        ),
    ]);

    let bootstrap = DiscoveryServiceHostBootstrap::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    assert!(bootstrap.config().resilience.rate_limit.enabled);
    assert_eq!(
        bootstrap.config().resilience.rate_limit.requests_per_second,
        25
    );
    assert_eq!(bootstrap.config().resilience.rate_limit.burst_capacity, 50);
    assert_eq!(bootstrap.config().watch.event_gc_interval_ms, 45_000);
}

#[test]
fn service_host_bootstrap_accepts_checked_in_postgres_env_example() {
    let env = parse_env_example(include_str!("../../../config/host/postgres.env.example"));

    let bootstrap = DiscoveryServiceHostBootstrap::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let postgres = bootstrap.config().storage.postgres.as_ref().unwrap();
    let summary = bootstrap.storage_safe_summary();

    assert_eq!(bootstrap.storage_provider_name(), "postgres");
    assert_eq!(postgres.host, "127.0.0.1");
    assert_eq!(postgres.database.as_deref(), Some("sdkwork_ai_dev"));
    assert_eq!(postgres.schema.as_deref(), Some("sdkwork_ai_dev"));
    assert_eq!(postgres.connect_timeout_ms, 3000);
    assert_eq!(postgres.max_connections, 16);
    assert!(summary.contains("schema=sdkwork_ai_dev"));
    assert!(!summary.to_ascii_lowercase().contains("password"));
    assert!(!summary.to_ascii_lowercase().contains("secret"));
}

#[test]
fn service_host_bootstrap_maps_default_deadline_into_rpc_server_config() {
    let env = BTreeMap::from([(
        "SDKWORK_DISCOVERY_RPC_DEFAULT_DEADLINE_MS".to_string(),
        "2750".to_string(),
    )]);

    let bootstrap = DiscoveryServiceHostBootstrap::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let rpc_config = bootstrap.internal_rpc_server_config().unwrap();

    assert_eq!(rpc_config.default_deadline_ms, 2750);
}

#[test]
fn service_host_bootstrap_exposes_backend_rpc_server_config_for_preflight() {
    let mut env = surface_bind_env(19190, 19191);
    env.insert(
        "SDKWORK_DISCOVERY_RPC_DEFAULT_DEADLINE_MS".to_string(),
        "2750".to_string(),
    );

    let bootstrap = DiscoveryServiceHostBootstrap::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let internal_config = bootstrap.internal_rpc_server_config().unwrap();
    let backend_config = bootstrap.backend_rpc_server_config().unwrap();

    assert_eq!(internal_config.bind_addr, "127.0.0.1:19190");
    assert_eq!(backend_config.bind_addr, "127.0.0.1:19191");
    assert_eq!(backend_config.default_deadline_ms, 2750);
    assert_eq!(
        backend_config.enable_reflection,
        bootstrap.config().server.enable_reflection
    );
}

#[test]
fn service_host_runtime_exposes_rpc_server_configs_for_preflight() {
    let env = surface_bind_env(19192, 19193);
    let runtime = DiscoveryServiceHostRuntime::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let internal_config = runtime.internal_rpc_server_config().unwrap();
    let backend_config = runtime.backend_rpc_server_config().unwrap();

    assert_eq!(internal_config.bind_addr, "127.0.0.1:19192");
    assert_eq!(backend_config.bind_addr, "127.0.0.1:19193");
}

#[test]
fn service_host_bootstrap_maps_watch_runtime_governance_into_rpc_server_config() {
    let env = BTreeMap::from([
        (
            "SDKWORK_DISCOVERY_WATCH_ENABLED".to_string(),
            "true".to_string(),
        ),
        (
            "SDKWORK_DISCOVERY_WATCH_MAX_STREAMS".to_string(),
            "64".to_string(),
        ),
        (
            "SDKWORK_DISCOVERY_WATCH_EVENT_BUFFER_SIZE".to_string(),
            "128".to_string(),
        ),
        (
            "SDKWORK_DISCOVERY_WATCH_HEARTBEAT_INTERVAL_MS".to_string(),
            "250".to_string(),
        ),
        (
            "SDKWORK_DISCOVERY_WATCH_DURABLE_POLL_INTERVAL_MS".to_string(),
            "750".to_string(),
        ),
        (
            "SDKWORK_DISCOVERY_WATCH_DURABLE_REPLAY_BATCH_SIZE".to_string(),
            "256".to_string(),
        ),
    ]);

    let bootstrap = DiscoveryServiceHostBootstrap::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let rpc_config = bootstrap.internal_rpc_server_config().unwrap();

    assert!(rpc_config.watch_enabled);
    assert_eq!(rpc_config.watch_max_streams, 64);
    assert_eq!(rpc_config.watch_event_buffer_size, 128);
    assert_eq!(rpc_config.watch_heartbeat_interval_ms, 250);
    assert_eq!(rpc_config.watch_durable_poll_interval_ms, 750);
    assert_eq!(rpc_config.watch_durable_replay_batch_size, 256);
}

#[test]
fn service_host_bootstrap_accepts_postgres_storage_provider_without_exposing_secrets() {
    let input = include_str!("../../../etc/discovery.example.toml").replace(
        r#"[storage]
provider = "memory""#,
        r#"[storage]
provider = "postgres"

[storage.postgres]
host = "127.0.0.1"
port = 5432
database = "sdkwork_ai_dev"
schema = "sdkwork_ai_dev"
username = "sdkwork_ai_dev"
password_file = "/run/secrets/sdkwork/discovery/postgres-password"
tls_enabled = false
connect_timeout_ms = 3000
max_connections = 16"#,
    );

    let bootstrap =
        DiscoveryServiceHostBootstrap::from_toml_str_with_env(&input, &BTreeMap::new()).unwrap();
    let summary = bootstrap.storage_safe_summary();

    assert_eq!(bootstrap.storage_provider_name(), "postgres");
    assert!(summary.contains("postgres host=127.0.0.1"));
    assert!(summary.contains("database=sdkwork_ai_dev"));
    assert!(!summary.to_ascii_lowercase().contains("password"));
    assert!(!summary.to_ascii_lowercase().contains("secret"));
}

#[test]
fn service_host_bootstrap_accepts_sqlite_storage_provider_with_safe_summary() {
    let input = include_str!("../../../etc/discovery.example.toml").replace(
        r#"[storage]
provider = "memory""#,
        r#"[storage]
provider = "sqlite"
apply_initial_schema = true

[storage.sqlite]
file = "target/dev/discovery/discovery.sqlite"
max_connections = 1"#,
    );

    let bootstrap =
        DiscoveryServiceHostBootstrap::from_toml_str_with_env(&input, &BTreeMap::new()).unwrap();
    let summary = bootstrap.storage_safe_summary();

    assert_eq!(bootstrap.storage_provider_name(), "sqlite");
    assert!(summary.contains("sqlite file=target/dev/discovery/discovery.sqlite"));
    assert!(!summary.to_ascii_lowercase().contains("password"));
    assert!(!summary.to_ascii_lowercase().contains("secret"));
}

#[tokio::test]
async fn sqlite_storage_initialization_applies_schema_when_explicitly_enabled() {
    let input = include_str!("../../../etc/discovery.example.toml").replace(
        r#"[storage]
provider = "memory""#,
        r#"[storage]
provider = "sqlite"
apply_initial_schema = true

[storage.sqlite]
file = ":memory:"
max_connections = 1"#,
    );
    let bootstrap =
        DiscoveryServiceHostBootstrap::from_toml_str_with_env(&input, &BTreeMap::new()).unwrap();

    bootstrap.initialize_storage().await.unwrap();
}

#[test]
fn service_host_bootstrap_keeps_initial_schema_application_explicit() {
    let input = include_str!("../../../etc/discovery.example.toml").replace(
        r#"[storage]
provider = "memory""#,
        r#"[storage]
provider = "postgres"
apply_initial_schema = true

[storage.postgres]
host = "127.0.0.1"
port = 5432
database = "sdkwork_ai_dev"
schema = "sdkwork_ai_dev"
username = "sdkwork_ai_dev"
password_file = "/run/secrets/sdkwork/discovery/postgres-password"
tls_enabled = false
connect_timeout_ms = 3000
max_connections = 16"#,
    );

    let bootstrap =
        DiscoveryServiceHostBootstrap::from_toml_str_with_env(&input, &BTreeMap::new()).unwrap();

    assert!(bootstrap.config().storage.apply_initial_schema);
}

#[test]
fn etcd_storage_initialization_fails_fast_until_adapter_is_implemented() {
    let input = include_str!("../../../etc/discovery.example.toml").replace(
        r#"[storage]
provider = "memory""#,
        r#"[storage]
provider = "etcd"

[storage.etcd]
host = "127.0.0.1"
port = 2379
tls_enabled = false
connect_timeout_ms = 3000
max_connections = 16"#,
    );
    match DiscoveryServiceHostBootstrap::from_toml_str_with_env(&input, &BTreeMap::new()) {
        Ok(_) => panic!("etcd storage provider must fail fast before bootstrap"),
        Err(error) => {
            assert!(error.to_string().contains("not implemented"));
            assert!(error.to_string().contains("etcd"));
        }
    }
}

#[test]
fn consul_storage_initialization_fails_fast_until_adapter_is_implemented() {
    let input = include_str!("../../../etc/discovery.example.toml").replace(
        r#"[storage]
provider = "memory""#,
        r#"[storage]
provider = "consul"

[storage.consul]
host = "127.0.0.1"
port = 8500
tls_enabled = false
connect_timeout_ms = 3000
max_connections = 16"#,
    );
    match DiscoveryServiceHostBootstrap::from_toml_str_with_env(&input, &BTreeMap::new()) {
        Ok(_) => panic!("consul storage provider must fail fast before bootstrap"),
        Err(error) => {
            assert!(error.to_string().contains("not implemented"));
            assert!(error.to_string().contains("consul"));
        }
    }
}

#[tokio::test]
async fn memory_storage_initialization_is_noop() {
    let bootstrap = DiscoveryServiceHostBootstrap::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &BTreeMap::new(),
    )
    .unwrap();

    bootstrap.initialize_storage().await.unwrap();
}

#[tokio::test]
async fn memory_runtime_can_start_and_stop_grpc_server_on_ephemeral_port() {
    let env = public_surface_bind_env(0);
    let runtime = DiscoveryServiceHostRuntime::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let server = runtime.serve_grpc().await.unwrap();

    server.shutdown().await;
}

#[tokio::test]
async fn memory_runtime_accepts_verified_service_token_when_unsigned_context_is_disabled_over_grpc()
{
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let secret_file =
        unique_sqlite_file("service-token-secret").with_file_name("service-token-hmac.secret");
    std::fs::write(&secret_file, SERVICE_TOKEN_SECRET).unwrap();

    let mut env = surface_bind_env(port, port);
    env.insert(
        "SDKWORK_DISCOVERY_RPC_ALLOW_UNSIGNED_LOCAL_CONTEXT".to_string(),
        "false".to_string(),
    );
    env.insert(
        "SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_HMAC_SECRET_FILE".to_string(),
        secret_file.to_string_lossy().into_owned(),
    );
    env.insert(
        "SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_ISSUER".to_string(),
        "sdkwork-discovery".to_string(),
    );
    env.insert(
        "SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_AUDIENCE".to_string(),
        "sdkwork-discovery-rpc".to_string(),
    );
    env.insert(
        "SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_MAX_TTL_SECONDS".to_string(),
        (200_u64 * 365 * 24 * 60 * 60).to_string(),
    );
    let runtime = DiscoveryServiceHostRuntime::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let server = runtime.serve_grpc().await.unwrap();
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = RegistryServiceClient::new(channel);
    let mut request = Request::new(register_request());
    add_verified_service_token_metadata(&mut request);

    let response = client
        .register_instance(request)
        .await
        .unwrap()
        .into_inner();
    server.shutdown().await;

    assert_eq!(response.lease_id, "lease-1");
}

#[tokio::test]
async fn memory_runtime_starts_separate_internal_and_admin_servers_when_ports_differ() {
    let internal_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let internal_port = internal_probe.local_addr().unwrap().port();
    let admin_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let admin_port = admin_probe.local_addr().unwrap().port();
    drop(internal_probe);
    drop(admin_probe);

    let env = surface_bind_env(internal_port, admin_port);
    let runtime = DiscoveryServiceHostRuntime::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let server = runtime.serve_grpc().await.unwrap();

    assert_eq!(server.bound_server_count(), 2);
    server.shutdown().await;
}

#[tokio::test]
async fn memory_runtime_enforces_configured_registry_lease_ttl_bounds_over_grpc() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let mut env = surface_bind_env(port, port);
    env.insert(
        "SDKWORK_DISCOVERY_REGISTRY_MIN_LEASE_TTL_SECONDS".to_string(),
        "10".to_string(),
    );
    env.insert(
        "SDKWORK_DISCOVERY_REGISTRY_DEFAULT_LEASE_TTL_SECONDS".to_string(),
        "30".to_string(),
    );
    env.insert(
        "SDKWORK_DISCOVERY_REGISTRY_MAX_LEASE_TTL_SECONDS".to_string(),
        "300".to_string(),
    );
    let runtime = DiscoveryServiceHostRuntime::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let server = runtime.serve_grpc().await.unwrap();
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = RegistryServiceClient::new(channel);
    let mut request = Request::new(RegisterInstanceRequest {
        lease_ttl_seconds: 9,
        ..register_request()
    });
    add_registry_write_metadata(&mut request);

    let status = client.register_instance(request).await.unwrap_err();
    server.shutdown().await;

    assert_eq!(status.code(), tonic::Code::PermissionDenied);
    assert!(status.message().contains("lease ttl"));
}

#[tokio::test]
async fn memory_runtime_uses_configured_expiry_scan_interval_over_grpc_watch() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let mut env = surface_bind_env(port, port);
    env.insert(
        "SDKWORK_DISCOVERY_REGISTRY_MIN_LEASE_TTL_SECONDS".to_string(),
        "1".to_string(),
    );
    env.insert(
        "SDKWORK_DISCOVERY_REGISTRY_EXPIRY_SCAN_INTERVAL_MS".to_string(),
        "20".to_string(),
    );
    let runtime = DiscoveryServiceHostRuntime::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let server = runtime.serve_grpc().await.unwrap();
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut registry = RegistryServiceClient::new(channel.clone());
    let mut register = Request::new(RegisterInstanceRequest {
        lease_ttl_seconds: 1,
        ..register_request()
    });
    add_registry_write_metadata(&mut register);
    registry.register_instance(register).await.unwrap();

    let mut watch = DiscoveryWatchServiceClient::new(channel);
    let mut request = Request::new(WatchServiceRequest {
        namespace: "sdkwork".to_string(),
        environment: "development".to_string(),
        service_name: "sdkwork-drive-product".to_string(),
        from_revision: 1,
    });
    add_registry_read_metadata(&mut request);
    let mut stream = watch.watch_service(request).await.unwrap().into_inner();

    let event = timeout(Duration::from_secs(3), stream.message())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    server.shutdown().await;

    assert_eq!(
        event.event_type,
        common_proto::WatchEventType::InstanceDeregistered as i32
    );
    assert_eq!(event.metadata.as_ref().unwrap().revision, 2);
    let instance = event
        .instance
        .expect("expiry watch event must include service identity tombstone");
    assert_eq!(instance.instance_id, "drive-1");
    assert_eq!(instance.status, ProtoInstanceStatus::NotServing as i32);
}

#[tokio::test]
async fn memory_runtime_applies_config_registry_read_policy_over_grpc() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let mut env = surface_bind_env(port, port);
    env.insert(
        "SDKWORK_DISCOVERY_CONFIG_REGISTRY_REQUIRE_PUBLISH_FOR_READS".to_string(),
        "false".to_string(),
    );
    let runtime = DiscoveryServiceHostRuntime::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let server = runtime.serve_grpc().await.unwrap();
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = DiscoveryConfigServiceClient::new(channel);
    let mut request = Request::new(RetrieveEffectiveConfigRequest {
        namespace: "sdkwork".to_string(),
        environment: "development".to_string(),
        application: "sdkwork-drive".to_string(),
        service_name: "sdkwork-drive-product".to_string(),
        group: "runtime".to_string(),
    });
    add_config_read_metadata(&mut request);

    let response = client
        .retrieve_effective_config(request)
        .await
        .unwrap()
        .into_inner();
    server.shutdown().await;

    assert!(response.values.is_empty());
    assert_eq!(response.metadata.unwrap().revision, 0);
}

#[tokio::test]
async fn memory_runtime_rejects_config_reads_when_config_registry_disabled_over_grpc() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let mut env = surface_bind_env(port, port);
    env.insert(
        "SDKWORK_DISCOVERY_CONFIG_REGISTRY_ENABLED".to_string(),
        "false".to_string(),
    );
    let runtime = DiscoveryServiceHostRuntime::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let server = runtime.serve_grpc().await.unwrap();
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = DiscoveryConfigServiceClient::new(channel);
    let mut request = Request::new(RetrieveEffectiveConfigRequest {
        namespace: "sdkwork".to_string(),
        environment: "development".to_string(),
        application: "sdkwork-drive".to_string(),
        service_name: "sdkwork-drive-product".to_string(),
        group: "runtime".to_string(),
    });
    add_config_read_metadata(&mut request);

    let status = client.retrieve_effective_config(request).await.unwrap_err();
    server.shutdown().await;

    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert!(status.message().contains("config registry"));
    assert!(status.message().contains("disabled"));
}

#[tokio::test]
async fn sqlite_backed_config_watch_streams_updates_published_by_another_runtime() {
    let watch_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let watch_port = watch_probe.local_addr().unwrap().port();
    let publish_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let publish_port = publish_probe.local_addr().unwrap().port();
    drop(watch_probe);
    drop(publish_probe);

    let database_file = unique_sqlite_file("cross-runtime-config-watch");
    let watch_env = sqlite_runtime_env(&database_file, watch_port);
    let publish_env = sqlite_runtime_env(&database_file, publish_port);
    let config = sqlite_runtime_config_toml();
    let watch_runtime =
        DiscoveryServiceHostRuntime::from_toml_str_with_env(&config, &watch_env).unwrap();
    watch_runtime
        .bootstrap()
        .initialize_storage()
        .await
        .unwrap();
    let publish_runtime =
        DiscoveryServiceHostRuntime::from_toml_str_with_env(&config, &publish_env).unwrap();

    let watch_server = watch_runtime.serve_grpc().await.unwrap();
    let publish_server = publish_runtime.serve_grpc().await.unwrap();
    let watch_channel = Endpoint::from_shared(format!("http://127.0.0.1:{watch_port}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let publish_channel = Endpoint::from_shared(format!("http://127.0.0.1:{publish_port}"))
        .unwrap()
        .connect()
        .await
        .unwrap();

    let mut watch = DiscoveryConfigServiceClient::new(watch_channel);
    let mut watch_request = Request::new(WatchConfigRequest {
        namespace: "sdkwork".to_string(),
        environment: "development".to_string(),
        application: "sdkwork-drive".to_string(),
        service_name: "sdkwork-drive-product".to_string(),
        group: "runtime".to_string(),
        from_revision: 0,
    });
    add_config_read_metadata(&mut watch_request);
    let mut stream = watch
        .watch_config(watch_request)
        .await
        .unwrap()
        .into_inner();

    let mut admin = DiscoveryAdminServiceClient::new(publish_channel);
    let mut create = Request::new(CreateConfigDraftRequest {
        namespace: "sdkwork".to_string(),
        environment: "development".to_string(),
        group: "runtime".to_string(),
        key: "log.level".to_string(),
        format: ProtoConfigFormat::Text as i32,
        value: "debug".to_string(),
        scope_type: ProtoConfigScopeType::Service as i32,
        application: "sdkwork-drive".to_string(),
        service_name: "sdkwork-drive-product".to_string(),
    });
    add_config_write_metadata(
        &mut create,
        "cross-runtime-draft-1",
        "sha256:cross-runtime-draft-1",
    );
    let draft = admin
        .create_config_draft(create)
        .await
        .unwrap()
        .into_inner();

    let mut publish = Request::new(PublishConfigRequest {
        draft_id: draft.draft_id,
    });
    add_config_write_metadata(
        &mut publish,
        "cross-runtime-publish-1",
        "sha256:cross-runtime-publish-1",
    );
    admin.publish_config(publish).await.unwrap();

    let event = timeout(Duration::from_secs(3), async {
        loop {
            let event = stream.message().await.unwrap().unwrap();
            if event.event_type == common_proto::WatchEventType::Heartbeat as i32 {
                continue;
            }
            return event;
        }
    })
    .await
    .unwrap();
    watch_server.shutdown().await;
    publish_server.shutdown().await;

    assert_eq!(
        event.event_type,
        common_proto::WatchEventType::ConfigPublished as i32
    );
    assert_eq!(event.group, "runtime");
    assert_eq!(event.key, "log.level");
    assert_eq!(event.metadata.unwrap().revision, 1);
}

#[tokio::test]
async fn sqlite_backed_service_watch_streams_instances_registered_by_another_runtime() {
    let watch_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let watch_port = watch_probe.local_addr().unwrap().port();
    let register_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let register_port = register_probe.local_addr().unwrap().port();
    drop(watch_probe);
    drop(register_probe);

    let database_file = unique_sqlite_file("cross-runtime-service-watch");
    let watch_env = sqlite_runtime_env(&database_file, watch_port);
    let register_env = sqlite_runtime_env(&database_file, register_port);
    let config = sqlite_runtime_config_toml();
    let watch_runtime =
        DiscoveryServiceHostRuntime::from_toml_str_with_env(&config, &watch_env).unwrap();
    watch_runtime
        .bootstrap()
        .initialize_storage()
        .await
        .unwrap();
    let register_runtime =
        DiscoveryServiceHostRuntime::from_toml_str_with_env(&config, &register_env).unwrap();

    let watch_server = watch_runtime.serve_grpc().await.unwrap();
    let register_server = register_runtime.serve_grpc().await.unwrap();
    let watch_channel = Endpoint::from_shared(format!("http://127.0.0.1:{watch_port}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let register_channel = Endpoint::from_shared(format!("http://127.0.0.1:{register_port}"))
        .unwrap()
        .connect()
        .await
        .unwrap();

    let mut watch = DiscoveryWatchServiceClient::new(watch_channel);
    let mut watch_request = Request::new(WatchServiceRequest {
        namespace: "sdkwork".to_string(),
        environment: "development".to_string(),
        service_name: "sdkwork-drive-product".to_string(),
        from_revision: 0,
    });
    add_registry_read_metadata(&mut watch_request);
    let mut stream = watch
        .watch_service(watch_request)
        .await
        .unwrap()
        .into_inner();

    let mut registry = RegistryServiceClient::new(register_channel);
    let mut register = Request::new(register_request());
    add_registry_write_metadata(&mut register);
    registry.register_instance(register).await.unwrap();

    let event = timeout(Duration::from_secs(3), async {
        loop {
            let event = stream.message().await.unwrap().unwrap();
            if event.event_type == common_proto::WatchEventType::Heartbeat as i32 {
                continue;
            }
            return event;
        }
    })
    .await
    .unwrap();
    watch_server.shutdown().await;
    register_server.shutdown().await;

    assert_eq!(
        event.event_type,
        common_proto::WatchEventType::InstanceRegistered as i32
    );
    assert_eq!(event.metadata.as_ref().unwrap().revision, 1);
    let instance = event
        .instance
        .expect("cross-runtime service watch event must include instance");
    assert_eq!(instance.instance_id, "drive-1");
    assert_eq!(instance.service_name, "sdkwork-drive-product");
    assert_eq!(instance.endpoint, "grpc://127.0.0.1:50051");
    assert_eq!(instance.status, ProtoInstanceStatus::Serving as i32);
}

#[tokio::test]
async fn runtime_rejects_missing_tls_certificate_files_before_binding_grpc() {
    let mut env = public_surface_bind_env(0);
    env.insert(
        "SDKWORK_DISCOVERY_RPC_TLS_ENABLED".to_string(),
        "true".to_string(),
    );
    env.insert(
        "SDKWORK_DISCOVERY_RPC_SERVER_TLS_CERT_FILE".to_string(),
        "target/test-missing/sdkwork-discovery/server.crt".to_string(),
    );
    env.insert(
        "SDKWORK_DISCOVERY_RPC_SERVER_TLS_KEY_FILE".to_string(),
        "target/test-missing/sdkwork-discovery/server.key".to_string(),
    );
    let runtime = DiscoveryServiceHostRuntime::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let result = runtime.serve_grpc().await;
    let error = match result {
        Ok(server) => {
            server.shutdown().await;
            panic!("TLS-enabled runtime must reject missing certificate files")
        }
        Err(error) => error,
    };

    assert!(error.to_string().contains("TLS"));
    assert!(error.to_string().contains("server certificate"));
    assert!(!error.to_string().contains("target/test-missing"));
}

#[tokio::test]
async fn runtime_rejects_missing_mtls_client_ca_file_before_binding_grpc() {
    let cert_dir = "target/test-generated/sdkwork-discovery/mtls-missing-client-ca";
    std::fs::create_dir_all(cert_dir).unwrap();
    let server_cert_file = format!("{cert_dir}/server.crt");
    let server_key_file = format!("{cert_dir}/server.key");
    std::fs::write(&server_cert_file, "placeholder certificate").unwrap();
    std::fs::write(&server_key_file, "placeholder private key").unwrap();

    let mut env = public_surface_bind_env(0);
    env.insert(
        "SDKWORK_DISCOVERY_RPC_MTLS_ENABLED".to_string(),
        "true".to_string(),
    );
    env.insert(
        "SDKWORK_DISCOVERY_RPC_SERVER_TLS_CERT_FILE".to_string(),
        server_cert_file,
    );
    env.insert(
        "SDKWORK_DISCOVERY_RPC_SERVER_TLS_KEY_FILE".to_string(),
        server_key_file,
    );
    env.insert(
        "SDKWORK_DISCOVERY_RPC_CLIENT_CA_CERT_FILE".to_string(),
        "target/test-missing/sdkwork-discovery/client-ca.crt".to_string(),
    );
    let runtime = DiscoveryServiceHostRuntime::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let result = runtime.serve_grpc().await;
    let error = match result {
        Ok(server) => {
            server.shutdown().await;
            panic!("mTLS-enabled runtime must reject missing client CA files")
        }
        Err(error) => error,
    };

    assert!(error.to_string().contains("mTLS"));
    assert!(error.to_string().contains("client CA certificate"));
    assert!(!error.to_string().contains("target/test-missing"));
}

#[test]
fn runtime_env_collection_separates_config_file_from_safe_overlay() {
    let env = BTreeMap::from([
        (
            "SDKWORK_DISCOVERY_CONFIG_FILE".to_string(),
            "etc/discovery.example.toml".to_string(),
        ),
        (
            "SDKWORK_DISCOVERY_APPLICATION_PUBLIC_INGRESS_BIND".to_string(),
            "127.0.0.1:19190".to_string(),
        ),
        ("UNRELATED".to_string(), "ignored".to_string()),
    ]);

    let options = DiscoveryServiceHostRuntime::options_from_env(&env).unwrap();

    assert_eq!(
        options.config_file.as_deref(),
        Some("etc/discovery.example.toml")
    );
    assert_eq!(
        options
            .env_overlay
            .get("SDKWORK_DISCOVERY_APPLICATION_PUBLIC_INGRESS_BIND")
            .unwrap(),
        "127.0.0.1:19190"
    );
    assert!(!options
        .env_overlay
        .contains_key("SDKWORK_DISCOVERY_CONFIG_FILE"));
    assert!(!options.env_overlay.contains_key("UNRELATED"));
}

#[test]
fn runtime_env_collection_includes_canonical_database_keys_and_ignores_admin_keys() {
    let env = BTreeMap::from([
        (
            "SDKWORK_DATABASE_NAME".to_string(),
            "sdkwork_ai_dev".to_string(),
        ),
        (
            "SDKWORK_DATABASE_ADMIN_USERNAME".to_string(),
            "postgres".to_string(),
        ),
    ]);

    let options = DiscoveryServiceHostRuntime::options_from_env(&env).unwrap();

    assert_eq!(
        options
            .env_overlay
            .get("SDKWORK_DATABASE_NAME")
            .map(String::as_str),
        Some("sdkwork_ai_dev")
    );
    assert!(!options
        .env_overlay
        .contains_key("SDKWORK_DATABASE_ADMIN_USERNAME"));
}

#[test]
fn runtime_env_collection_rejects_retired_database_aliases() {
    for key in [
        ["SDKWORK", "CLOUD", "DATABASE", "NAME"].join("_"),
        ["SDKWORK", "DISCOVERY", "DATABASE", "NAME"].join("_"),
        ["SDKWORK", "DISCOVERY", "STORAGE", "POSTGRES", "DATABASE"].join("_"),
    ] {
        let env = BTreeMap::from([(key.to_string(), "sdkwork_ai_dev".to_string())]);
        let error = DiscoveryServiceHostRuntime::options_from_env(&env).unwrap_err();

        assert!(error.to_string().contains(&key));
        assert!(error.to_string().contains("SDKWORK_DATABASE_*"));
    }
}

#[test]
fn runtime_summary_contains_only_safe_operational_fields() {
    let env = BTreeMap::from([(
        "SDKWORK_DISCOVERY_APPLICATION_PUBLIC_INGRESS_BIND".to_string(),
        "127.0.0.1:19190".to_string(),
    )]);
    let runtime = DiscoveryServiceHostRuntime::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let summary = runtime.safe_summary();

    assert!(summary.contains("provider=memory"));
    assert!(summary.contains("grpc=127.0.0.1:19190"));
    assert!(!summary.to_ascii_lowercase().contains("password"));
    assert!(!summary.to_ascii_lowercase().contains("secret"));
    assert!(!summary.to_ascii_lowercase().contains("token"));
}

#[test]
fn runtime_summary_includes_storage_safe_summary_without_secret_material() {
    let env = parse_env_example(include_str!("../../../config/host/postgres.env.example"));
    let runtime = DiscoveryServiceHostRuntime::from_toml_str_with_env(
        include_str!("../../../etc/discovery.example.toml"),
        &env,
    )
    .unwrap();

    let summary = runtime.safe_summary();

    assert!(summary.contains("provider=postgres"));
    assert!(summary.contains("storage=\"postgres host=127.0.0.1"));
    assert!(summary.contains("database=sdkwork_ai_dev"));
    assert!(summary.contains("schema=sdkwork_ai_dev"));
    assert!(!summary.to_ascii_lowercase().contains("password"));
    assert!(!summary.to_ascii_lowercase().contains("secret"));
    assert!(!summary.to_ascii_lowercase().contains("token"));
}

fn add_registry_write_metadata<T>(request: &mut Request<T>) {
    request
        .metadata_mut()
        .insert("authorization", "Bearer test-auth-token".parse().unwrap());
    request
        .metadata_mut()
        .insert("access-token", "test-access-token".parse().unwrap());
    request
        .metadata_mut()
        .insert("x-sdkwork-subject-id", "service-1".parse().unwrap());
    request
        .metadata_mut()
        .insert("x-sdkwork-registry-permissions", "write".parse().unwrap());
}

fn add_registry_read_metadata<T>(request: &mut Request<T>) {
    request
        .metadata_mut()
        .insert("authorization", "Bearer test-auth-token".parse().unwrap());
    request
        .metadata_mut()
        .insert("access-token", "test-access-token".parse().unwrap());
    request
        .metadata_mut()
        .insert("x-sdkwork-subject-id", "service-1".parse().unwrap());
    request
        .metadata_mut()
        .insert("x-sdkwork-registry-permissions", "read".parse().unwrap());
}

fn add_config_read_metadata<T>(request: &mut Request<T>) {
    request
        .metadata_mut()
        .insert("authorization", "Bearer test-auth-token".parse().unwrap());
    request
        .metadata_mut()
        .insert("access-token", "test-access-token".parse().unwrap());
    request
        .metadata_mut()
        .insert("x-sdkwork-subject-id", "service-1".parse().unwrap());
    request
        .metadata_mut()
        .insert("x-sdkwork-config-permissions", "read".parse().unwrap());
}

fn add_config_write_metadata<T>(
    request: &mut Request<T>,
    idempotency_key: &'static str,
    request_hash: &'static str,
) {
    request
        .metadata_mut()
        .insert("authorization", "Bearer test-auth-token".parse().unwrap());
    request
        .metadata_mut()
        .insert("access-token", "test-access-token".parse().unwrap());
    request
        .metadata_mut()
        .insert("x-sdkwork-subject-id", "operator-1".parse().unwrap());
    request
        .metadata_mut()
        .insert("x-sdkwork-config-permissions", "publish".parse().unwrap());
    request
        .metadata_mut()
        .insert("idempotency-key", idempotency_key.parse().unwrap());
    request
        .metadata_mut()
        .insert("x-request-hash", request_hash.parse().unwrap());
}

const SERVICE_TOKEN_SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";
const VERIFIED_ACCESS_TOKEN: &str = "verified-access-token";
const VERIFIED_SERVICE_TOKEN: &str = "sdkwork-discovery-v1.eyJhbGciOiJIUzI1NiIsInR5cCI6InNka3dvcmsuZGlzY292ZXJ5LnNlcnZpY2UtdG9rZW4udjEifQ.eyJpc3MiOiJzZGt3b3JrLWRpc2NvdmVyeSIsImF1ZCI6InNka3dvcmstZGlzY292ZXJ5LXJwYyIsInN1YiI6InNlcnZpY2UtMSIsInRlbmFudF9pZCI6InNka3dvcmsiLCJpYXRfbXMiOjE3MDAwMDAwMDAwMDAsImV4cF9tcyI6NDEwMjQ0NDgwMDAwMCwiYWNjZXNzX3Rva2VuX3NoYTI1NiI6ImIxNjFiZWIyOTYzOTYyNTJiZmQ2ZTc3NTZkMjg3ZTY2Nzk5MTM3ZjAyMjE5YzhkZTgzZTgyMTM4MjcxYjlkMjciLCJyZWdpc3RyeV9wZXJtaXNzaW9ucyI6WyJyZWFkIiwid3JpdGUiXSwiY29uZmlnX3Blcm1pc3Npb25zIjpbInJlYWQiLCJwdWJsaXNoIl19.yTqskA6sBzRoF2BqYFgX2Qn2PIYyQqLSuNXX5G1kpNs";

fn add_verified_service_token_metadata<T>(request: &mut Request<T>) {
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {VERIFIED_SERVICE_TOKEN}").parse().unwrap(),
    );
    request
        .metadata_mut()
        .insert("access-token", VERIFIED_ACCESS_TOKEN.parse().unwrap());
}

fn sqlite_runtime_env(database_file: &std::path::Path, grpc_port: u16) -> BTreeMap<String, String> {
    let mut env = surface_bind_env(grpc_port, grpc_port);
    env.insert(
        "SDKWORK_DISCOVERY_STORAGE_PROVIDER".to_string(),
        "sqlite".to_string(),
    );
    env.insert(
        "SDKWORK_DATABASE_FILE".to_string(),
        database_file.to_string_lossy().into_owned(),
    );
    env.insert(
        "SDKWORK_DATABASE_MAX_CONNECTIONS".to_string(),
        "2".to_string(),
    );
    env.insert(
        "SDKWORK_DISCOVERY_WATCH_HEARTBEAT_INTERVAL_MS".to_string(),
        "50".to_string(),
    );
    env
}

fn sqlite_runtime_config_toml() -> String {
    include_str!("../../../etc/discovery.example.toml").replace(
        r#"[storage]
provider = "memory""#,
        r#"[storage]
provider = "memory"
apply_initial_schema = true"#,
    )
}

fn unique_sqlite_file(test_name: &str) -> PathBuf {
    let id = TEST_STORAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let process_id = std::process::id();
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let directory = PathBuf::from("target")
        .join("test-generated")
        .join("sdkwork-discovery")
        .join(test_name)
        .join(format!("run-{process_id}-{timestamp_ms}-{id}"));
    std::fs::create_dir_all(&directory).unwrap();
    directory.join("discovery.sqlite")
}

fn register_request() -> RegisterInstanceRequest {
    RegisterInstanceRequest {
        namespace: "sdkwork".to_string(),
        environment: "development".to_string(),
        service_name: "sdkwork-drive-product".to_string(),
        instance_id: "drive-1".to_string(),
        endpoint: "grpc://127.0.0.1:50051".to_string(),
        protocol: "grpc".to_string(),
        version: "0.1.0".to_string(),
        region: "local".to_string(),
        zone: "local-a".to_string(),
        weight: 100,
        priority: 0,
        status: ProtoInstanceStatus::Serving as i32,
        metadata: Default::default(),
        lease_ttl_seconds: 30,
        expected_revision: None,
        persistent: false,
        health_check: None,
    }
}
