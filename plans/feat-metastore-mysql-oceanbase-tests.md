# feat: MetaStore MySQL 和 OceanBase 全面测试实现

## Overview

为 OpenObserve 的 MetaStore MySQL 和 OceanBase 实现添加全面的单元测试和集成测试。当前测试覆盖存在明显缺口：现有的 MySQL 测试（`mysql.rs` 中约 28 个）均为纯单元测试，不需要数据库连接；而需要真实数据库的集成测试大多被标记为 `ignore`。本计划将建立完整的测试框架，支持 Testcontainers 自动化数据库测试和 OceanBase 兼容性验证。

## Problem Statement / Motivation

### 当前问题

1. **集成测试缺失**: `db/mod.rs` 仅有 3 个基础集成测试，`file_list/mysql.rs` 的 80+ 测试标记为 `ignore`
2. **OceanBase 测试为零**: 虽然 OceanBase 复用 MySQL 驱动，但缺乏兼容性验证（特别是 `GET_LOCK` 行为）
3. **并发测试缺失**: `get_for_update` 的锁机制从未在真实数据库上测试
4. **CI 覆盖不足**: 单元测试 CI 仅运行 PostgreSQL 和 SQLite，缺少 MySQL 测试

### 影响

- 生产环境可能出现未测试的边缘情况
- OceanBase 用户可能遇到兼容性问题
- 并发场景下的数据完整性无法保证

## Proposed Solution

建立三层测试体系：

```
┌─────────────────────────────────────────────────────────────┐
│                    测试金字塔                                │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────────────────────────────────────────────┐   │
│  │           性能测试 (criterion)                       │   │
│  │         真实数据库 | 发布前运行                       │   │
│  └─────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │       OceanBase 兼容性测试 (feature flag)           │   │
│  │         真实 OceanBase | 定期运行                    │   │
│  └─────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │          集成测试 (Testcontainers)                   │   │
│  │         MySQL 容器 | PR 合并时运行                   │   │
│  └─────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              单元测试 (#[test])                      │   │
│  │            无数据库 | 每次提交                        │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

## Technical Approach

### Architecture

#### 测试目录结构

```
src/infra/
├── Cargo.toml                    # 添加 dev-dependencies 和 features
└── src/
    └── db/
        ├── mod.rs
        ├── mysql.rs              # 现有单元测试保留
        └── tests/                # 新建测试模块
            ├── mod.rs            # 测试模块入口
            ├── fixtures/         # SQL 测试数据
            │   └── meta_seed.sql
            ├── helpers.rs        # 测试辅助函数
            ├── mysql_unit_tests.rs
            ├── mysql_integration_tests.rs
            └── oceanbase_compat_tests.rs
```

#### 依赖配置

```toml
# src/infra/Cargo.toml

[features]
default = []
cloud = []
# 新增测试 features
integration-tests = []
mysql-tests = ["integration-tests"]
oceanbase-tests = ["integration-tests"]

[dev-dependencies]
collapse.workspace = true
sea-orm = { workspace = true, features = ["mock"] }
tempfile.workspace = true
# 新增
testcontainers = "0.23"
testcontainers-modules = { version = "0.11", features = ["mysql"] }
serial_test = "3.0"
```

### Implementation Phases

#### Phase 1: 测试基础设施搭建

**任务:**
- [ ] 创建 `src/infra/src/db/tests/` 目录结构
- [ ] 添加 `testcontainers` 和 `testcontainers-modules` 依赖
- [ ] 实现 `TestDatabase` 辅助结构体
- [ ] 创建 `meta_seed.sql` 测试数据固件
- [ ] 配置 feature flags (`mysql-tests`, `oceanbase-tests`)

**关键文件:**

```rust
// src/infra/src/db/tests/helpers.rs

use testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
use testcontainers_modules::mysql::Mysql;
use sqlx::{MySqlPool, Row};
use std::time::Duration;

pub struct TestMySqlContainer {
    _container: ContainerAsync<Mysql>,
    pub pool: MySqlPool,
    pub dsn: String,
}

