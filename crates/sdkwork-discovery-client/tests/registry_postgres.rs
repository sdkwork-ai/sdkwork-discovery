//! Real-PostgreSQL contract for the HTTP registry.
//!
//! Skipped unless `SDKWORK_TEST_DATABASE_URL` points at a disposable
//! PostgreSQL database (for example
//! `postgres://postgres:postgres@127.0.0.1:55432/postgres`). The test creates a
//! unique schema, exercises registration idempotency, lease heartbeat,
//! discovery ordering, and deregistration, then drops the schema.

use std::time::{SystemTime, UNIX_EPOCH};

use sdkwork_discovery_client::{
    HttpInstanceRegistration, HttpRegistry, RegistryIdentity, RegistryStatus,
};
use sqlx::postgres::PgPoolOptions;

fn test_database_url() -> Option<String> {
    std::env::var("SDKWORK_TEST_DATABASE_URL")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn unique_schema() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("registry_test_{}_{}", std::process::id(), nanos)
}

#[tokio::test]
async fn registration_is_idempotent_and_heartbeat_extends_the_lease() {
    let Some(base_url) = test_database_url() else {
        eprintln!("SDKWORK_TEST_DATABASE_URL is not set; skipping PostgreSQL registry contract");
        return;
    };
    let schema = unique_schema();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base_url)
        .await
        .expect("test database connects");
    // The schema name is generated from the process id and a nanosecond
    // timestamp (ASCII alphanumeric + underscore), so it is injection-safe;
    // SQLx 0.9 requires the explicit audit assertion for dynamic DDL.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&admin)
        .await
        .expect("test schema creates");

    let separator = if base_url.contains('?') { '&' } else { '?' };
    let schema_url = format!("{base_url}{separator}options=-c%20search_path%3D{schema}");
    let registry = HttpRegistry::connect(
        &schema_url,
        RegistryIdentity::new("platform", "integration"),
        2,
    )
    .await
    .expect("registry connects");
    registry.ensure_schema().await.expect("schema ensures");

    let mut first = HttpInstanceRegistration::serving(
        "cloudrouter",
        "gateway-1:8080",
        "https://api.example.com",
        "0.1.37",
        30,
    );
    first
        .metadata
        .insert("deployment_profile".to_owned(), "standalone".to_owned());
    first
        .metadata
        .insert("runtime_target".to_owned(), "container".to_owned());
    first.priority = 10;
    first.weight = 100;
    let first_lease = registry.register(&first).await.expect("first register");

    let second_lease = registry.register(&first).await.expect("second register");
    assert_eq!(
        first_lease.lease_id, second_lease.lease_id,
        "a live instance keeps its lease across re-registration"
    );
    assert_eq!(first_lease.instance_id, second_lease.instance_id);

    let discovered = registry.discover("cloudrouter").await.expect("discover");
    assert_eq!(discovered.len(), 1, "idempotent registration keeps one row");
    assert_eq!(discovered[0].endpoint, "https://api.example.com");
    assert_eq!(discovered[0].priority, 10);

    let renewed = registry
        .heartbeat(&first_lease.lease_id, 60)
        .await
        .expect("heartbeat")
        .expect("lease is alive");
    assert!(
        renewed.expires_at_ms > first_lease.expires_at_ms,
        "heartbeat extends the lease"
    );

    let mut sibling = HttpInstanceRegistration::serving(
        "cloudrouter",
        "gateway-2:8080",
        "https://api2.example.com",
        "0.1.37",
        30,
    );
    sibling.priority = 10;
    sibling.weight = 500;
    let _sibling_lease = registry.register(&sibling).await.expect("sibling register");
    let ordered = registry.discover("cloudrouter").await.expect("discover");
    assert_eq!(ordered.len(), 2);
    assert_eq!(
        ordered[0].instance_id, "gateway-2:8080",
        "higher weight wins inside the same priority group"
    );

    let demoted = registry
        .report_status("cloudrouter", "gateway-2:8080", RegistryStatus::NotServing)
        .await
        .expect("status report");
    assert!(demoted);
    let after_demotion = registry.discover("cloudrouter").await.expect("discover");
    assert_eq!(after_demotion.len(), 1);
    assert_eq!(after_demotion[0].instance_id, "gateway-1:8080");

    assert!(registry
        .deregister("cloudrouter", "gateway-1:8080")
        .await
        .expect("deregister"));
    assert!(registry
        .discover("cloudrouter")
        .await
        .expect("discover")
        .is_empty());
    let _ = registry.deregister("cloudrouter", "gateway-2:8080").await;

    // Expiry hygiene: a short lease is filtered immediately after expiry and
    // the producer-side sweep soft-deletes it.
    let expiring = HttpInstanceRegistration::serving(
        "expiry",
        "short-lived:1",
        "https://short.example.com",
        "0.1.37",
        1,
    );
    registry
        .register(&expiring)
        .await
        .expect("short lease registers");
    assert_eq!(
        registry.discover("expiry").await.expect("discover").len(),
        1
    );
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let reaped = registry.reap_expired(100).await.expect("expired rows reap");
    assert!(reaped >= 1, "the expired row must be soft-deleted");
    assert!(registry
        .discover("expiry")
        .await
        .expect("discover after expiry")
        .is_empty());

    registry.close().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await
        .expect("test schema drops");
    admin.close().await;
}
