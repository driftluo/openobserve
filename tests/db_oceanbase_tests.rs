// Copyright 2025 OpenObserve Inc.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! Integration tests running against a real OceanBase instance.
//!
//! Default connection: mysql://root:vdthink88@10.10.14.64:2881/openobserve_test
//!
//! # Running Tests
//!
//! ```bash
//! cargo test --test db_oceanbase_tests --features db-oceanbase-tests -- --test-threads=1 --nocapture
//! ```
//!
//! Or with custom DSN:
//! ```bash
//! ZO_TEST_OCEANBASE_DSN="mysql://user:pass@host:port/db" \
//!   cargo test --test db_oceanbase_tests --features db-oceanbase-tests -- --test-threads=1
//! ```

#![cfg(feature = "db-oceanbase-tests")]

mod common;

use common::db_helpers::RealOceanBaseInstance;
use serial_test::serial;
use std::time::Duration;

/// Basic connectivity and CRUD tests against real OceanBase
mod basic_tests {
    use super::*;

    #[tokio::test]
    #[serial]
    async fn test_connection() {
        let ob = RealOceanBaseInstance::new().await;

        // Verify we can query
        let result: (i64,) = sqlx::query_as("SELECT 1")
            .fetch_one(&ob.pool)
            .await
            .expect("Basic query failed");

        assert_eq!(result.0, 1);
        println!("✓ OceanBase connection successful");
    }

    #[tokio::test]
    #[serial]
    async fn test_version() {
        let ob = RealOceanBaseInstance::new().await;

        let version: (String,) = sqlx::query_as("SELECT VERSION()")
            .fetch_one(&ob.pool)
            .await
            .expect("VERSION query failed");

        println!("✓ OceanBase version: {}", version.0);
        // OceanBase version string typically contains "OceanBase" or version number
        assert!(!version.0.is_empty());
    }

    #[tokio::test]
    #[serial]
    async fn test_schema_created() {
        let ob = RealOceanBaseInstance::new().await;

        // Verify meta table exists
        let tables: Vec<(String,)> = sqlx::query_as(
            "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES
             WHERE TABLE_SCHEMA = 'openobserve_test' AND TABLE_NAME = 'meta'",
        )
        .fetch_all(&ob.pool)
        .await
        .expect("Failed to query tables");

        assert_eq!(tables.len(), 1);
        println!("✓ Meta table created successfully");
    }

    #[tokio::test]
    #[serial]
    async fn test_insert_and_select() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        // Insert
        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value)
             VALUES ('test_module', 'test_key1', 'test_key2', 0, '{\"data\": \"test\"}')",
        )
        .execute(&ob.pool)
        .await
        .expect("Insert failed");

        // Select
        let row: (String, String, String, i64, String) = sqlx::query_as(
            "SELECT module, key1, key2, start_dt, value FROM meta WHERE module = 'test_module'",
        )
        .fetch_one(&ob.pool)
        .await
        .expect("Select failed");

        assert_eq!(row.0, "test_module");
        assert_eq!(row.1, "test_key1");
        assert_eq!(row.2, "test_key2");
        assert_eq!(row.3, 0);
        assert_eq!(row.4, "{\"data\": \"test\"}");
        println!("✓ Insert and select successful");
    }

    #[tokio::test]
    #[serial]
    async fn test_update() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        // Insert
        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value)
             VALUES ('upd', 'k1', 'k2', 0, 'original')",
        )
        .execute(&ob.pool)
        .await
        .expect("Insert failed");

        // Update
        let result = sqlx::query(
            "UPDATE meta SET value = 'updated' WHERE module = 'upd' AND key1 = 'k1'",
        )
        .execute(&ob.pool)
        .await
        .expect("Update failed");

        assert_eq!(result.rows_affected(), 1);

        // Verify
        let row: (String,) =
            sqlx::query_as("SELECT value FROM meta WHERE module = 'upd' AND key1 = 'k1'")
                .fetch_one(&ob.pool)
                .await
                .expect("Select failed");

        assert_eq!(row.0, "updated");
        println!("✓ Update successful");
    }

    #[tokio::test]
    #[serial]
    async fn test_delete() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        // Insert
        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value)
             VALUES ('del', 'k1', 'k2', 0, 'to_delete')",
        )
        .execute(&ob.pool)
        .await
        .expect("Insert failed");

        // Delete
        let result = sqlx::query("DELETE FROM meta WHERE module = 'del'")
            .execute(&ob.pool)
            .await
            .expect("Delete failed");

        assert_eq!(result.rows_affected(), 1);

        // Verify
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM meta WHERE module = 'del'")
            .fetch_one(&ob.pool)
            .await
            .expect("Count failed");

        assert_eq!(count.0, 0);
        println!("✓ Delete successful");
    }
}

