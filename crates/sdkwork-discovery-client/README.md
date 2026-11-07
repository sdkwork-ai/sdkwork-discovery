# sdkwork-discovery-client

Direct-PostgreSQL HTTP service registry client for SDKWork backend SDKs
(`DISCOVERY_SPEC.md` section 12.1).

## What It Owns

- `HttpRegistry`: idempotent instance registration, lease heartbeat, soft
  deregistration, status reporting, and healthy-target discovery against the
  canonical `discovery_service_instance` table.
- `HttpRegistryResolver`: a cache-first resolver implementing
  `sdkwork_utils_rust::service_base_url::HttpServiceRegistry`, with background
  snapshot refresh, priority/weight QoS selection, consecutive-failure
  demotion, cooldown, and last-known-good degradation.
- `REGISTRY_TABLE_DDL`: the canonical table/index DDL for a backend that hosts
  the registry in its own schema.

## Registration Guarantees

- The unique identity `(namespace, environment, service_name, instance_id)`
  makes registration an upsert; restarts and replica races never duplicate a
  row.
- A still-live instance keeps its lease id across re-registration; the lease
  rotates only after expiry or deregistration.
- A heartbeat renews `expires_at_ms`; discovery filters expired,
  soft-deleted, and `not_serving` rows.
- Every gateway instance/port registers its own `instance_id`
  (`<profile-id>:<hostname>:<port>`).

## Consumer Bootstrap

```rust
let identity = sdkwork_discovery_client::registry_identity_from_env();
let resolver = sdkwork_discovery_client::install_resolver_from_env(
    identity,
    sdkwork_discovery_client::HttpRegistryConfig::default(),
)
.await;
```

`install_resolver_from_env` uses `SDKWORK_DISCOVERY_DATABASE_URL` when set and
falls back to `SDKWORK_DATABASE_URL`. When neither is configured it returns
`None`; callers keep their authored topology values.

## Resolution Precedence

1. explicit dependency override;
2. the surface composed in-process (no HTTP base URL);
3. a healthy registry instance (this crate);
4. the authored topology public URL;
5. the documented split default.

A registry outage keeps the last-known-good snapshot and never fails startup.

## Tests

```bash
cargo test -p sdkwork-discovery-client
```

The tests cover QoS ordering, cooldown demotion, success reset, and
cache-only resolution. The SQL contract is exercised against PostgreSQL by the
deployment regression run (`DISCOVERY_SPEC.md` section 16).
