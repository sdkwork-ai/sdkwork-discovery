# Release v0.1.0 — SDKWork Discovery Server

Status: production-ready candidate
Application: sdkwork-discovery
Version: 0.1.0
Runtime target: server
Deployment profile: cloud (server tar.gz)
Package id: linux-x64-cloud-server-tar-gz (primary); platform-specific tar.gz via `sdkwork.workflow.json`
Specs: RELEASE_SPEC.md, APP_MANIFEST_SPEC.md, DISCOVERY_SPEC.md

## Summary

First production-oriented release of the SDKWork Discovery control plane: gRPC service registry, versioned runtime configuration, revision-ordered watch, durable PostgreSQL storage, security hardening, observability hooks, operator runbooks, and commercial server packaging.

## Artifacts

| Artifact | Location |
| --- | --- |
| Server tar.gz | `dist/server/sdkwork-discovery-<version>-<platform>-<arch>-server.tar.gz` |
| Install guide | `INSTALL.md` (inside package) |
| Production config template | `config/discovery.production.example.toml` |
| Install manifest | `install-manifest.json` |
| Checksums | `checksums.sha256` |

Container image reference (manifest): `registry.sdkwork.com/apps/sdkwork-discovery` (`agent.app.config.json`)

## Compatibility

- **RPC contracts**: `proto/` and `sdkwork-discovery-rpc-sdk` at workspace version 0.1.0
- **Database**: apply `database/migrations/postgres/` before serving production traffic
- **Breaking changes**: initial public slice; future breaking RPC or schema changes require migration notes per `MIGRATION_SPEC.md`

## Operator Impact

- Production requires PostgreSQL, TLS or mTLS, service-token HMAC secret file (≥ 32 bytes), and non-loopback bind
- Apply migrations before start; see [RUNBOOK-production-server-deployment.md](../runbooks/RUNBOOK-production-server-deployment.md)
- Rollback: [RUNBOOK-database-migration-rollback.md](../runbooks/RUNBOOK-database-migration-rollback.md)

## Integrator Impact

- Consume `sdkwork-discovery-rpc-sdk` (Rust/TypeScript); do not call discovery from browser clients unless a governed gRPC-Web ADR exists
- Register with `namespace + environment + service_name + instance_id`
- Watch clients must reconnect from last observed revision after failover

## Rollout

1. Apply database migrations in target environment
2. Deploy config + secret files
3. Start `sdkwork-discovery-service-host` with `SDKWORK_DISCOVERY_CONFIG_FILE`
4. Verify gRPC health and scrape Prometheus on `SDKWORK_DISCOVERY_METRICS_BIND`
5. Smoke: register test instance, read effective config, open watch from revision 0

## Rollback

1. Stop new instances; restore previous server package binary if needed
2. Follow database migration rollback runbook when schema rollback is required
3. Re-point callers to previous discovery endpoint if DNS/ingress changed

## Verification Evidence

| Gate | Command |
| --- | --- |
| Repository verify | `pnpm run verify` |
| Standards | `cargo test -p sdkwork-discovery-standards` |
| Package contract | `pnpm run release:validate` |
| Docs canon | `pnpm run verify:docs` |
| Database framework | `pnpm run test:contract:database` |

CI: `.github/workflows/verify.yml` (verify + package-smoke on ubuntu-24.04)

## Supply Chain

- Signing: not required (`sdkwork.workflow.json` → `signingRequired: false`)
- SBOM: not required (`sbomRequired: false`); placeholders documented in workflow lifecycle
- Artifact attestations: enabled in workflow security block

## Deferred (Non-Blocking)

- OTLP tracing export
- etcd/Consul storage adapters
- App store listing (`publish.status: DRAFT`)

## Changelog

See [CHANGELOG.md](../changelogs/CHANGELOG.md).

## Linked Evidence

- [Release gate review](../engineering/reviews/REVIEW-20260623-release-gate-v0.1.0.md)
- [PRD.md](../product/prd/PRD.md)