impl TestMySqlContainer {
    pub async fn new() -> Self {
        let container = Mysql::default()
            .with_env_var("MYSQL_ROOT_PASSWORD", "test_password")
            .with_env_var("MYSQL_DATABASE", "openobserve_test")
            .start()
            .await
            .expect("Failed to start MySQL container");

        let host_port = container.get_host_port_ipv4(3306).await.unwrap();
        let dsn = format!(
            "mysql://root:test_password@127.0.0.1:{}/openobserve_test",
            host_port
        );

        // 等待数据库就绪
        let pool = Self::wait_for_connection(&dsn).await;

        // 创建表结构
        Self::create_schema(&pool).await;

        Self {
            _container: container,
            pool,
            dsn,
        }
    }

    async fn wait_for_connection(dsn: &str) -> MySqlPool {
        for _ in 0..30 {
            match MySqlPool::connect(dsn).await {
                Ok(pool) => return pool,
                Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
            }
        }
        panic!("Failed to connect to MySQL after 15 seconds");
    }

    async fn create_schema(pool: &MySqlPool) {
        sqlx::query(include_str!("./fixtures/meta_schema.sql"))
            .execute(pool)
            .await
            .expect("Failed to create schema");
    }

    pub async fn truncate(&self) {
        sqlx::query("TRUNCATE TABLE meta")
            .execute(&self.pool)
            .await
            .expect("Failed to truncate");
    }
}
```

```sql
-- src/infra/src/db/tests/fixtures/meta_schema.sql

CREATE TABLE IF NOT EXISTS meta (
    id BIGINT NOT NULL PRIMARY KEY AUTO_INCREMENT,
    module VARCHAR(100) NOT NULL,
    key1 VARCHAR(256) NOT NULL,
    key2 VARCHAR(256) NOT NULL,
    start_dt BIGINT NOT NULL DEFAULT 0,
    value LONGTEXT NOT NULL,
    INDEX idx_meta_module (module),
    INDEX idx_meta_module_key1 (module, key1),
    UNIQUE INDEX idx_meta_module_key1_key2_start_dt (module, key1, key2, start_dt)
);
```

**成功标准:**
- [ ] `cargo test --package infra --features mysql-tests` 能编译通过
- [ ] Testcontainers 能自动启动 MySQL 容器
- [ ] 测试完成后容器自动清理

**预估改动:** ~200 行新代码

---

#### Phase 2: MySQL 集成测试

**任务:**
- [ ] 实现 CRUD 基础操作测试
- [ ] 实现并发操作测试（`get_for_update`）
- [ ] 实现批量操作测试
- [ ] 实现错误恢复测试
- [ ] 实现边缘情况测试

**关键测试用例:**

```rust
// src/infra/src/db/tests/mysql_integration_tests.rs

#![cfg(feature = "mysql-tests")]

use super::helpers::TestMySqlContainer;
use crate::db::mysql::MysqlDb;
use crate::db::Db;
use bytes::Bytes;
use serial_test::serial;
use std::sync::Arc;

mod crud_tests {
    use super::*;

    #[tokio::test]
    #[serial]
    async fn test_put_and_get() {
        let ctx = TestMySqlContainer::new().await;
        // 设置环境变量以使用测试数据库
        std::env::set_var("ZO_META_MYSQL_DSN", &ctx.dsn);

        let db = MysqlDb::default();
        let key = "/test/crud/basic";
        let value = Bytes::from("test_value");

        db.put(key, value.clone(), false, None).await.unwrap();
        let result = db.get(key).await.unwrap();

        assert_eq!(result, value);
    }

    #[tokio::test]
    #[serial]
    async fn test_delete_single() {
        let ctx = TestMySqlContainer::new().await;
        std::env::set_var("ZO_META_MYSQL_DSN", &ctx.dsn);

        let db = MysqlDb::default();
        let key = "/test/delete/single";

        db.put(key, Bytes::from("value"), false, None).await.unwrap();
        db.delete(key, false, false, None).await.unwrap();

        let result = db.get(key).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    #[serial]
    async fn test_delete_with_prefix() {
        let ctx = TestMySqlContainer::new().await;
        std::env::set_var("ZO_META_MYSQL_DSN", &ctx.dsn);

        let db = MysqlDb::default();

        // 插入多条记录
        for i in 0..5 {
            let key = format!("/test/delete/prefix/key{}", i);
            db.put(&key, Bytes::from("value"), false, None).await.unwrap();
        }

        // 使用前缀删除
        db.delete("/test/delete/prefix/", true, false, None).await.unwrap();

        // 验证全部删除
        let keys = db.list_keys("/test/delete/prefix/").await.unwrap();
        assert!(keys.is_empty());
    }
}

mod concurrency_tests {
    use super::*;