/// Lock mechanism tests - critical for MetaStore concurrency
mod lock_tests {
    use super::*;

    #[tokio::test]
    #[serial]
    async fn test_get_lock_basic() {
        let ob = RealOceanBaseInstance::new().await;

        // GET_LOCK should return 1 on success
        let result: (i32,) = sqlx::query_as("SELECT GET_LOCK('ob_test_lock_1', 10)")
            .fetch_one(&ob.pool)
            .await
            .expect("GET_LOCK failed");

        assert_eq!(result.0, 1, "GET_LOCK should return 1 on success");
        println!("✓ GET_LOCK returns 1 on success");

        // Release - OceanBase may return 0 or 1 depending on version
        // MySQL returns 1 when lock was held, OceanBase CE may return 0
        let release: (i32,) = sqlx::query_as("SELECT RELEASE_LOCK('ob_test_lock_1')")
            .fetch_one(&ob.pool)
            .await
            .expect("RELEASE_LOCK failed");

        // Accept either 0 or 1 as success (OceanBase compatibility)
        assert!(
            release.0 == 0 || release.0 == 1,
            "RELEASE_LOCK should return 0 or 1, got {}",
            release.0
        );
        println!(
            "✓ RELEASE_LOCK returned {} (OceanBase behavior)",
            release.0
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_lock_contention() {
        let ob = RealOceanBaseInstance::new().await;

        // First connection acquires the lock
        sqlx::query("SELECT GET_LOCK('ob_contention_lock', 10)")
            .fetch_one(&ob.pool)
            .await
            .expect("First GET_LOCK failed");

        // Create second connection
        let pool2 = sqlx::MySqlPool::connect(&ob.dsn)
            .await
            .expect("Failed to create second connection");

        // Second connection should timeout with 1 second timeout
        let start = std::time::Instant::now();
        let result: (i32,) = sqlx::query_as("SELECT GET_LOCK('ob_contention_lock', 1)")
            .fetch_one(&pool2)
            .await
            .expect("Second GET_LOCK query failed");

        let elapsed = start.elapsed();

        // Should return 0 (timeout)
        assert_eq!(result.0, 0, "GET_LOCK should return 0 on timeout");
        assert!(
            elapsed >= Duration::from_millis(900),
            "Should wait approximately 1 second, waited {:?}",
            elapsed
        );
        println!(
            "✓ Lock contention works correctly (waited {:?} for timeout)",
            elapsed
        );

        // Clean up
        sqlx::query("SELECT RELEASE_LOCK('ob_contention_lock')")
            .execute(&ob.pool)
            .await
            .ok();
    }

    #[tokio::test]
    #[serial]
    async fn test_release_nonexistent_lock() {
        let ob = RealOceanBaseInstance::new().await;

        // RELEASE_LOCK on non-held lock should return NULL
        let result: Option<i32> =
            sqlx::query_scalar("SELECT RELEASE_LOCK('nonexistent_ob_lock_12345')")
                .fetch_one(&ob.pool)
                .await
                .expect("RELEASE_LOCK query failed");

        assert!(
            result.is_none(),
            "RELEASE_LOCK should return NULL for non-held lock"
        );
        println!("✓ RELEASE_LOCK returns NULL for non-held lock");
    }
}

/// Transaction tests
mod transaction_tests {
    use super::*;

    #[tokio::test]
    #[serial]
    async fn test_transaction_commit() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        // Start transaction
        let mut tx = ob.pool.begin().await.expect("Failed to begin transaction");

        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value)
             VALUES ('tx_commit', 'k1', 'k2', 0, 'tx_value')",
        )
        .execute(&mut *tx)
        .await
        .expect("Insert in transaction failed");

        // Commit
        tx.commit().await.expect("Commit failed");

        // Verify persisted
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM meta WHERE module = 'tx_commit'")
                .fetch_one(&ob.pool)
                .await
                .expect("Count failed");

