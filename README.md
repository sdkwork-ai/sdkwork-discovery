# sdkwork-discovery
repository-kind: foundation-dependency

Rust gRPC control plane for SDKWork service discovery and versioned runtime configuration.

## Scope

- Service Registry: instance registration, lease renewal, deregistration, exact instance retrieval, discovery, and watch events.
- HTTP Service Registry (`sdkwork-discovery-client`, `DISCOVERY_SPEC.md` §12.1): backend Rust SDKs register every instance/port and resolve peer HTTP base URLs directly from the registry database with heartbeat leases, QoS ordering, failure demotion, and last-known-good degradation.
- Config Registry: drafts, immutable releases, rollback, effective configuration, and watch events.
- Typed runtime configuration for server/container deployment.
- SDKWork RPC contracts under `proto/` and RPC SDK manifest under `sdks/`.

No browser UI is included in this application.

## Development

```bash
cargo test --workspace
cargo fmt --all -- --check
pnpm.cmd verify
pnpm.cmd dev
```

Topology-aware local dev loads `etc/topology/` profiles via `@sdkwork/app-topology` (`pnpm dev`, `pnpm dev:cloud`). See `docs/topology-standard.md`.

`pnpm.cmd` should be used on Windows if PowerShell blocks `pnpm.ps1`.

## Running The Product

The product crate has a runnable binary that loads runtime configuration, applies the safe `SDKWORK_DISCOVERY_*` environment overlay, validates policy, initializes configured storage, starts the tonic gRPC server, and prints a redacted operational summary:

```bash
cargo run -p sdkwork-discovery-service-host --offline
```

Use `SDKWORK_DISCOVERY_CONFIG_FILE` to point at a host-local TOML config file. This key selects the file only; it is not forwarded into the runtime config overlay.

The default example binds **application.public-ingress** to `127.0.0.1:19090` and **operations.control-ingress** to `127.0.0.1:19091`. If both surfaces share the same address, the product runs a single combined server; otherwise it binds separate internal and backend surfaces. Health and reflection are registered only when enabled by runtime config. The configured `default_deadline_ms` is applied to the tonic server as the default RPC request timeout.

The safe process env overlay supports lifecycle/runtime fields, topology surface bind keys, storage selection and storage connection fields, config/watch controls, and RPC security toggles:

- `SDKWORK_DISCOVERY_ENVIRONMENT`, `SDKWORK_DISCOVERY_CONFIG_PROFILE`, `SDKWORK_DISCOVERY_RUNTIME_TARGET`, `SDKWORK_DISCOVERY_DEPLOYMENT_PROFILE`, `SDKWORK_DISCOVERY_PROFILE_ID`
- `SDKWORK_DISCOVERY_APPLICATION_PUBLIC_INGRESS_BIND`, `SDKWORK_DISCOVERY_APPLICATION_PUBLIC_GRPC_URL`, `SDKWORK_DISCOVERY_OPERATIONS_CONTROL_INGRESS_BIND`, `SDKWORK_DISCOVERY_OPERATIONS_CONTROL_GRPC_URL`, `SDKWORK_DISCOVERY_RPC_DEFAULT_DEADLINE_MS`
- `SDKWORK_DISCOVERY_STORAGE_PROVIDER`, `SDKWORK_DISCOVERY_CONFIG_REGISTRY_ENABLED`
- Canonical PostgreSQL/SQLite database overlays: `SDKWORK_DATABASE_ENGINE`, `SDKWORK_DATABASE_HOST`, `SDKWORK_DATABASE_PORT`, `SDKWORK_DATABASE_NAME`, `SDKWORK_DATABASE_SCHEMA`, `SDKWORK_DATABASE_USERNAME`, `SDKWORK_DATABASE_PASSWORD_FILE`, `SDKWORK_DATABASE_SSL_MODE`, `SDKWORK_DATABASE_ACQUIRE_TIMEOUT`, `SDKWORK_DATABASE_MAX_CONNECTIONS`, `SDKWORK_DATABASE_FILE`
- Provider-specific Redis/etcd/Consul overlays: `SDKWORK_DISCOVERY_STORAGE_<PROVIDER>_HOST`, `SDKWORK_DISCOVERY_STORAGE_<PROVIDER>_PORT`, `SDKWORK_DISCOVERY_STORAGE_<PROVIDER>_DATABASE`, `SDKWORK_DISCOVERY_STORAGE_<PROVIDER>_USERNAME`, `SDKWORK_DISCOVERY_STORAGE_<PROVIDER>_PASSWORD_FILE`, `SDKWORK_DISCOVERY_STORAGE_<PROVIDER>_TLS_ENABLED`, `SDKWORK_DISCOVERY_STORAGE_<PROVIDER>_CONNECT_TIMEOUT_MS`, `SDKWORK_DISCOVERY_STORAGE_<PROVIDER>_MAX_CONNECTIONS`
- `SDKWORK_DISCOVERY_WATCH_ENABLED`, `SDKWORK_DISCOVERY_WATCH_MAX_STREAMS`, `SDKWORK_DISCOVERY_WATCH_EVENT_BUFFER_SIZE`, `SDKWORK_DISCOVERY_WATCH_HEARTBEAT_INTERVAL_MS`, `SDKWORK_DISCOVERY_WATCH_DURABLE_POLL_INTERVAL_MS`, `SDKWORK_DISCOVERY_WATCH_DURABLE_REPLAY_BATCH_SIZE`
- `SDKWORK_DISCOVERY_RPC_AUTH_MODE`, `SDKWORK_DISCOVERY_RPC_ALLOW_UNSIGNED_LOCAL_CONTEXT`, `SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_HMAC_SECRET_FILE`, `SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_ISSUER`, `SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_AUDIENCE`, `SDKWORK_DISCOVERY_RPC_SERVICE_TOKEN_MAX_TTL_SECONDS`, `SDKWORK_DISCOVERY_RPC_TLS_ENABLED`, `SDKWORK_DISCOVERY_RPC_MTLS_ENABLED`, `SDKWORK_DISCOVERY_RPC_REFLECTION_ENABLED`, `SDKWORK_DISCOVERY_RPC_HEALTH_ENABLED`
- `SDKWORK_DISCOVERY_METRICS_BIND` when the server binary is built with the `prometheus` feature (enabled in release packages). Defaults to `127.0.0.1:9090`; production deployments typically set `0.0.0.0:9090` for in-cluster scraping.

Production-oriented config template: `etc/discovery.production.example.toml`. Development template: `etc/discovery.example.toml`.

Operator runbooks: [production deployment](docs/runbooks/RUNBOOK-production-server-deployment.md), [database migration rollback](docs/runbooks/RUNBOOK-database-migration-rollback.md).

Release evidence: [CHANGELOG](docs/changelogs/CHANGELOG.md), [RELEASE v0.1.0](docs/releases/RELEASE-v0.1.0.md), [release gate review](docs/engineering/reviews/REVIEW-20260623-release-gate-v0.1.0.md).

TLS and mTLS are configured through secret-file references under `[security]` or the matching private process env overlay:

- `server_tls_cert_file` or `SDKWORK_DISCOVERY_RPC_SERVER_TLS_CERT_FILE`
- `server_tls_key_file` or `SDKWORK_DISCOVERY_RPC_SERVER_TLS_KEY_FILE`
- `client_ca_cert_file` or `SDKWORK_DISCOVERY_RPC_CLIENT_CA_CERT_FILE` when mTLS is enabled

Production config requires TLS or mTLS, PostgreSQL storage, non-loopback bind, reflection disabled or access-controlled by a governed deployment layer, no unsigned local caller context, no literal config secrets, and no automatic schema application. Certificate/key contents are never accepted inline and are not included in operational summaries.

`allow_unsigned_local_context = true` is a development/test loopback-only mode for local service-to-service smoke calls that pass `x-sdkwork-subject-id` and permission metadata directly. When disabled, the RPC adapter rejects unsigned caller context and requires a signed SDKWork Discovery service token.

Service-token mode uses `authorization: Bearer <sdkwork-discovery-v1 token>` plus `access-token`. The signed token is HMAC-SHA256 verified with `[security.service_token].hmac_secret_file`, must match configured issuer/audience and max TTL, carries `sub`, `registry_permissions`, and `config_permissions` claims, and binds the `access-token` value through an `access_token_sha256` claim. The secret file is private process config, must contain at least 32 bytes after trailing whitespace is trimmed, and is never included in operational summaries.

`proto/` and `sdks/sdkwork-discovery-rpc-sdk/` are the source RPC contracts. The Rust proto crate generates tonic/prost bindings at build time from the checked-in `.proto` files; generated output is not hand-edited.

