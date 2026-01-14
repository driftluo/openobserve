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

//! MySQL integration tests for MysqlDb trait implementation.
//!
//! These tests verify the MysqlDb implementation of the Db trait
//! using real database connections.
//!
//! Default connection: mysql://root:mysqlroot@10.10.14.61:3306/openobserve_test
//!
//! # Running Tests
//!
//! ```bash
//! cargo test --test db_mysql_tests --features db-mysql-tests -- --test-threads=1 --nocapture
//! ```
//!
//! Or with custom DSN:
//! ```bash
//! ZO_TEST_MYSQL_DSN="mysql://user:pass@host:port/db" \
//!   cargo test --test db_mysql_tests --features db-mysql-tests -- --test-threads=1
//! ```

#![cfg(feature = "db-mysql-tests")]

mod common;

use common::db_helpers::RealMySqlInstance;
use serial_test::serial;

/// Tests for schema validation (index existence).
/// These tests verify that the database schema is correct.
mod schema_tests {
    use super::*;

    #[tokio::test]
    #[serial]
    async fn test_required_indexes_exist() {
        let container = RealMySqlInstance::new().await;

        // Query for indexes on meta table
        let indexes: Vec<(String,)> = sqlx::query_as(
            "SELECT INDEX_NAME FROM INFORMATION_SCHEMA.STATISTICS
             WHERE TABLE_SCHEMA = 'openobserve_test' AND TABLE_NAME = 'meta'
             GROUP BY INDEX_NAME",
        )
        .fetch_all(&container.pool)
        .await
        .expect("Query failed");

        let index_names: Vec<&str> = indexes.iter().map(|(n,)| n.as_str()).collect();

        // Verify expected indexes exist
        assert!(
            index_names.iter().any(|n| n.contains("module")),
            "Module index should exist. Found: {:?}",
            index_names
        );
        assert!(
            index_names.iter().any(|n| n.contains("key1")),
            "Key1 index should exist. Found: {:?}",
            index_names
        );
        println!("✓ Required indexes exist: {:?}", index_names);
    }

    #[tokio::test]
    #[serial]
    async fn test_unique_constraint_on_composite_key() {
        let container = RealMySqlInstance::new().await;

        // Insert first record
        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value) VALUES (?, ?, ?, ?, ?)",
        )
        .bind("unique_test")
        .bind("key1")
        .bind("key2")
        .bind(100i64)
        .bind("value1")
        .execute(&container.pool)
        .await
        .expect("Insert failed");

        // Different start_dt should be allowed (unique constraint is on module+key1+key2+start_dt)
        sqlx::query(
            "INSERT INTO meta (module, key1, key2, start_dt, value) VALUES (?, ?, ?, ?, ?)",
        )
        .bind("unique_test")
        .bind("key1")
        .bind("key2")
        .bind(200i64)
        .bind("value2")
        .execute(&container.pool)
        .await
        .expect("Insert with different start_dt should succeed");

        // Verify both records exist
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM meta WHERE module = 'unique_test'",
        )
        .fetch_one(&container.pool)
        .await
        .expect("Count failed");

        assert_eq!(count.0, 2);
        println!("✓ Unique constraint allows different start_dt values");
    }
}