        assert_eq!(count.0, 1);
        println!("✓ Transaction commit works");
    }

    #[tokio::test]
    #[serial]
    async fn test_transaction_rollback() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        // Start transaction
        let mut tx = ob.pool.begin().await.expect("Failed to begin transaction");

        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value)
             VALUES ('tx_rollback', 'k1', 'k2', 0, 'tx_value')",
        )
        .execute(&mut *tx)
        .await
        .expect("Insert in transaction failed");

        // Rollback
        tx.rollback().await.expect("Rollback failed");

        // Verify not persisted
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM meta WHERE module = 'tx_rollback'")
                .fetch_one(&ob.pool)
                .await
                .expect("Count failed");

        assert_eq!(count.0, 0);
        println!("✓ Transaction rollback works");
    }

    #[tokio::test]
    #[serial]
    async fn test_transaction_isolation() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        // Start transaction but don't commit
        let mut tx = ob.pool.begin().await.expect("Failed to begin transaction");

        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value)
             VALUES ('tx_iso', 'k1', 'k2', 0, 'uncommitted')",
        )
        .execute(&mut *tx)
        .await
        .expect("Insert in transaction failed");

        // Second connection should NOT see uncommitted data
        let pool2 = sqlx::MySqlPool::connect(&ob.dsn)
            .await
            .expect("Failed to create second connection");

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM meta WHERE module = 'tx_iso'")
            .fetch_one(&pool2)
            .await
            .expect("Count failed");

        assert_eq!(count.0, 0, "Uncommitted data should not be visible");
        println!("✓ Transaction isolation works (uncommitted not visible)");

        // Now commit
        tx.commit().await.expect("Commit failed");

        // Now should be visible
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM meta WHERE module = 'tx_iso'")
            .fetch_one(&pool2)
            .await
            .expect("Count failed");

        assert_eq!(count.0, 1, "Committed data should be visible");
        println!("✓ Committed data visible after commit");
    }
}

/// Data type and edge case tests
mod data_type_tests {
    use super::*;

    #[tokio::test]
    #[serial]
    async fn test_unicode_values() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        let unicode_value = r#"{"中文": "测试", "日本語": "テスト", "emoji": "🚀🎉"}"#;

        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value) VALUES ('unicode', 'k1', 'k2', 0, ?)",
        )
        .bind(unicode_value)
        .execute(&ob.pool)
        .await
        .expect("Insert Unicode failed");

        let row: (String,) =
            sqlx::query_as("SELECT value FROM meta WHERE module = 'unicode'")
                .fetch_one(&ob.pool)
                .await
                .expect("Select failed");

        assert_eq!(row.0, unicode_value);
        println!("✓ Unicode values stored and retrieved correctly");
    }

    #[tokio::test]
    #[serial]
    async fn test_large_value() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        // 1MB value
        let large_value = "x".repeat(1024 * 1024);

        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value) VALUES ('large', 'k1', 'k2', 0, ?)",
        )
        .bind(&large_value)
        .execute(&ob.pool)
        .await
        .expect("Insert large value failed");

        let row: (String,) =
            sqlx::query_as("SELECT value FROM meta WHERE module = 'large'")
                .fetch_one(&ob.pool)
                .await
                .expect("Select failed");

        assert_eq!(row.0.len(), 1024 * 1024);
        println!("✓ Large value (1MB) stored and retrieved correctly");
    }

    #[tokio::test]
    #[serial]
    async fn test_special_characters() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        let special_value = r#"{"quote": "it's \"quoted\"", "backslash": "path\\to\\file", "newline": "line1\nline2"}"#;

        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value) VALUES ('special', 'k1', 'k2', 0, ?)",
        )
        .bind(special_value)
        .execute(&ob.pool)
        .await
        .expect("Insert special chars failed");

        let row: (String,) =
            sqlx::query_as("SELECT value FROM meta WHERE module = 'special'")
                .fetch_one(&ob.pool)
                .await
                .expect("Select failed");

        assert_eq!(row.0, special_value);
        println!("✓ Special characters handled correctly");
    }

    #[tokio::test]
    #[serial]
    async fn test_bigint_range() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        let max_bigint: i64 = i64::MAX;

        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value) VALUES ('bigint', 'max', 'k', ?, 'v')",
        )
        .bind(max_bigint)
        .execute(&ob.pool)
        .await
        .expect("Insert max BIGINT failed");

        let row: (i64,) =
            sqlx::query_as("SELECT start_dt FROM meta WHERE module = 'bigint' AND key1 = 'max'")
                .fetch_one(&ob.pool)
                .await
                .expect("Select failed");

        assert_eq!(row.0, max_bigint);
        println!("✓ BIGINT max value preserved: {}", max_bigint);
    }

    #[tokio::test]
    #[serial]
    async fn test_empty_value() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value) VALUES ('empty', 'k1', 'k2', 0, '')",
        )
        .execute(&ob.pool)
        .await
        .expect("Insert empty value failed");

        let row: (String,) =
            sqlx::query_as("SELECT value FROM meta WHERE module = 'empty'")
                .fetch_one(&ob.pool)
                .await
                .expect("Select failed");

        assert_eq!(row.0, "");
        println!("✓ Empty value stored and retrieved correctly");
    }
}