## Runtime Storage

Runtime storage is selected by typed config under `[storage]`. Supported provider names are:

- `memory`: deterministic local/test adapter.
- `postgres`: durable PostgreSQL adapter for registry, config, and watch storage. Apply `database/migrations/postgres/` through `pnpm db:migrate` or the sdkwork-database CLI before serving traffic.
- `sqlite`: durable local/test/small single-node adapter for registry, config, and watch storage. Apply `tests/fixtures/database/sqlite/migrations/` through `pnpm db:migrate` before serving traffic, or set `apply_initial_schema = true` only in non-production bootstrap.
- `redis`: durable Redis-backed registry, config, and watch storage using the `sdkwork:discovery:v1` key namespace. Suitable for single-writer or restart recovery deployments; multi-writer clusters should prefer PostgreSQL or add external coordination.
- `etcd`: distributed registry/watch configuration shape, fail-fast at product bootstrap until the adapter lands.
- `consul`: distributed registry/watch configuration shape, fail-fast at product bootstrap until the adapter lands.

Provider configuration uses structured host/port/database fields and `password_file` secret references. Direct password values are rejected by the runtime config validator. Production config rejects the `memory` and `sqlite` providers.
The canonical `SDKWORK_DATABASE_*` env overlay is the only database authority. It maps PostgreSQL fields into `[storage.postgres]` and SQLite `SDKWORK_DATABASE_FILE` into `[storage.sqlite]`. If `SDKWORK_DISCOVERY_STORAGE_PROVIDER` and `SDKWORK_DATABASE_ENGINE` are both present, they must describe the same provider or startup fails before serving traffic. PostgreSQL `schema` maps to the connection `search_path`, and startup requires the database, schema, and username to match the active workspace environment.
SQLite uses `[storage.sqlite]` with `file` and `max_connections`. `:memory:` always runs with a single connection so tests do not split state across independent in-memory databases. Production server/container config rejects SQLite and must use PostgreSQL for shared durable state.

## Watch Semantics

The watch RPC surface streams revision-ordered stored registry/config events from the configured watch store starting at `from_revision`, then continues with live events published by the current runtime process. This gives every watcher durable catch-up before live delivery.

`WatchService` registry mutation events include a `ServiceInstance` payload so clients can update local service discovery caches from the stream. Registered, updated, renewed, and status events are enriched from current registry state. Deregistered events, or events whose current row is no longer available, return an identity tombstone with namespace, environment, service name, instance id, event revision, and `INSTANCE_STATUS_NOT_SERVING`. Strict event-time instance snapshots require a future storage schema review.

Registry lease cleanup is governed by `[registry].expiry_scan_interval_ms` and `[registry].expiry_scan_batch_size`. The scan interval controls how often the runtime actor attempts cleanup; the batch size bounds each transaction so large fleets and multi-process failover do not turn one cleanup pass into an unbounded database scan.

Watch is governed by typed runtime config under `[watch]`. When `enabled = false`, the internal Watch service is not registered. When enabled, `max_streams` caps concurrent Watch streams and excess clients receive gRPC `ResourceExhausted`; `event_buffer_size` bounds each server-side stream queue; `heartbeat_interval_ms` emits a `heartbeat` event with the last delivered revision while the stream is idle; `durable_poll_interval_ms` controls how often streams poll durable watch storage for changes written by other runtime processes; `durable_replay_batch_size` bounds each replay query so slow consumers catch up without loading unbounded event history.

Live fanout is process-local. In a horizontally scaled PostgreSQL deployment, clients should connect through a sticky stream endpoint or reconnect with the last observed revision after failover until a provider-specific distributed fanout adapter is added. Reconnects remain safe because the durable watch store is the source of truth for revision replay.

## Standards

Read `AGENTS.md`, `specs/component.spec.json`, and `docs/superpowers/specs/2026-06-09-sdkwork-discovery-control-plane-design.md` before changing public behavior, RPC contracts, storage contracts, runtime config, or verification.

## Documentation Canon

- [docs/README.md](docs/README.md)
- [docs/product/prd/PRD.md](docs/product/prd/PRD.md)
- [docs/architecture/tech/TECH_ARCHITECTURE.md](docs/architecture/tech/TECH_ARCHITECTURE.md)

## Application Roots

- [apps directory index](apps/README.md)
