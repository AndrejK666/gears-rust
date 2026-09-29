#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(all(
    feature = "integration",
    any(feature = "sqlite", feature = "pg", feature = "mysql")
))]

//! The outbox schema upgrades in place from what an earlier toolkit-db created
//! (constructorfabric/gears-rust#5044).
//!
//! toolkit-db 0.16 added `trace` to the body and dead-letter tables and a trace
//! table with three indexes by editing the shipped `m001` migration. A database
//! whose gear journal already recorded `m001` kept the old shape, and every
//! enqueue failed on the missing column. `m002` adds what is missing.
//!
//! Each backend runs one scenario against one database, over three table
//! families side by side:
//!
//! - two **upgraded** families (the default prefix and a custom one, so both
//!   halves of the migration naming scheme are exercised): `m001` is applied
//!   and recorded, then the 0.16 additions are removed again, which is exactly
//!   the shape a 0.15 `m001` left behind. Running the full migration set must
//!   apply only `m002`, and an enqueue - traced, with one entity dead-lettered,
//!   so every added column and table is written - must then succeed.
//! - one **fresh** family: the full migration set on an empty schema, where
//!   `m001` already creates everything and `m002` must be a no-op.
//!
//! The upgraded families must end up with the same columns, types,
//! nullability and index definitions as the fresh one, and a second run of the
//! migrations must apply nothing and change nothing.

mod common;

use std::sync::Arc;
use std::time::Duration;

use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::MigrationTrait;
use toolkit_db::migration_runner::run_migrations_for_testing;
use toolkit_db::outbox::{
    DeadLetterFilter, LeasedMessageHandler, MessageResult, Outbox, OutboxMessage, Partitions,
    Records, WorkerTuning, outbox_migrations, outbox_migrations_with_prefix,
};
use toolkit_db::{ConnectOpts, Db, connect_db};

const DEFAULT_PREFIX: &str = "toolkit_outbox";
const CUSTOM_PREFIX: &str = "mini_chat_outbox";
const FRESH_PREFIX: &str = "fresh_outbox";

/// The payload the handler rejects, so the batch leaves one dead letter.
const BAD: &[u8] = b"bad";

fn migrations(prefix: &str) -> Vec<Box<dyn MigrationTrait>> {
    if prefix == DEFAULT_PREFIX {
        outbox_migrations()
    } else {
        outbox_migrations_with_prefix(prefix).unwrap()
    }
}

/// Only the schema migration, as toolkit-db 0.15 shipped it under this name.
fn schema_migration_only(prefix: &str) -> Vec<Box<dyn MigrationTrait>> {
    let mut all = migrations(prefix);
    all.truncate(1);
    assert!(
        all[0]
            .name()
            .starts_with("m001_create_toolkit_outbox_schema"),
        "the first outbox migration is the schema, got {}",
        all[0].name()
    );
    all
}