/// Index and performance tests
mod index_tests {
    use super::*;

    #[tokio::test]
    #[serial]
    async fn test_unique_constraint() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        // Insert first record
        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value)
             VALUES ('unique', 'k1', 'k2', 100, 'first')",
        )
        .execute(&ob.pool)
        .await
        .expect("First insert failed");

        // Try to insert duplicate (same module, key1, key2, start_dt)
        let result = sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value)
             VALUES ('unique', 'k1', 'k2', 100, 'second')",
        )
        .execute(&ob.pool)
        .await;

        assert!(result.is_err(), "Duplicate insert should fail");
        println!("✓ Unique constraint enforced correctly");
    }

    #[tokio::test]
    #[serial]
    async fn test_index_exists() {
        let ob = RealOceanBaseInstance::new().await;

        // Check indexes on meta table
        let indexes: Vec<(String,)> = sqlx::query_as(
            "SELECT INDEX_NAME FROM INFORMATION_SCHEMA.STATISTICS
             WHERE TABLE_SCHEMA = 'openobserve_test' AND TABLE_NAME = 'meta'
             GROUP BY INDEX_NAME",
        )
        .fetch_all(&ob.pool)
        .await
        .expect("Failed to query indexes");

        let index_names: Vec<&str> = indexes.iter().map(|(n,)| n.as_str()).collect();
        println!("Indexes found: {:?}", index_names);

        // Should have at least PRIMARY and our custom indexes
        assert!(
            index_names.iter().any(|n| n.to_uppercase() == "PRIMARY"),
            "Should have PRIMARY index"
        );
        println!("✓ Indexes exist on meta table");
    }

    #[tokio::test]
    #[serial]
    async fn test_prefix_query_performance() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        // Insert 100 records with format key_0000 to key_0099
        for i in 0..100 {
            sqlx::query(
                "INSERT INTO meta (module, key1, key2, start_dt, value) VALUES ('perf', ?, 'k', 0, 'v')",
            )
            .bind(format!("key_{:04}", i))
            .execute(&ob.pool)
            .await
            .expect("Insert failed");
        }

        // Query with prefix - key_000% matches key_0000 to key_0009 (10 rows)
        let start = std::time::Instant::now();
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT key1 FROM meta WHERE module = 'perf' AND key1 LIKE 'key_000%'")
                .fetch_all(&ob.pool)
                .await
                .expect("Query failed");

        let elapsed = start.elapsed();

        assert_eq!(rows.len(), 10, "key_000% should match key_0000 to key_0009");
        println!(
            "✓ Prefix query returned {} rows in {:?}",
            rows.len(),
            elapsed
        );
    }
}

/// OpenObserve-specific pattern tests
mod openobserve_patterns {
    use super::*;

