//! Minimal, ordered SQLite schema migrations.
//!
//! The first version of every harness database predates this module and was
//! created with idempotent `CREATE TABLE IF NOT EXISTS` statements. Those
//! statements are still the baseline: running them is safe on both a fresh
//! database and an existing one, and `baseline` records version 1 exactly once.
//!
//! From version 2 onward a migration runs inside `BEGIN IMMEDIATE` together
//! with its own version row, so a crash cannot leave the schema half-applied
//! without the matching bookkeeping. Statements and column additions must
//! remain idempotent anyway: SQLite cannot roll back every DDL edge case on
//! every platform, and a re-run after a crash must be harmless.

use chrono::Utc;
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::StoreError;

/// The version recorded by [`baseline`].
pub const BASELINE_VERSION: i64 = 1;

/// One ordered schema change, applied only when its version is not recorded.
/// Treat an applied entry as immutable: bump the version instead of editing
/// it, because an existing database will never re-run that version.
#[derive(Debug)]
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    /// One SQL statement per entry; SQLx prepares a single statement at a time.
    pub statements: &'static [&'static str],
    pub columns: &'static [Column],
}

/// A column that may already exist on an older database.
#[derive(Debug)]
pub struct Column {
    pub table: &'static str,
    pub column: &'static str,
    pub declaration: &'static str,
}

/// Creates the version table and records the idempotent baseline schema.
///
/// Call this after the legacy `CREATE TABLE IF NOT EXISTS` block has run.
/// Re-running is safe: the version row is inserted with `OR IGNORE`.
pub async fn baseline(pool: &SqlitePool, namespace: &str) -> Result<(), StoreError> {
    ensure_table(pool).await?;
    sqlx::query(
        "INSERT OR IGNORE INTO _stark_migrations (namespace, version, name, applied_at) VALUES (?, ?, 'baseline', ?)",
    )
    .bind(namespace)
    .bind(BASELINE_VERSION)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// Applies every migration newer than the recorded baseline exactly once.
pub async fn apply(
    pool: &SqlitePool,
    namespace: &str,
    migrations: &[Migration],
) -> Result<(), StoreError> {
    ensure_table(pool).await?;
    for migration in migrations
        .iter()
        .filter(|migration| migration.version > BASELINE_VERSION)
    {
        let applied: Option<i64> = sqlx::query_scalar(
            "SELECT version FROM _stark_migrations WHERE namespace = ? AND version = ?",
        )
        .bind(namespace)
        .bind(migration.version)
        .fetch_optional(pool)
        .await?;
        if applied.is_some() {
            continue;
        }

        let mut tx = crate::begin_immediate(pool).await?;
        // Another process may have applied this version while we waited for
        // the write lock. Re-check under the lock before running DDL.
        let applied_now: Option<i64> = sqlx::query_scalar(
            "SELECT version FROM _stark_migrations WHERE namespace = ? AND version = ?",
        )
        .bind(namespace)
        .bind(migration.version)
        .fetch_optional(&mut *tx)
        .await?;
        if applied_now.is_some() {
            let _ = tx.rollback().await;
            continue;
        }
        for statement in migration.statements {
            sqlx::query(*statement).execute(&mut *tx).await?;
        }
        for column in migration.columns {
            ensure_column_tx(&mut tx, column).await?;
        }
        sqlx::query(
            "INSERT INTO _stark_migrations (namespace, version, name, applied_at) VALUES (?, ?, ?, ?)",
        )
        .bind(namespace)
        .bind(migration.version)
        .bind(migration.name)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }
    Ok(())
}

/// Adds a column only when `PRAGMA table_info` says it is absent.
///
/// This replaces blind `ALTER TABLE ... ADD COLUMN` calls whose duplicate-column
/// errors were previously discarded. Table and column names are internal
/// constants; identifiers are validated so a future call cannot inject SQL.
pub async fn ensure_column(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    declaration: &str,
) -> Result<(), StoreError> {
    if !valid_identifier(table) || !valid_identifier(column) {
        return Err(StoreError::Invalid(format!(
            "invalid column migration identifier: {table}.{column}"
        )));
    }
    let exists: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?"
    )))
    .bind(column)
    .fetch_optional(pool)
    .await?;
    if exists.is_none()
        && let Err(error) = sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
        )))
        .execute(pool)
        .await
        && !duplicate_column(&error)
    {
        return Err(error.into());
    }
    Ok(())
}

fn duplicate_column(error: &sqlx::Error) -> bool {
    error.as_database_error().is_some_and(|error| {
        error
            .message()
            .to_ascii_lowercase()
            .contains("duplicate column")
    })
}

async fn ensure_column_tx(
    tx: &mut Transaction<'static, Sqlite>,
    column: &Column,
) -> Result<(), StoreError> {
    if !valid_identifier(column.table) || !valid_identifier(column.column) {
        return Err(StoreError::Invalid(format!(
            "invalid column migration identifier: {}.{}",
            column.table, column.column
        )));
    }
    let exists: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT 1 FROM pragma_table_info('{}') WHERE name = ?",
        column.table
    )))
    .bind(column.column)
    .fetch_optional(&mut **tx)
    .await?;
    if exists.is_none()
        && let Err(error) = sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE {} ADD COLUMN {} {}",
            column.table, column.column, column.declaration
        )))
        .execute(&mut **tx)
        .await
        && !duplicate_column(&error)
    {
        return Err(error.into());
    }
    Ok(())
}

async fn ensure_table(pool: &SqlitePool) -> Result<(), StoreError> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS _stark_migrations (
            namespace TEXT NOT NULL,
            version INTEGER NOT NULL,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL,
            PRIMARY KEY (namespace, version)
        )
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool(dir: &tempfile::TempDir) -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(crate::sqlite_options(&dir.path().join("test.db"), true))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn baseline_is_recorded_once() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(&dir).await;
        baseline(&pool, "test").await.unwrap();
        baseline(&pool, "test").await.unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _stark_migrations WHERE namespace = 'test'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        pool.close().await;
    }

    #[tokio::test]
    async fn versioned_migration_applies_once() {
        const MIGRATIONS: &[Migration] = &[Migration {
            version: 2,
            name: "probe_table",
            statements: &["CREATE TABLE probe (id INTEGER PRIMARY KEY)"],
            columns: &[],
        }];
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(&dir).await;
        baseline(&pool, "test").await.unwrap();
        apply(&pool, "test", MIGRATIONS).await.unwrap();
        apply(&pool, "test", MIGRATIONS).await.unwrap();
        let applied: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM _stark_migrations WHERE namespace = 'test' AND version = 2",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(applied, 1);
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM probe")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0);
        pool.close().await;
    }

    #[tokio::test]
    async fn ensure_column_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(&dir).await;
        sqlx::query("CREATE TABLE probe (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        ensure_column(&pool, "probe", "note", "TEXT DEFAULT ''")
            .await
            .unwrap();
        ensure_column(&pool, "probe", "note", "TEXT DEFAULT ''")
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('probe') WHERE name = 'note'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
        pool.close().await;
    }

    #[tokio::test]
    async fn invalid_identifiers_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(&dir).await;
        assert!(
            ensure_column(&pool, "probe; DROP TABLE probe", "note", "TEXT")
                .await
                .is_err()
        );
        assert!(
            ensure_column(&pool, "probe", "note; DROP TABLE probe", "TEXT")
                .await
                .is_err()
        );
        pool.close().await;
    }
}