/// Tests for MysqlDb trait methods using the actual Db trait implementation.
/// These tests verify the behavior of the MysqlDb struct's Db trait implementation.
///
/// IMPORTANT: Due to sqlx connection pool being tied to tokio runtime,
/// all MysqlDb trait tests use a global TEST_RUNTIME for consistent pool behavior.
mod mysqldb_trait_tests {
    use super::*;
    use bytes::Bytes;
    use common::db_helpers::init_config_for_mysql_tests;
    use infra::db::{mysql::MysqlDb, Db};
    use once_cell::sync::Lazy;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    /// Global tokio runtime for all MysqlDb trait tests.
    /// sqlx connection pools are tied to the runtime they were created in,
    /// so we need a single shared runtime for all tests.
    static TEST_RUNTIME: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Failed to create test runtime")
    });

    /// Global test counter for unique key prefixes.
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Helper to set up test environment for MysqlDb trait tests.
    async fn setup_trait_test() -> (MysqlDb, String) {
        use std::sync::atomic::{AtomicBool, Ordering};
        static DB_AND_SCHEMA_CREATED: AtomicBool = AtomicBool::new(false);

        init_config_for_mysql_tests();

        let test_id = TEST_COUNTER.fetch_add(1, AtomicOrdering::SeqCst);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let unique_prefix = format!("t{}_{}", timestamp, test_id);

        if !DB_AND_SCHEMA_CREATED.load(Ordering::SeqCst) {
            let dsn = std::env::var("ZO_TEST_MYSQL_DSN")
                .unwrap_or_else(|_| "mysql://root:mysqlroot@10.10.14.61:3306/openobserve_test".to_string());
            let base_dsn = dsn.rsplit_once('/').map(|(base, _)| base).unwrap_or(&dsn);
            let db_name = dsn.rsplit_once('/').map(|(_, db)| db).unwrap_or("openobserve_test");

            use sqlx::Connection;
            let mut conn = sqlx::MySqlConnection::connect(&format!("{}/", base_dsn))
                .await
                .expect("Failed to connect to MySQL server");
            sqlx::query(&format!("CREATE DATABASE IF NOT EXISTS `{}`", db_name))
                .execute(&mut conn)
                .await
                .expect("Failed to create database");

            let db = MysqlDb::new();
            db.create_table().await.expect("Failed to create table");

            // Truncate to clean up data from previous runs
            let mut truncate_conn = sqlx::MySqlConnection::connect(&dsn)
                .await
                .expect("Failed to connect for truncate");
            sqlx::query("TRUNCATE TABLE meta")
                .execute(&mut truncate_conn)
                .await
                .expect("Failed to truncate meta table");

            DB_AND_SCHEMA_CREATED.store(true, Ordering::SeqCst);
            println!("✓ Database initialized and truncated for clean test run");
            return (db, unique_prefix);
        }

        let db = MysqlDb::new();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        (db, unique_prefix)
    }

    /// Tests for get_for_update method
    mod get_for_update_tests {
        use super::*;

        /// Test basic get_for_update success scenario.
        #[test]
        #[serial]
        fn test_get_for_update_basic_update() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/gfu_test/{}/basic_update/key1", prefix);

                db.put(&key, Bytes::from("initial_value"), false, Some(0))
                    .await
                    .expect("Initial put failed");

                let initial = db.get(&key).await.expect("Initial get failed");
                assert_eq!(initial, Bytes::from("initial_value"));

                let update_fn: Box<infra::db::UpdateFn> = Box::new(|value: Option<Bytes>| {
                    let current = value.map(|v| String::from_utf8_lossy(&v).to_string());
                    assert_eq!(current, Some("initial_value".to_string()));
                    Ok(Some((Some(Bytes::from("updated_value")), None)))
                });

                db.get_for_update(&key, false, Some(0), update_fn)
                    .await
                    .expect("get_for_update failed");

                let updated = db.get(&key).await.expect("Get after update failed");
                assert_eq!(updated, Bytes::from("updated_value"));
                println!("✓ get_for_update basic update test passed");
            });
        }

        /// Test get_for_update when update_fn returns None.
        #[test]
        #[serial]
        fn test_get_for_update_returns_none() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/gfu_test/{}/returns_none/key1", prefix);

                db.put(&key, Bytes::from("original_value"), false, Some(0))
                    .await
                    .expect("Initial put failed");

                let update_fn: Box<infra::db::UpdateFn> = Box::new(|value: Option<Bytes>| {
                    assert!(value.is_some());
                    Ok(None)
                });

                db.get_for_update(&key, false, Some(0), update_fn)
                    .await
                    .expect("get_for_update should succeed");

                let result = db.get(&key).await.expect("Get after no-update failed");
                assert_eq!(result, Bytes::from("original_value"));
                println!("✓ get_for_update returns None test passed");
            });
        }

        /// Test get_for_update when update_fn returns an error.
        #[test]
        #[serial]
        fn test_get_for_update_returns_error() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/gfu_test/{}/returns_error/key1", prefix);

                db.put(&key, Bytes::from("original_value"), false, Some(0))
                    .await
                    .expect("Initial put failed");

                let update_fn: Box<infra::db::UpdateFn> = Box::new(|_value: Option<Bytes>| {
                    Err(infra::errors::Error::Message("Test error".to_string()))
                });

                let result = db.get_for_update(&key, false, Some(0), update_fn).await;
                assert!(result.is_err());

                let data = db.get(&key).await.expect("Get after error failed");
                assert_eq!(data, Bytes::from("original_value"));
                println!("✓ get_for_update returns error test passed");
            });
        }

        /// Test get_for_update inserting a new record.
        #[test]
        #[serial]
        fn test_get_for_update_insert_when_not_exist() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/gfu_test/{}/insert_new/key1", prefix);

                let update_fn: Box<infra::db::UpdateFn> = Box::new(|value: Option<Bytes>| {
                    assert!(value.is_none(), "Should have no existing value");
                    Ok(Some((Some(Bytes::from("newly_inserted")), None)))
                });

                db.get_for_update(&key, false, Some(0), update_fn)
                    .await
                    .expect("get_for_update should insert");

                let result = db.get(&key).await.expect("Get after insert failed");
                assert_eq!(result, Bytes::from("newly_inserted"));
                println!("✓ get_for_update insert when not exist test passed");
            });
        }

        /// Test get_for_update with a new key returned from update_fn.
        #[test]
        #[serial]
        fn test_get_for_update_with_new_key() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/gfu_test/{}/with_new_key/key1", prefix);
                let new_key = format!("/gfu_test/{}/with_new_key/key2", prefix);

                db.put(&key, Bytes::from("original"), false, Some(0))
                    .await
                    .expect("Initial put failed");

                let new_key_clone = new_key.clone();
                let update_fn: Box<infra::db::UpdateFn> = Box::new(move |value: Option<Bytes>| {
                    assert!(value.is_some());
                    Ok(Some((
                        None,
                        Some((new_key_clone.clone(), Bytes::from("new_key_value"), Some(0)))
                    )))
                });

                db.get_for_update(&key, false, Some(0), update_fn)
                    .await
                    .expect("get_for_update with new key failed");

                let original = db.get(&key).await.expect("Original key should still exist");
                assert_eq!(original, Bytes::from("original"));

                let result = db.get(&new_key).await.expect("Get new key failed");
                assert_eq!(result, Bytes::from("new_key_value"));
                println!("✓ get_for_update with new key test passed");
            });
        }

        /// Test get_for_update with start_dt parameter.
        #[test]
        #[serial]
        fn test_get_for_update_with_start_dt() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/gfu_test/{}/with_start_dt/key1", prefix);

                db.put(&key, Bytes::from("version_100"), false, Some(100))
                    .await
                    .expect("Put with start_dt=100 failed");

                let update_fn: Box<infra::db::UpdateFn> = Box::new(|value: Option<Bytes>| {
                    assert_eq!(value, Some(Bytes::from("version_100")));
                    Ok(Some((Some(Bytes::from("updated_version")), None)))
                });

                db.get_for_update(&key, false, Some(100), update_fn)
                    .await
                    .expect("get_for_update with start_dt failed");

                let result = db.get(&key).await.expect("Get after update failed");
                assert_eq!(result, Bytes::from("updated_version"));
                println!("✓ get_for_update with start_dt test passed");
            });
        }

        /// Test get_for_update without start_dt (None) gets the latest record.
        #[test]
        #[serial]
        fn test_get_for_update_without_start_dt_gets_latest() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/gfu_test/{}/latest/key1", prefix);

                db.put(&key, Bytes::from("old_version"), false, Some(100))
                    .await
                    .expect("Put old version failed");
                db.put(&key, Bytes::from("new_version"), false, Some(200))
                    .await
                    .expect("Put new version failed");

                let update_fn: Box<infra::db::UpdateFn> = Box::new(|value: Option<Bytes>| {
                    assert_eq!(value, Some(Bytes::from("new_version")));
                    Ok(Some((Some(Bytes::from("latest_updated")), None)))
                });

                db.get_for_update(&key, false, None, update_fn)
                    .await
                    .expect("get_for_update should get latest");

                let result = db.get(&key).await.expect("Get latest failed");
                assert_eq!(result, Bytes::from("latest_updated"));
                println!("✓ get_for_update without start_dt gets latest test passed");
            });
        }

        /// Test get_for_update with both update and new key.
        #[test]
        #[serial]
        fn test_get_for_update_update_and_new_key() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/gfu_test/{}/update_and_new/key1", prefix);
                let new_key = format!("/gfu_test/{}/update_and_new/key2", prefix);

                db.put(&key, Bytes::from("original"), false, Some(0))
                    .await
                    .expect("Initial put failed");

                let new_key_clone = new_key.clone();
                let update_fn: Box<infra::db::UpdateFn> = Box::new(move |value: Option<Bytes>| {
                    assert!(value.is_some());
                    Ok(Some((
                        Some(Bytes::from("updated_original")),
                        Some((new_key_clone.clone(), Bytes::from("new_key_value"), Some(0)))
                    )))
                });

                db.get_for_update(&key, false, Some(0), update_fn)
                    .await
                    .expect("get_for_update should succeed");

                let original = db.get(&key).await.expect("Get original failed");
                assert_eq!(original, Bytes::from("updated_original"));

                let new_result = db.get(&new_key).await.expect("Get new key failed");
                assert_eq!(new_result, Bytes::from("new_key_value"));
                println!("✓ get_for_update update and new key test passed");
            });
        }
    }

    /// Tests for CRUD operations
    mod crud_tests {
        use super::*;

        #[test]
        #[serial]
        fn test_put_and_get() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/crud_test/{}/put_get/key1", prefix);

                db.put(&key, Bytes::from("test_value"), false, Some(0))
                    .await
                    .expect("Put failed");

                let result = db.get(&key).await.expect("Get failed");
                assert_eq!(result, Bytes::from("test_value"));
                println!("✓ put and get test passed");
            });
        }

        #[test]
        #[serial]
        fn test_delete() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/crud_test/{}/delete/key1", prefix);

                db.put(&key, Bytes::from("to_delete"), false, Some(0))
                    .await
                    .expect("Put failed");

                db.delete(&key, false, false, None)
                    .await
                    .expect("Delete failed");

                let result = db.get(&key).await;
                assert!(result.is_err());
                println!("✓ delete test passed");
            });
        }

        #[test]
        #[serial]
        fn test_count() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let base = format!("/crud_test/{}/count", prefix);

                for i in 0..3 {
                    db.put(&format!("{}/key{}", base, i), Bytes::from("v"), false, Some(0))
                        .await
                        .expect("Put failed");
                }

                let count = db.count(&base).await.expect("Count failed");
                assert_eq!(count, 3);
                println!("✓ count test passed");
            });
        }

        #[test]
        #[serial]
        fn test_get_nonexistent_key() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/crud_test/{}/nonexistent/key_that_does_not_exist", prefix);

                let result = db.get(&key).await;
                assert!(result.is_err(), "Getting nonexistent key should fail");
                println!("✓ get nonexistent key test passed");
            });
        }

        #[test]
        #[serial]
        fn test_put_overwrites() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/crud_test/{}/overwrite/key1", prefix);

                db.put(&key, Bytes::from("first_value"), false, Some(0))
                    .await
                    .expect("First put failed");

                let result1 = db.get(&key).await.expect("Get first failed");
                assert_eq!(result1, Bytes::from("first_value"));

                db.put(&key, Bytes::from("second_value"), false, Some(0))
                    .await
                    .expect("Second put failed");

                let result2 = db.get(&key).await.expect("Get second failed");
                assert_eq!(result2, Bytes::from("second_value"));
                println!("✓ put overwrites test passed");
            });
        }

        #[test]
        #[serial]
        fn test_delete_with_prefix() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let base = format!("/crud_test/{}/delete_prefix", prefix);

                db.put(&format!("{}/key1", base), Bytes::from("v1"), false, Some(0))
                    .await
                    .expect("Put key1 failed");
                db.put(&format!("{}/key2", base), Bytes::from("v2"), false, Some(0))
                    .await
                    .expect("Put key2 failed");
                db.put(&format!("{}/key3", base), Bytes::from("v3"), false, Some(0))
                    .await
                    .expect("Put key3 failed");

                let count_before = db.count(&base).await.expect("Count failed");
                assert_eq!(count_before, 3);

                db.delete(&base, true, false, None)
                    .await
                    .expect("Delete with prefix failed");

                let count_after = db.count(&base).await.expect("Count after delete failed");
                assert_eq!(count_after, 0);
                println!("✓ delete with prefix test passed");
            });
        }

        #[test]
        #[serial]
        fn test_list() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let base = format!("/crud_test/{}/list", prefix);

                db.put(&format!("{}/a", base), Bytes::from("value_a"), false, Some(0))
                    .await
                    .expect("Put a failed");
                db.put(&format!("{}/b", base), Bytes::from("value_b"), false, Some(0))
                    .await
                    .expect("Put b failed");
                db.put(&format!("{}/c", base), Bytes::from("value_c"), false, Some(0))
                    .await
                    .expect("Put c failed");

                let results = db.list(&base).await.expect("List failed");
                assert_eq!(results.len(), 3);

                let values: Vec<String> = results
                    .into_iter()
                    .map(|(_, v)| String::from_utf8_lossy(&v).to_string())
                    .collect();
                assert!(values.contains(&"value_a".to_string()));
                assert!(values.contains(&"value_b".to_string()));
                assert!(values.contains(&"value_c".to_string()));
                println!("✓ list test passed");
            });
        }

        #[test]
        #[serial]
        fn test_list_keys() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let base = format!("/crud_test/{}/list_keys", prefix);

                db.put(&format!("{}/key1", base), Bytes::from("v1"), false, Some(0))
                    .await
                    .expect("Put key1 failed");
                db.put(&format!("{}/key2", base), Bytes::from("v2"), false, Some(0))
                    .await
                    .expect("Put key2 failed");

                let keys = db.list_keys(&base).await.expect("List keys failed");
                assert_eq!(keys.len(), 2);

                for key in &keys {
                    assert!(key.starts_with(&base), "Key {} should start with {}", key, base);
                }
                println!("✓ list_keys test passed");
            });
        }

        #[test]
        #[serial]
        fn test_stats() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let base = format!("/crud_test/{}/stats", prefix);

                db.put(&format!("{}/key1", base), Bytes::from("value1"), false, Some(0))
                    .await
                    .expect("Put failed");

                let stats = db.stats().await.expect("Stats failed");

                assert!(stats.keys_count >= 0, "Stats keys_count should be non-negative");
                assert!(stats.bytes_len >= 0, "Stats bytes_len should be non-negative");

                println!("Stats: bytes_len={}, keys_count={}", stats.bytes_len, stats.keys_count);
                println!("✓ stats test passed");
            });
        }
    }

    /// Tests for edge cases - ensuring MysqlDb handles special data correctly
    mod edge_case_tests {
        use super::*;

        /// Test that MysqlDb correctly handles Unicode values.
        #[test]
        #[serial]
        fn test_unicode_values() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;

                let unicode_cases = [
                    ("chinese", "中文测试值"),
                    ("japanese", "日本語テスト"),
                    ("korean", "한국어 테스트"),
                    ("emoji", "🎉🚀💻🔥"),
                    ("mixed", "Hello 世界 🌍"),
                ];

                for (name, value) in &unicode_cases {
                    let key = format!("/edge_test/{}/unicode/{}", prefix, name);

                    db.put(&key, Bytes::from(*value), false, Some(0))
                        .await
                        .unwrap_or_else(|e| panic!("Put {} failed: {}", name, e));

                    let result = db.get(&key).await
                        .unwrap_or_else(|e| panic!("Get {} failed: {}", name, e));

                    assert_eq!(
                        String::from_utf8_lossy(&result), *value,
                        "Unicode mismatch for {}", name
                    );
                }
                println!("✓ Unicode values test passed");
            });
        }

        /// Test that MysqlDb correctly handles special characters.
        #[test]
        #[serial]
        fn test_special_characters() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;

                let special_cases = [
                    ("quotes", "value with 'single' and \"double\" quotes"),
                    ("backslash", "value with \\backslash\\"),
                    ("newline", "value with\nnewline"),
                    ("tab", "value with\ttab"),
                    ("percent", "value with % percent"),
                ];

                for (name, value) in &special_cases {
                    let key = format!("/edge_test/{}/special/{}", prefix, name);

                    db.put(&key, Bytes::from(*value), false, Some(0))
                        .await
                        .unwrap_or_else(|e| panic!("Put {} failed: {}", name, e));

                    let result = db.get(&key).await
                        .unwrap_or_else(|e| panic!("Get {} failed: {}", name, e));

                    assert_eq!(
                        String::from_utf8_lossy(&result), *value,
                        "Special char mismatch for {}", name
                    );
                }
                println!("✓ Special characters test passed");
            });
        }

        /// Test that MysqlDb correctly handles empty values.
        #[test]
        #[serial]
        fn test_empty_value() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/edge_test/{}/empty/key1", prefix);

                db.put(&key, Bytes::from(""), false, Some(0))
                    .await
                    .expect("Put empty value failed");

                let result = db.get(&key).await.expect("Get empty value failed");
                assert_eq!(result, Bytes::from(""));
                println!("✓ Empty value test passed");
            });
        }

        /// Test that MysqlDb correctly handles large values (1MB).
        #[test]
        #[serial]
        fn test_large_value() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let key = format!("/edge_test/{}/large/key1", prefix);

                // Create a 1MB value
                let large_value: String = "x".repeat(1024 * 1024);

                db.put(&key, Bytes::from(large_value.clone()), false, Some(0))
                    .await
                    .expect("Put large value failed");

                let result = db.get(&key).await.expect("Get large value failed");
                assert_eq!(result.len(), large_value.len());
                println!("✓ Large value (1MB) test passed");
            });
        }

        /// Test that MysqlDb correctly handles various start_dt values.
        #[test]
        #[serial]
        fn test_start_dt_variations() {
            TEST_RUNTIME.block_on(async {
                let (db, prefix) = setup_trait_test().await;
                let base = format!("/edge_test/{}/start_dt", prefix);

                let start_dts = [0i64, 1, 1000, 1704067200000000i64];

                for start_dt in start_dts {
                    let key = format!("{}/dt_{}", base, start_dt);

                    db.put(&key, Bytes::from(format!("value_{}", start_dt)), false, Some(start_dt))
                        .await
                        .unwrap_or_else(|e| panic!("Put with start_dt {} failed: {}", start_dt, e));
                }

                // Verify count
                let count = db.count(&base).await.expect("Count failed");
                assert_eq!(count, start_dts.len() as i64);
                println!("✓ start_dt variations test passed");
            });
        }
    }
}