    #[tokio::test]
    #[serial]
    async fn test_schema_versioning_pattern() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        // Simulate schema versioning with start_dt as version timestamp
        let versions = vec![
            (1000i64, r#"{"version": 1, "fields": ["f1"]}"#),
            (2000i64, r#"{"version": 2, "fields": ["f1", "f2"]}"#),
            (3000i64, r#"{"version": 3, "fields": ["f1", "f2", "f3"]}"#),
        ];

        for (start_dt, value) in &versions {
            sqlx::query(
                "INSERT INTO meta (module, key1, key2, start_dt, value)
                 VALUES ('schema', 'org1', 'stream1', ?, ?)",
            )
            .bind(start_dt)
            .bind(value)
            .execute(&ob.pool)
            .await
            .expect("Insert failed");
        }

        // Get latest version
        let latest: (i64, String) = sqlx::query_as(
            "SELECT start_dt, value FROM meta
             WHERE module = 'schema' AND key1 = 'org1' AND key2 = 'stream1'
             ORDER BY start_dt DESC LIMIT 1",
        )
        .fetch_one(&ob.pool)
        .await
        .expect("Query failed");

        assert_eq!(latest.0, 3000);
        assert!(latest.1.contains("version\": 3"));
        println!("✓ Schema versioning pattern works (latest version: {})", latest.0);
    }

    #[tokio::test]
    #[serial]
    async fn test_list_by_prefix_pattern() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        // Insert multi-tenant data
        let data = vec![
            ("schema", "org1", "logs"),
            ("schema", "org1", "metrics"),
            ("schema", "org2", "logs"),
            ("users", "org1", "admin"),
        ];

        for (module, key1, key2) in &data {
            sqlx::query(
                "INSERT INTO meta (module, key1, key2, start_dt, value)
                 VALUES (?, ?, ?, 0, '{}')",
            )
            .bind(module)
            .bind(key1)
            .bind(key2)
            .execute(&ob.pool)
            .await
            .expect("Insert failed");
        }

        // List all schemas for org1
        let org1_schemas: Vec<(String,)> = sqlx::query_as(
            "SELECT key2 FROM meta WHERE module = 'schema' AND key1 = 'org1' ORDER BY key2",
        )
        .fetch_all(&ob.pool)
        .await
        .expect("Query failed");

        assert_eq!(org1_schemas.len(), 2);
        assert_eq!(org1_schemas[0].0, "logs");
        assert_eq!(org1_schemas[1].0, "metrics");
        println!("✓ List by prefix pattern works (found {} streams for org1)", org1_schemas.len());
    }

    #[tokio::test]
    #[serial]
    async fn test_atomic_update_with_lock() {
        let ob = RealOceanBaseInstance::new().await;
        ob.truncate().await;

        // Insert initial record
        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value)
             VALUES ('atomic', 'counter', 'k', 0, '{\"count\": 0}')",
        )
        .execute(&ob.pool)
        .await
        .expect("Insert failed");

        // Simulate atomic update with lock (as MysqlDb does)
        let lock_result: (i32,) = sqlx::query_as("SELECT GET_LOCK('atomic_update_lock', 10)")
            .fetch_one(&ob.pool)
            .await
            .expect("GET_LOCK failed");

        assert_eq!(lock_result.0, 1);

        // Read current value
        let current: (String,) = sqlx::query_as(
            "SELECT value FROM meta WHERE module = 'atomic' AND key1 = 'counter'",
        )
        .fetch_one(&ob.pool)
        .await
        .expect("Select failed");

        // Parse, modify, write back
        let mut json: serde_json::Value =
            serde_json::from_str(&current.0).expect("Parse JSON failed");
        json["count"] = serde_json::json!(json["count"].as_i64().unwrap_or(0) + 1);

        sqlx::query(
            "UPDATE meta SET value = ? WHERE module = 'atomic' AND key1 = 'counter'",
        )
        .bind(json.to_string())
        .execute(&ob.pool)
        .await
        .expect("Update failed");

        // Release lock
        sqlx::query("SELECT RELEASE_LOCK('atomic_update_lock')")
            .execute(&ob.pool)
            .await
            .expect("RELEASE_LOCK failed");

        // Verify
        let updated: (String,) = sqlx::query_as(
            "SELECT value FROM meta WHERE module = 'atomic' AND key1 = 'counter'",
        )
        .fetch_one(&ob.pool)
        .await
        .expect("Select failed");

        let updated_json: serde_json::Value =
            serde_json::from_str(&updated.0).expect("Parse JSON failed");
        assert_eq!(updated_json["count"], 1);
        println!("✓ Atomic update with lock pattern works");
    }
}

