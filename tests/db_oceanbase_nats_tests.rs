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

//! OceanBase + NATS distributed lock integration tests.
//!
//! These tests verify the OceanBaseDb implementation using NATS distributed locks
//! instead of local mutex. This requires a running NATS server.
//!
//! OceanBaseDb uses NATS distributed locks for the `get_for_update` operation
//! to ensure compatibility with OceanBase versions prior to V4.2.0 that don't
//! support MySQL's GET_LOCK function.
//!
//! # Prerequisites
//!
//! 1. Start NATS server with JetStream:
//! ```bash
//! docker run -d --name nats-test -p 4222:4222 nats:latest -js
//! ```
//!
//! 2. Ensure OceanBase is accessible at the configured DSN.
//!
//! # Running Tests
//!
//! ```bash
//! cargo test --test db_oceanbase_nats_tests --features db-oceanbase-nats-tests \
//!   -- --test-threads=1 --nocapture
//! ```
//!
//! With custom NATS address:
//! ```bash
//! ZO_NATS_ADDR="nats.example.com:4222" \
//!   cargo test --test db_oceanbase_nats_tests --features db-oceanbase-nats-tests \
//!   -- --test-threads=1 --nocapture
//! ```
//!
//! # Cleanup
//!
//! ```bash
//! docker stop nats-test && docker rm nats-test
//! ```

#![cfg(feature = "db-oceanbase-nats-tests")]

mod common;

use common::db_helpers::{init_config_for_oceanbase_nats_tests, RealOceanBaseInstance};
use common::db_tests_impl;
use infra::db::oceanbase::OceanBaseDb;
use once_cell::sync::Lazy;
use serial_test::serial;

// ==================== Global Runtime ====================

static TEST_RUNTIME: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("Failed to create test runtime")
});

/// Setup test environment and return (db, prefix)
async fn setup_test() -> (OceanBaseDb, String) {
    init_config_for_oceanbase_nats_tests();
    let _ = RealOceanBaseInstance::new().await; // Ensures schema and truncation
    let db = OceanBaseDb::new();
    let prefix = format!(
        "ob_nats_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros()
    );
    (db, prefix)
}

// ==================== NATS Connection Test ====================

mod nats_connection_tests {
    use super::*;

    /// Test that NATS connection is established in cluster mode
    #[test]
    #[serial]
    fn test_nats_connection() {
        TEST_RUNTIME.block_on(async {
            init_config_for_oceanbase_nats_tests();

            // Verify config is set correctly
            let cfg = config::get_config();
            assert!(
                !cfg.common.local_mode,
                "local_mode should be false for NATS tests"
            );
            assert_eq!(
                cfg.common.cluster_coordinator, "nats",
                "cluster_coordinator should be nats"
            );
            println!(
                "NATS config: addr={}, prefix={}",
                cfg.nats.addr, cfg.nats.prefix
            );
        });
    }
}

// ==================== Get For Update with NATS Lock Tests ====================

mod get_for_update_nats_tests {
    use super::*;

    #[test]
    #[serial]
    fn test_get_for_update_basic_update() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_get_for_update_basic_update_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_get_for_update_returns_none() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_get_for_update_returns_none_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_get_for_update_returns_error() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_get_for_update_returns_error_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_get_for_update_insert_when_not_exist() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_get_for_update_insert_when_not_exist_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_get_for_update_with_new_key() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_get_for_update_with_new_key_impl(&db, &prefix).await;
        });
    }
}

// ==================== Concurrent Tests with NATS Lock ====================

mod concurrent_nats_tests {
    use super::*;

    #[test]
    #[serial]
    fn test_concurrent_two_clients_same_key() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_concurrent_two_clients_same_key_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_concurrent_lock_serialization() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_concurrent_lock_serialization_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_concurrent_counter_increment() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_concurrent_counter_increment_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_concurrent_different_keys() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_concurrent_different_keys_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_lock_released_on_success() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_lock_released_on_success_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_lock_released_on_update_fn_error() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_lock_released_on_update_fn_error_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_lock_contention_fairness() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_lock_contention_fairness_impl(&db, &prefix, "OceanBase+NATS").await;
        });
    }

    #[test]
    #[serial]
    fn test_concurrent_insert_same_new_key() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_concurrent_insert_same_new_key_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_high_concurrency_20_clients() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_high_concurrency_20_clients_impl(&db, &prefix).await;
        });
    }
}

// ==================== CRUD Tests ====================

mod crud_nats_tests {
    use super::*;

    #[test]
    #[serial]
    fn test_put_and_get() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_put_and_get_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_delete() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_delete_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_list() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_list_impl(&db, &prefix).await;
        });
    }

    #[test]
    #[serial]
    fn test_count() {
        TEST_RUNTIME.block_on(async {
            let (db, prefix) = setup_test().await;
            db_tests_impl::test_count_impl(&db, &prefix).await;
        });
    }
}