async fn exec(raw: &DatabaseConnection, sql: String) {
    raw.execute_raw(Statement::from_string(
        raw.get_database_backend(),
        sql.clone(),
    ))
    .await
    .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// Remove what 0.16 added to `m001`, leaving the shape a 0.15 `m001` created.
async fn strip_to_pre_trace_shape(raw: &DatabaseConnection, prefix: &str) {
    exec(raw, format!("DROP TABLE {prefix}_trace")).await;
    for table in ["body", "dead_letters"] {
        exec(
            raw,
            format!("ALTER TABLE {prefix}_{table} DROP COLUMN trace"),
        )
        .await;
    }
}

/// One line per column and one per index of the whole schema, in a form that
/// is comparable across table families.
fn catalog_queries(backend: DbBackend) -> [&'static str; 2] {
    match backend {
        DbBackend::Postgres => [
            "SELECT table_name::text || '.' || column_name::text || ' ' || data_type::text \
             || COALESCE('(' || character_maximum_length::text || ')', '') \
             || ' ' || is_nullable::text AS d \
             FROM information_schema.columns WHERE table_schema = current_schema()",
            "SELECT tablename::text || ' ' || indexdef AS d \
             FROM pg_indexes WHERE schemaname = current_schema()",
        ],
        DbBackend::MySql => [
            "SELECT CAST(CONCAT(table_name, '.', column_name, ' ', column_type, ' ', \
             is_nullable, ' ', COALESCE(character_set_name, '-')) AS CHAR) AS d \
             FROM information_schema.columns WHERE table_schema = DATABASE()",
            "SELECT CAST(CONCAT(table_name, ' ', index_name, ' ', non_unique, ' ', \
             seq_in_index, ' ', column_name) AS CHAR) AS d \
             FROM information_schema.statistics WHERE table_schema = DATABASE()",
        ],
        DbBackend::Sqlite => [
            "SELECT m.name || '.' || p.name || ' ' || p.type || ' ' || p.\"notnull\" AS d \
             FROM sqlite_master m JOIN pragma_table_info(m.name) p WHERE m.type = 'table'",
            "SELECT tbl_name || ' ' || name || ' ' || COALESCE(sql, '-') AS d \
             FROM sqlite_master WHERE type = 'index'",
        ],
        other => panic!("no catalog queries for {other:?}"),
    }
}

/// The columns and indexes of one table family, with its prefix replaced by a
/// placeholder so two families compare equal when their shape is equal.
async fn schema_of(raw: &DatabaseConnection, prefix: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for sql in catalog_queries(raw.get_database_backend()) {
        let rows = raw
            .query_all_raw(Statement::from_string(raw.get_database_backend(), sql))
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
        lines.extend(
            rows.iter()
                .map(|row| row.try_get::<String>("", "d").expect("catalog line"))
                .filter(|line| line.starts_with(&format!("{prefix}_")))
                .map(|line| line.replace(prefix, "<prefix>")),
        );
    }
    lines.sort();
    assert!(!lines.is_empty(), "no tables found for {prefix}");
    lines
}

/// Acks every entity except [`BAD`], which it rejects into a dead letter.
struct RejectBad;

#[async_trait::async_trait]
impl LeasedMessageHandler for RejectBad {
    async fn handle(&self, msg: &OutboxMessage) -> MessageResult {
        if msg.payload == BAD {
            MessageResult::Reject("rejected by the test".into())
        } else {
            MessageResult::Ok
        }
    }
}

/// Enqueue a traced two-entity batch through the real pipeline and wait for
/// its completion: the enqueue writes `<prefix>_body.trace` and the trace row,
/// the rejected entity writes `<prefix>_dead_letters.trace`.
async fn enqueue_and_deliver(db: &Db, prefix: &str) {
    let handle = Outbox::builder(db.clone())
        .table_prefix(prefix)
        .unwrap()
        .processors(1)
        .maintenance(1, 1)
        .processor_tuning(
            WorkerTuning::processor()
                .idle_interval(Duration::from_millis(100))
                .retry_base(Duration::from_millis(50))
                .retry_max(Duration::from_millis(200)),
        )
        .queue("q", Partitions::of(1))
        .leased(RejectBad)
        .done()
        .start()
        .await
        .expect("start outbox");
    let outbox = Arc::clone(handle.outbox());
    let sub = outbox.subscribe("upgrade-1").unwrap();

    // One transaction, so on MySQL the trace insert and its `LAST_INSERT_ID()`
    // read share a connection.
    let enqueuer = Arc::clone(&outbox);
    let (db, result) = db
        .clone()
        .transaction(move |tx| {
            Box::pin(async move {
                let batch = Records::to("q")
                    .payload_type("test/plain")
                    .trace("upgrade-1")
                    .push(0, b"good".to_vec())
                    .push(0, BAD.to_vec())
                    .build()
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                enqueuer
                    .enqueue_batch(tx, batch)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))
            })
        })
        .await;
    result
        .unwrap_or_else(|e| panic!("enqueue into the {prefix} outbox failed: {e}"))
        .fire();

    let outcome = tokio::time::timeout(Duration::from_secs(30), sub.completion())
        .await
        .expect("the traced batch completes")
        .expect("completion is delivered");
    assert_eq!(outcome.entities, 2, "{prefix}: both entities accounted for");
    assert_eq!(outcome.failures, 1, "{prefix}: the rejected entity");

    let conn = db.conn().expect("conn");
    let dead = outbox
        .dead_letter_count(&conn, &DeadLetterFilter::default())
        .await
        .expect("dead letters readable");
    assert_eq!(dead, 1, "{prefix}: the rejected entity is a dead letter");

    handle.stop().await;
}

