> Migrated from `docs/superpowers/plans/2026-06-09-sdkwork-discovery-control-plane.md` on 2026-06-24.
> Owner: SDKWork maintainers

> **Status:** Implemented in the main agent session. This plan is retained as execution evidence and has been updated to match the delivered workspace.

**Goal:** Build the first SDKWork-standard Rust control plane for service discovery and versioned runtime configuration.

**Architecture:** Implement a focused Rust workspace with contract, config, storage-port, memory-store, core-service, and product-bootstrap crates. Keep domain logic independent of generated RPC transport while landing proto and RPC manifest contracts.

**Tech Stack:** Rust 2021, Cargo workspace, serde, toml, thiserror, uuid, sha2, SDKWork specs, proto3 contracts.

---

### Task 1: Repository Standards

**Files:**
- Create: `AGENTS.md`
- Create: `CODEX.md`
- Create: `CLAUDE.md`
- Create: `GEMINI.md`
- Create: `.sdkwork/README.md`
- Create: `.sdkwork/skills/README.md`
- Create: `.sdkwork/plugins/README.md`
- Create: `specs/README.md`
- Create: `specs/component.spec.json`
- Create: `agent.app.config.json`
- Create: `README.md`

- [x] Write repository metadata and local component specs.
- [x] Verify paths point to `../sdkwork-specs/`.

### Task 2: Cargo Workspace And Failing Tests

**Files:**
- Create: `Cargo.toml`
- Create: each crate `Cargo.toml`
- Create tests under `crates/*/tests/`

- [x] Write failing tests for config normalization and validation.
- [x] Write failing tests for registry lease lifecycle.
- [x] Write failing tests for config publish and effective resolution.
- [x] Write failing tests for core permission enforcement.
- [x] Run focused tests first, then `cargo test --workspace`.

### Task 3: Contract And Config Crates

**Files:**
- Create: `crates/sdkwork-discovery-contract/src/*.rs`
- Create: `crates/sdkwork-discovery-config/src/*.rs`

- [x] Implement domain contracts and typed errors.
- [x] Implement runtime config loading, safe env overlay, production validation, and watch runtime governance config.
- [x] Run focused contract/config tests.

### Task 4: Storage Ports And Memory Store

**Files:**
- Create: `crates/sdkwork-discovery-storage-contract/src/*.rs`
- Create: `crates/sdkwork-discovery-storage-memory/src/*.rs`

- [x] Implement storage traits.
- [x] Implement deterministic memory store.
- [x] Implement durable PostgreSQL registry, config, and watch storage adapter.
- [x] Implement durable SQLite local/test/small single-node registry, config, and watch storage adapter.
- [x] Run memory, PostgreSQL, and SQLite store tests.

### Task 5: Core Service

**Files:**
- Create: `crates/sdkwork-discovery-core/src/*.rs`

- [x] Implement service layer permission and policy enforcement.
- [x] Run core tests.

### Task 6: RPC Contract Artifacts

**Files:**
- Create: `proto/sdkwork/discovery/common/v1/discovery_types.proto`
- Create: `proto/sdkwork/discovery/internal/v1/registry_service.proto`
- Create: `proto/sdkwork/discovery/internal/v1/discovery_config_service.proto`
- Create: `proto/sdkwork/discovery/backend/v3/discovery_admin_service.proto`
- Create: `sdks/sdkwork-discovery-rpc-sdk/sdkwork-discovery-rpc.manifest.json`
- Create: `sdks/sdkwork-discovery-rpc-sdk/README.md`

- [x] Land proto source and RPC manifest with operation ids.
- [x] Wire Rust proto crate generation from checked-in `.proto` source.
- [x] Implement tonic RPC server adapter, health/reflection controls, TLS/mTLS validation, bounded shutdown, Watch durable replay, live fanout, concurrency caps, and heartbeat.

### Task 7: Verification

**Files:**
- Modify as needed.

- [x] Run `cargo fmt --all -- --check`.
- [x] Run `cargo test --workspace`.
- [x] Run `pnpm.cmd verify`.
- [x] Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] Report exact verification evidence and known deferred items.