    #[tokio::test]
    #[serial]
    async fn test_concurrent_get_for_update() {
        let ctx = TestMySqlContainer::new().await;
        std::env::set_var("ZO_META_MYSQL_DSN", &ctx.dsn);

        let db = Arc::new(MysqlDb::default());
        let key = "/test/concurrent/counter";

        // 初始化计数器
        db.put(key, Bytes::from("0"), false, None).await.unwrap();

        // 并发增加计数器
        let mut handles = vec![];
        for _ in 0..10 {
            let db = Arc::clone(&db);
            let key = key.to_string();
            handles.push(tokio::spawn(async move {
                db.get_for_update(
                    &key,
                    false,
                    None,
                    Box::new(|value| {
                        let v: i32 = String::from_utf8_lossy(&value.unwrap())
                            .parse()
                            .unwrap_or(0);
                        Ok(Some((Some(Bytes::from((v + 1).to_string())), None)))
                    }),
                )
                .await
            }));
        }

        for h in handles {
            h.await.unwrap().unwrap();
        }

        // 验证最终值
        let final_value = db.get(key).await.unwrap();
        assert_eq!(final_value, Bytes::from("10"));
    }

    #[tokio::test]
    #[serial]
    async fn test_lock_timeout() {
        let ctx = TestMySqlContainer::new().await;
        std::env::set_var("ZO_META_MYSQL_DSN", &ctx.dsn);

        let db = Arc::new(MysqlDb::default());
        let key = "/test/lock/timeout";

        db.put(key, Bytes::from("initial"), false, None).await.unwrap();

        // 第一个事务持有锁较长时间
        let db1 = Arc::clone(&db);
        let key1 = key.to_string();
        let handle1 = tokio::spawn(async move {
            db1.get_for_update(
                &key1,
                false,
                None,
                Box::new(|_| {
                    // 模拟长时间处理
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    Ok(Some((Some(Bytes::from("updated_by_1")), None)))
                }),
            )
            .await
        });

        // 等待第一个事务获取锁
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // 第二个事务应该等待
        let start = std::time::Instant::now();
        let db2 = Arc::clone(&db);
        let key2 = key.to_string();
        let handle2 = tokio::spawn(async move {
            db2.get_for_update(
                &key2,
                false,
                None,
                Box::new(|_| Ok(Some((Some(Bytes::from("updated_by_2")), None)))),
            )
            .await
        });

        handle1.await.unwrap().unwrap();
        handle2.await.unwrap().unwrap();

        // 验证第二个事务确实等待了
        assert!(start.elapsed() >= std::time::Duration::from_secs(2));
    }
}

mod edge_cases {
    use super::*;