/// Leave `prefix` as toolkit-db 0.15 left it: `m001` recorded, no trace schema.
async fn create_pre_trace_family(db: &Db, raw: &DatabaseConnection, prefix: &str) {
    let first = run_migrations_for_testing(db, schema_migration_only(prefix))
        .await
        .expect("m001");
    assert_eq!(first.applied, 1, "{prefix}: m001 applied");
    strip_to_pre_trace_shape(raw, prefix).await;

    let old = schema_of(raw, prefix).await;
    assert!(
        !old.iter()
            .any(|l| l.contains("_trace") || l.contains(".trace ")),
        "{prefix}: the pre-0.16 shape still carries trace: {old:#?}"
    );
}

async fn upgrade_scenario(url: &str) {
    let db = connect_db(url, ConnectOpts::default())
        .await
        .expect("connect");
    let raw = Database::connect(url).await.expect("raw connect");
    let upgraded = [DEFAULT_PREFIX, CUSTOM_PREFIX];

    for prefix in upgraded {
        create_pre_trace_family(&db, &raw, prefix).await;
    }

    // (b) A fresh family: m001 creates everything, m002 must not fail on it.
    let fresh = run_migrations_for_testing(&db, migrations(FRESH_PREFIX))
        .await
        .expect("fresh migrations");
    assert_eq!(fresh.applied, 2, "fresh: m001 and m002 applied");
    let fresh_schema = schema_of(&raw, FRESH_PREFIX).await;

    // (a) The upgrade: the journal already has m001, so only m002 runs, and
    // it brings the family to exactly the fresh shape.
    for prefix in upgraded {
        let run = run_migrations_for_testing(&db, migrations(prefix))
            .await
            .unwrap_or_else(|e| panic!("{prefix}: upgrade failed: {e}"));
        assert_eq!(
            (run.applied, run.skipped),
            (1, 1),
            "{prefix}: m001 skipped, m002 applied"
        );
        assert_eq!(
            schema_of(&raw, prefix).await,
            fresh_schema,
            "{prefix}: the upgraded schema differs from a fresh one"
        );
    }

    // (c) Running every migration again is a no-op.
    let mut all = Vec::new();
    for prefix in [DEFAULT_PREFIX, CUSTOM_PREFIX, FRESH_PREFIX] {
        all.extend(migrations(prefix));
    }
    let again = run_migrations_for_testing(&db, all)
        .await
        .expect("second run");
    assert_eq!(
        (again.applied, again.skipped),
        (0, 6),
        "nothing left to apply"
    );
    for prefix in [DEFAULT_PREFIX, CUSTOM_PREFIX, FRESH_PREFIX] {
        assert_eq!(schema_of(&raw, prefix).await, fresh_schema, "{prefix}");
    }

    for prefix in upgraded {
        enqueue_and_deliver(&db, prefix).await;
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_outbox_schema_upgrades_in_place() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("outbox_upgrade.db");
    upgrade_scenario(&format!("sqlite://{}?mode=rwc", path.display())).await;
}

#[cfg(feature = "pg")]
#[tokio::test]
async fn postgres_outbox_schema_upgrades_in_place() -> anyhow::Result<()> {
    let db = common::bring_up_postgres().await?;
    upgrade_scenario(&db.url).await;
    Ok(())
}

#[cfg(feature = "mysql")]
#[tokio::test]
async fn mysql_outbox_schema_upgrades_in_place() -> anyhow::Result<()> {
    let db = common::bring_up_mysql().await?;
    upgrade_scenario(&db.url).await;
    Ok(())
}