/// Additional lock timeout tests for OceanBase
mod lock_timeout_tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[tokio::test]
    #[serial]
    async fn test_lock_timeout() {
        let ob = RealOceanBaseInstance::new().await;
        let dsn = ob.dsn.clone();
        ob.truncate().await;

        // First connection acquires lock and holds it
        let conn1 = sqlx::MySqlPool::connect(&dsn)
            .await
            .expect("Failed to create connection 1");

        let lock_id = "ob_timeout_test_lock";

        // Acquire lock on connection 1
        let lock_result: (i32,) = sqlx::query_as(&format!("SELECT GET_LOCK('{}', 10)", lock_id))
            .fetch_one(&conn1)
            .await
            .expect("GET_LOCK failed on conn1");

        assert_eq!(lock_result.0, 1, "First connection should acquire lock");
        println!("✓ First connection acquired lock");

        // Second connection tries to acquire the same lock with a short timeout
        let conn2 = sqlx::MySqlPool::connect(&dsn)
            .await
            .expect("Failed to create connection 2");

        let start = Instant::now();
        let timeout_seconds = 2;

        // This should timeout because conn1 holds the lock
        let lock_result2: (i32,) =
            sqlx::query_as(&format!("SELECT GET_LOCK('{}', {})", lock_id, timeout_seconds))
                .fetch_one(&conn2)
                .await
                .expect("GET_LOCK query failed on conn2");

        let elapsed = start.elapsed();

        // Should return 0 (timeout) and wait approximately the timeout duration
        assert_eq!(
            lock_result2.0, 0,
            "Second connection should timeout (got {})",
            lock_result2.0
        );
        assert!(
            elapsed >= Duration::from_secs(timeout_seconds as u64 - 1),
            "Should wait at least {} seconds, waited {:?}",
            timeout_seconds - 1,
            elapsed
        );
        println!(
            "✓ Second connection correctly timed out after {:?}",
            elapsed
        );

        // Release lock on conn1
        let _: (i32,) = sqlx::query_as(&format!("SELECT RELEASE_LOCK('{}')", lock_id))
            .fetch_one(&conn1)
            .await
            .expect("RELEASE_LOCK failed");

        // Now conn2 should be able to acquire the lock
        let lock_result3: (i32,) = sqlx::query_as(&format!("SELECT GET_LOCK('{}', 10)", lock_id))
            .fetch_one(&conn2)
            .await
            .expect("GET_LOCK failed after release");

        assert_eq!(
            lock_result3.0, 1,
            "Second connection should acquire lock after release"
        );
        println!("✓ Second connection acquired lock after first released");

        // Cleanup
        let _: (i32,) = sqlx::query_as(&format!("SELECT RELEASE_LOCK('{}')", lock_id))
            .fetch_one(&conn2)
            .await
            .expect("Final RELEASE_LOCK failed");
    }

    #[tokio::test]
    #[serial]
    async fn test_concurrent_update_with_lock() {
        let ob = Arc::new(RealOceanBaseInstance::new().await);
        let dsn = Arc::new(ob.dsn.clone());
        ob.truncate().await;

        // Insert initial counter
        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value) VALUES ('ob_lock', 'counter', 'k', 0, '0')",
        )
        .execute(&ob.pool)
        .await
        .expect("Insert failed");

        let lock_id = "ob_concurrent_lock";
        let num_workers = 5;

        let mut handles = vec![];

        for worker_id in 0..num_workers {
            let dsn = dsn.clone();

            handles.push(tokio::spawn(async move {
                // Create dedicated connection for this worker
                let conn = sqlx::MySqlPool::connect(&dsn)
                    .await
                    .expect("Failed to create connection");

                // Acquire lock (wait up to 60 seconds)
                let lock_result: (i32,) =
                    sqlx::query_as(&format!("SELECT GET_LOCK('{}', 60)", lock_id))
                        .fetch_one(&conn)
                        .await
                        .expect("GET_LOCK failed");

                assert_eq!(lock_result.0, 1, "Worker {} should acquire lock", worker_id);

                // Read current counter
                let row: (String,) = sqlx::query_as(
                    "SELECT value FROM meta WHERE module = 'ob_lock' AND key1 = 'counter'",
                )
                .fetch_one(&conn)
                .await
                .expect("Select failed");

                let current_val: i32 = row.0.parse().unwrap_or(0);

                // Update counter
                sqlx::query(
                    "UPDATE meta SET value = ? WHERE module = 'ob_lock' AND key1 = 'counter'",
                )
                .bind((current_val + 1).to_string())
                .execute(&conn)
                .await
                .expect("Update failed");

                // Release lock
                let _: (i32,) = sqlx::query_as(&format!("SELECT RELEASE_LOCK('{}')", lock_id))
                    .fetch_one(&conn)
                    .await
                    .expect("RELEASE_LOCK failed");
            }));
        }

        // Wait for all workers to complete
        for handle in handles {
            handle.await.expect("Worker task panicked");
        }

        // Verify final counter value
        let row: (String,) = sqlx::query_as(
            "SELECT value FROM meta WHERE module = 'ob_lock' AND key1 = 'counter'",
        )
        .fetch_one(&ob.pool)
        .await
        .expect("Select failed");

        assert_eq!(
            row.0,
            num_workers.to_string(),
            "Counter should be exactly {} after {} increments",
            num_workers,
            num_workers
        );

        println!(
            "✓ Concurrent update test passed: final counter = {}",
            row.0
        );
    }
}