    #[tokio::test]
    #[serial]
    async fn test_special_characters_in_key() {
        let ctx = TestMySqlContainer::new().await;
        std::env::set_var("ZO_META_MYSQL_DSN", &ctx.dsn);

        let db = MysqlDb::default();

        // SQL 注入防护测试
        let dangerous_keys = [
            "/test/special/key'with'quotes",
            "/test/special/key\"with\"double",
            "/test/special/key;DROP TABLE meta;--",
            "/test/special/key%with%percent",
        ];

        for key in &dangerous_keys {
            let value = Bytes::from("safe_value");
            db.put(key, value.clone(), false, None).await.unwrap();
            let result = db.get(key).await.unwrap();
            assert_eq!(result, value, "Failed for key: {}", key);
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_unicode_key_and_value() {
        let ctx = TestMySqlContainer::new().await;
        std::env::set_var("ZO_META_MYSQL_DSN", &ctx.dsn);

        let db = MysqlDb::default();
        let key = "/测试/中文/键";
        let value = Bytes::from("中文值 with emoji 🎉");

        db.put(key, value.clone(), false, None).await.unwrap();
        let result = db.get(key).await.unwrap();

        assert_eq!(result, value);
    }

    #[tokio::test]
    #[serial]
    async fn test_empty_value() {
        let ctx = TestMySqlContainer::new().await;
        std::env::set_var("ZO_META_MYSQL_DSN", &ctx.dsn);

        let db = MysqlDb::default();
        let key = "/test/edge/empty_value";
        let value = Bytes::new();

        db.put(key, value.clone(), false, None).await.unwrap();
        let result = db.get(key).await.unwrap();

        assert_eq!(result, value);
    }

    #[tokio::test]
    #[serial]
    async fn test_large_value() {
        let ctx = TestMySqlContainer::new().await;
        std::env::set_var("ZO_META_MYSQL_DSN", &ctx.dsn);

        let db = MysqlDb::default();
        let key = "/test/edge/large_value";
        // 1MB 数据
        let value = Bytes::from(vec![b'x'; 1024 * 1024]);

        db.put(key, value.clone(), false, None).await.unwrap();
        let result = db.get(key).await.unwrap();

        assert_eq!(result.len(), value.len());
    }
}

mod batch_tests {
    use super::*;
    use crate::db::file_list::FileKey;

    #[tokio::test]
    #[serial]
    async fn test_batch_add() {
        let ctx = TestMySqlContainer::new().await;
        std::env::set_var("ZO_META_MYSQL_DSN", &ctx.dsn);

        // TODO: 实现 batch_add 测试
        // 需要 FileList trait 的测试
    }
}
```

**成功标准:**
- [ ] CRUD 测试 100% 通过
- [ ] 并发测试验证锁机制正确性
- [ ] 边缘情况测试覆盖特殊字符、Unicode、空值、大值
- [ ] 测试覆盖率 > 80%

**预估改动:** ~500 行测试代码

---

#### Phase 3: OceanBase 兼容性测试

**任务:**
- [ ] 配置 OceanBase 测试环境（需要真实实例或 Docker）
- [ ] 验证 `GET_LOCK` 在 OceanBase 上的行为
- [ ] 验证 `AUTO_INCREMENT` 在分布式环境的表现
- [ ] 验证事务隔离级别差异
- [ ] 记录已知兼容性问题

**关键测试:**

```rust
// src/infra/src/db/tests/oceanbase_compat_tests.rs

#![cfg(feature = "oceanbase-tests")]

use super::helpers::TestOceanBaseContainer;
use crate::db::mysql::MysqlDb;
use crate::db::Db;
use bytes::Bytes;
use std::sync::Arc;

/// OceanBase 使用 MySQL 协议，但某些功能可能有差异
/// 这些测试验证关键功能的兼容性

mod get_lock_compatibility {
    use super::*;

    /// 验证 GET_LOCK 在 OceanBase 上是否可用
    #[tokio::test]
    async fn test_get_lock_basic() {
        let ctx = TestOceanBaseContainer::new().await;
        std::env::set_var("ZO_META_MYSQL_DSN", &ctx.dsn);
        std::env::set_var("ZO_META_STORE", "oceanbase");

        let db = MysqlDb::default();
        let key = "/test/oceanbase/lock";

        db.put(key, Bytes::from("initial"), false, None).await.unwrap();

        // 测试 get_for_update（内部使用 GET_LOCK）
        let result = db.get_for_update(
            key,
            false,
            None,
            Box::new(|value| {
                Ok(Some((Some(Bytes::from("updated")), None)))
            }),
        )
        .await;

        assert!(result.is_ok(), "GET_LOCK should work on OceanBase");
    }

    /// 验证锁超时行为
    #[tokio::test]
    async fn test_lock_timeout_behavior() {
        // OceanBase 可能对锁超时有不同的处理
        // 此测试记录实际行为
    }
}

mod auto_increment_compatibility {
    use super::*;

    /// 验证 AUTO_INCREMENT 在 OceanBase 分布式环境下的行为
    #[tokio::test]
    async fn test_auto_increment_uniqueness() {
        let ctx = TestOceanBaseContainer::new().await;
        std::env::set_var("ZO_META_MYSQL_DSN", &ctx.dsn);

        let db = MysqlDb::default();

        // 并发插入，验证 ID 唯一性
        let mut handles = vec![];
        for i in 0..100 {
            let key = format!("/test/oceanbase/auto_inc/{}", i);
            handles.push(async move {
                db.put(&key, Bytes::from("value"), false, None).await
            });
        }

        // 收集所有生成的 ID，验证唯一性
        // 注意：OceanBase 分布式 ID 可能不连续
    }
}

mod transaction_isolation {
    use super::*;

    /// OceanBase 默认使用 Read Committed 隔离级别
    /// 验证在此级别下的并发行为
    #[tokio::test]
    async fn test_read_committed_behavior() {
        // 测试脏读是否被阻止
        // 测试不可重复读是否被允许
    }
}
```

**成功标准:**
- [ ] 基础 CRUD 操作在 OceanBase 上正常工作
- [ ] `GET_LOCK` 兼容性问题被记录或解决
- [ ] 已知差异被文档化

**预估改动:** ~300 行测试代码

---

#### Phase 4: CI/CD 集成

**任务:**
- [ ] 创建 `.github/workflows/mysql-tests.yml`
- [ ] 更新 `.github/workflows/unit-tests.yml` 添加 MySQL 测试
- [ ] 配置 OceanBase 测试（可选，定期运行）
- [ ] 添加测试覆盖率报告

**CI 配置:**

```yaml
# .github/workflows/mysql-tests.yml

name: MySQL Integration Tests

on:
  push:
    branches: [main]
    paths:
      - 'src/infra/src/db/**'
      - 'src/config/src/meta/**'
  pull_request:
    paths:
      - 'src/infra/src/db/**'
      - 'src/config/src/meta/**'

env:
  CARGO_TERM_COLOR: always

jobs:
  mysql-integration:
    runs-on: ubuntu-latest

    services:
      mysql:
        image: mysql:8.0
        env:
          MYSQL_ROOT_PASSWORD: test_password
          MYSQL_DATABASE: openobserve_test
        ports:
          - 3306:3306
        options: >-
          --health-cmd="mysqladmin ping"
          --health-interval=10s
          --health-timeout=5s
          --health-retries=5

    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-action@stable

      - name: Cache cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/bin/
            ~/.cargo/registry/index/
            ~/.cargo/registry/cache/
            ~/.cargo/git/db/
            target/
          key: ${{ runner.os }}-cargo-mysql-tests-${{ hashFiles('**/Cargo.lock') }}

      - name: Run MySQL Integration Tests
        env:
          ZO_META_MYSQL_DSN: mysql://root:test_password@localhost:3306/openobserve_test
          ZO_META_STORE: mysql
        run: |
          cargo test --package infra --features mysql-tests -- --test-threads=1

      - name: Run OceanBase Compatibility Tests (Dry Run)
        if: github.event_name == 'push' && github.ref == 'refs/heads/main'
        env:
          ZO_META_MYSQL_DSN: mysql://root:test_password@localhost:3306/openobserve_test
          ZO_META_STORE: oceanbase
        run: |
          # OceanBase 测试使用 MySQL 作为代理运行（验证基本兼容性）
          cargo test --package infra --features mysql-tests -- --test-threads=1 || true
```

**成功标准:**
- [ ] MySQL 测试在 CI 中自动运行
- [ ] PR 检查包含 MySQL 测试结果
- [ ] 测试失败时阻止合并

**预估改动:** ~100 行 YAML 配置

---

## Alternative Approaches Considered

### 方案 A: 使用 `#[sqlx::test]` 宏

**优点:**
- sqlx 官方推荐
- 自动数据库隔离
- 自动迁移应用

**缺点:**
- 需要 `DATABASE_URL` 环境变量
- 需要重构现有连接池管理
- 与项目现有模式差异较大

**结论:** 不采用，因为项目已有成熟的连接池管理模式

### 方案 B: 使用 Mock 数据库

**优点:**
- 无需真实数据库
- 测试速度快

**缺点:**
- 无法测试真实 SQL 行为
- 无法验证并发锁机制
- 无法测试 OceanBase 兼容性

**结论:** 不采用，本计划的核心目标是真实数据库测试

### 方案 C: 使用 Testcontainers（已采用）

**优点:**
- 自动化容器生命周期
- 每个测试独立环境
- 与现有代码兼容性好

**缺点:**
- 需要 Docker
- 测试启动较慢

**结论:** 采用，最佳平衡

## Acceptance Criteria

### Functional Requirements

- [ ] MySQL 集成测试覆盖所有 `Db` trait 方法
- [ ] 并发测试验证 `get_for_update` 锁机制正确性
- [ ] OceanBase 兼容性测试验证基本 CRUD 操作
- [ ] 边缘情况测试覆盖特殊字符、Unicode、空值、大值
- [ ] 批量操作测试验证原子性

### Non-Functional Requirements

- [ ] 单个测试用例执行时间 < 10 秒
- [ ] 测试套件总执行时间 < 5 分钟
- [ ] 测试代码覆盖率 > 80%（针对 mysql.rs）

### Quality Gates

- [ ] 所有测试在 CI 中通过
- [ ] 代码审查通过
- [ ] 测试文档完整

## Success Metrics

| 指标 | 目标值 | 测量方法 |
|------|--------|----------|
| MySQL 测试覆盖率 | > 80% | cargo llvm-cov |
| 集成测试数量 | > 30 | cargo test --list |
| CI 测试通过率 | 100% | GitHub Actions |
| OceanBase 兼容性问题数 | 记录并处理 | 测试报告 |

## Dependencies & Prerequisites

### 技术依赖

- Docker（Testcontainers 需要）
- Rust toolchain 1.75+
- MySQL 8.0 兼容镜像

### 外部依赖

- `testcontainers = "0.23"`
- `testcontainers-modules = { version = "0.11", features = ["mysql"] }`
- `serial_test = "3.0"`

### 环境依赖

- OceanBase 测试需要真实 OceanBase 实例（可选）

## Risk Analysis & Mitigation

| 风险 | 影响 | 可能性 | 缓解措施 |
|------|------|--------|----------|
| Testcontainers 在 CI 中不稳定 | 高 | 中 | 使用 GitHub Actions 服务容器作为备选 |
| OceanBase `GET_LOCK` 不兼容 | 高 | 中 | 准备 OceanBase 特定的锁实现 |
| 测试执行时间过长 | 中 | 低 | 并行化测试，使用连接池 |
| Docker 资源限制 | 中 | 低 | 配置资源限制，测试后清理 |

## References & Research

### Internal References

- `src/infra/src/db/mysql.rs` - MySQL 实现
- `src/infra/src/db/mod.rs:336-391` - 现有基础测试
- `src/config/src/meta/meta_store.rs` - MetaStore 枚举定义
- `.github/workflows/unit-tests.yml` - 现有 CI 配置

### External References

- [SQLx Testing Documentation](https://docs.rs/sqlx/latest/sqlx/attr.test.html)
- [Testcontainers for Rust](https://rust.testcontainers.org/)
- [OceanBase MySQL Compatibility](https://en.oceanbase.com/docs/common-oceanbase-database-10000000001970955)

### Related Work

- 本计划基于之前完成的 OceanBase MetaStore 支持实现

---

## MVP 实现示例

### helpers.rs

```rust
// src/infra/src/db/tests/helpers.rs

use testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
use testcontainers_modules::mysql::Mysql;
use sqlx::MySqlPool;
use std::time::Duration;

pub struct TestMySqlContainer {
    _container: ContainerAsync<Mysql>,
    pub pool: MySqlPool,
    pub dsn: String,
}

impl TestMySqlContainer {
    pub async fn new() -> Self {
        let container = Mysql::default()
            .with_env_var("MYSQL_ROOT_PASSWORD", "test")
            .with_env_var("MYSQL_DATABASE", "test_db")
            .start()
            .await
            .expect("Failed to start MySQL");

        let port = container.get_host_port_ipv4(3306).await.unwrap();
        let dsn = format!("mysql://root:test@127.0.0.1:{}/test_db", port);

        let pool = loop {
            match MySqlPool::connect(&dsn).await {
                Ok(p) => break p,
                Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
            }
        };

        // 创建 meta 表
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS meta (
                id BIGINT NOT NULL PRIMARY KEY AUTO_INCREMENT,
                module VARCHAR(100) NOT NULL,
                key1 VARCHAR(256) NOT NULL,
                key2 VARCHAR(256) NOT NULL,
                start_dt BIGINT NOT NULL DEFAULT 0,
                value LONGTEXT NOT NULL,
                UNIQUE INDEX idx_meta (module, key1, key2, start_dt)
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        Self { _container: container, pool, dsn }
    }
}
```

### mod.rs (tests 模块入口)

```rust
// src/infra/src/db/tests/mod.rs

#[cfg(test)]
pub mod helpers;

#[cfg(all(test, feature = "mysql-tests"))]
pub mod mysql_integration_tests;

#[cfg(all(test, feature = "oceanbase-tests"))]
pub mod oceanbase_compat_tests;
```
