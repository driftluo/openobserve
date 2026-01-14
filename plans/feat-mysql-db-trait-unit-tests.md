# feat: 为 MysqlDb Trait 方法添加真实数据库单元测试

> 日期: 2026-01-13

## 概述

为 `src/infra/src/db/mysql.rs` 中的 `MysqlDb` 结构体实现的 `Db` trait 方法添加全面的单元测试，重点测试 `get_for_update` 等关键并发控制方法。测试使用真实 MySQL 数据库连接，验证锁机制、事务行为和错误处理逻辑。

## 问题陈述 / 动机

### 现状分析

当前测试文件 `tests/db_mysql_tests.rs` (1127 行) 主要使用**原始 SQL** 测试数据库功能，但存在以下测试缺口：

| 缺口类别 | 描述 | 影响 |
|----------|------|------|
| **GFU-01** | 缺少通过 `MysqlDb` trait 方法的直接测试 | 无法验证实际代码逻辑 |
| **GFU-02~05** | `update_fn` 的 4 个返回分支未测试 | 并发更新核心逻辑无覆盖 |
| **ERR-01~05** | 错误恢复场景未测试 | 异常情况下锁可能未正确释放 |
| **OTH-01~06** | `put`、`delete`、`list` 等方法 trait 级别测试缺失 | trait 实现未验证 |

### 为什么需要这些测试

1. **`get_for_update` 是并发控制核心** - 使用 MySQL `GET_LOCK/RELEASE_LOCK` 实现分布式锁
2. **update_fn 闭包逻辑复杂** - 4 种返回值对应不同代码路径
3. **错误恢复至关重要** - 锁未释放会导致死锁
4. **现有测试仅验证 SQL 层** - 未测试 Rust 代码逻辑

## 提议的解决方案

### 1. 创建新的测试模块结构

在 `tests/db_mysql_tests.rs` 中添加新的测试模块：

```rust
/// 测试 MysqlDb trait 方法的直接调用
mod mysqldb_trait_tests {
    mod get_for_update_tests {
        // get_for_update 的所有场景
    }
    mod put_tests {
        // put 方法测试
    }
    mod get_tests {
        // get 方法测试
    }
    mod delete_tests {
        // delete 方法测试
    }
    mod list_tests {
        // list/list_keys/list_values 测试
    }
}
```

### 2. get_for_update 测试矩阵

| 测试场景 | update_fn 返回 | 预期行为 |
|----------|---------------|----------|
| 基本成功 | `Ok(Some((updated_value, None)))` | 更新记录，释放锁 |
| 无需更新 | `Ok(None)` | 回滚事务，释放锁 |
| 返回错误 | `Err(error)` | 回滚事务，释放锁，返回错误 |
| 插入新记录 | `Ok(Some((None, Some(new_key, new_value))))` | 插入新记录 |
| 更新并插入 | `Ok(Some((updated, Some(new))))` | 更新 + 插入 |
| 锁超时 | N/A | 返回 LockTimeout 错误 |
| 并发竞争 | N/A | 验证数据一致性 |

### 3. 测试辅助工具扩展

在 `tests/common/db_helpers.rs` 中添加：

```rust
// db_helpers.rs

/// 创建用于 trait 测试的 MysqlDb 实例
pub struct MysqlDbTestContext {
    pub instance: RealMySqlInstance,
    // 注意：需要解决 config 初始化问题
}

impl MysqlDbTestContext {
    pub async fn new() -> Self {
        // 1. 初始化配置
        // 2. 设置 DSN 环境变量
        // 3. 创建测试数据库
    }

    pub fn get_db(&self) -> MysqlDb {
        MysqlDb::new()
    }
}
```

## 技术考虑

### 架构影响

1. **配置依赖** - `MysqlDb` 使用 `config::get_config()` 获取 DSN，测试需要初始化配置
2. **连接池共享** - `CLIENT`、`CLIENT_RO`、`CLIENT_DDL` 是静态 Lazy 实例
3. **无法 mock cluster_coordinator** - `need_watch=true` 场景需特殊处理

### 配置初始化方案

```rust
// 方案 1: 使用环境变量
std::env::set_var("ZO_META_MYSQL_DSN", &dsn);
config::init().await;

// 方案 2: 使用 test fixture 初始化配置
// 需要在测试模块中添加 #[ctor] 或 lazy_static 初始化
```

### 安全考虑

- **SQL 注入** - 验证 key 中特殊字符的转义 (`'` → `''`)
- **锁泄漏** - 确保所有错误路径都释放锁
- **数据隔离** - 每个测试前 TRUNCATE 表

## 验收标准

### 功能要求

- [ ] `get_for_update` 所有 5 种 `update_fn` 返回分支均有测试覆盖
- [ ] `get_for_update` 锁超时场景测试通过
- [ ] `get_for_update` 并发竞争测试验证数据一致性
- [ ] `put` 方法两阶段提交逻辑测试通过
- [ ] `get` 方法正常/不存在/错误场景测试通过
- [ ] `delete` 方法各种 `with_prefix` 变体测试通过
- [ ] `list/list_keys/list_values` 方法测试通过

### 非功能要求

- [ ] 所有测试使用 `#[serial]` 确保隔离
- [ ] 测试可通过 `cargo test --test db_mysql_tests --features db-mysql-tests` 运行
- [ ] 测试代码遵循现有命名约定

### 质量门禁

- [ ] 新测试不影响现有测试通过率
- [ ] 代码通过 `cargo clippy`
- [ ] 代码通过 `cargo fmt`

## 成功指标

| 指标 | 目标 |
|------|------|
| `get_for_update` 分支覆盖 | 5/5 (100%) |
| 错误恢复场景覆盖 | 3/5 (主要场景) |
| 测试执行时间 | < 60 秒 |
| 并发测试稳定性 | 连续 10 次运行无失败 |

## 依赖和风险

### 依赖

| 依赖项 | 状态 | 说明 |
|--------|------|------|
| `tests/common/db_helpers.rs` | 已存在 | 需扩展 |
| `RealMySqlInstance` | 已存在 | 复用 |
| `serial_test` crate | 已配置 | 复用 |
| MySQL 测试数据库 | 需要 | 默认 `10.10.14.61:3306` |

### 风险

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| 配置初始化复杂 | 高 | 研究现有集成测试如何初始化 |
| 静态连接池难以测试 | 中 | 可能需要重构或使用特殊测试模式 |
| `cluster_coordinator` mock | 低 | `need_watch=false` 场景优先 |
| 测试不稳定 | 中 | 使用 `#[serial]` + 充分等待 |

## 实现步骤

### Phase 1: 基础设施准备

1. **研究配置初始化** - 分析如何在测试中初始化 `config::get_config()`
2. **扩展 db_helpers.rs** - 添加 `MysqlDbTestContext`
3. **验证连接** - 确保可以创建 `MysqlDb` 实例

### Phase 2: get_for_update 核心测试

4. **基本成功场景** - `update_fn` 返回 `Some((updated, None))`
5. **无需更新场景** - `update_fn` 返回 `None`
6. **错误场景** - `update_fn` 返回 `Err`
7. **插入新记录** - `update_fn` 返回 `Some((None, Some(new)))`
8. **更新并插入** - `update_fn` 返回 `Some((updated, Some(new)))`

### Phase 3: 锁和并发测试

9. **锁超时测试** - 模拟锁等待超时
10. **并发竞争测试** - 多个 worker 同时更新同一 key
11. **锁释放验证** - 错误后锁正确释放

### Phase 4: 其他 Trait 方法测试

12. **put 方法测试** - 两阶段提交逻辑
13. **get 方法测试** - 正常/不存在/错误
14. **delete 方法测试** - 各种前缀变体
15. **list 方法测试** - 列表操作

## 代码示例

### get_for_update 基本测试

```rust
// tests/db_mysql_tests.rs

mod mysqldb_trait_tests {
    use super::*;
    use infra::db::{Db, mysql::MysqlDb};
    use bytes::Bytes;

    mod get_for_update_tests {
        use super::*;

        #[tokio::test]
        #[serial]
        async fn test_get_for_update_basic_success() {
            // 初始化测试环境
            let container = RealMySqlInstance::new().await;
            // TODO: 初始化 config 使 MysqlDb 可以工作

            let db = MysqlDb::new();
            let key = "/test_module/key1/key2";

            // 先插入初始数据
            db.put(key, Bytes::from("initial_value"), false, Some(0))
                .await
                .expect("put failed");

            // 调用 get_for_update 更新数据
            let update_fn = Box::new(|value: Option<Bytes>| {
                let current = value.map(|v| String::from_utf8_lossy(&v).to_string());
                assert_eq!(current, Some("initial_value".to_string()));
                Ok(Some((
                    Some(Bytes::from("updated_value")),
                    None
                )))
            });

            db.get_for_update(key, false, Some(0), update_fn)
                .await
                .expect("get_for_update failed");

            // 验证更新结果
            let result = db.get(key).await.expect("get failed");
            assert_eq!(result, Bytes::from("updated_value"));
        }

        #[tokio::test]
        #[serial]
        async fn test_get_for_update_returns_none() {
            // update_fn 返回 None，不更新数据
            let container = RealMySqlInstance::new().await;
            let db = MysqlDb::new();
            let key = "/test_module/key1/key2";

            db.put(key, Bytes::from("original"), false, Some(0))
                .await
                .expect("put failed");

            let update_fn = Box::new(|_value: Option<Bytes>| {
                Ok(None) // 不更新
            });

            db.get_for_update(key, false, Some(0), update_fn)
                .await
                .expect("get_for_update failed");

            // 验证数据未变
            let result = db.get(key).await.expect("get failed");
            assert_eq!(result, Bytes::from("original"));
        }

        #[tokio::test]
        #[serial]
        async fn test_get_for_update_returns_error() {
            let container = RealMySqlInstance::new().await;
            let db = MysqlDb::new();
            let key = "/test_module/key1/key2";

            db.put(key, Bytes::from("original"), false, Some(0))
                .await
                .expect("put failed");

            let update_fn = Box::new(|_value: Option<Bytes>| {
                Err(anyhow::anyhow!("test error"))
            });

            let result = db.get_for_update(key, false, Some(0), update_fn).await;

            // 应该返回错误
            assert!(result.is_err());

            // 验证数据未变（事务回滚）
            let data = db.get(key).await.expect("get failed");
            assert_eq!(data, Bytes::from("original"));
        }
    }
}
```

### 锁超时测试

```rust
#[tokio::test]
#[serial]
async fn test_get_for_update_lock_timeout() {
    use std::sync::Arc;
    use tokio::sync::Barrier;

    let container = Arc::new(RealMySqlInstance::new().await);
    let db = MysqlDb::new();
    let key = "/test_module/lock_test/key2";

    db.put(key, Bytes::from("initial"), false, Some(0))
        .await
        .expect("put failed");

    let barrier = Arc::new(Barrier::new(2));

    // Worker 1: 长时间持有锁
    let barrier1 = barrier.clone();
    let handle1 = tokio::spawn(async move {
        let db = MysqlDb::new();
        let update_fn = Box::new(|_value: Option<Bytes>| {
            // 模拟长时间操作
            std::thread::sleep(std::time::Duration::from_secs(5));
            Ok(Some((Some(Bytes::from("from_worker1")), None)))
        });

        barrier1.wait().await;
        db.get_for_update(key, false, Some(0), update_fn).await
    });

    // Worker 2: 尝试获取锁（应该超时）
    let barrier2 = barrier.clone();
    let handle2 = tokio::spawn(async move {
        let db = MysqlDb::new();
        let update_fn = Box::new(|_value: Option<Bytes>| {
            Ok(Some((Some(Bytes::from("from_worker2")), None)))
        });

        barrier2.wait().await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        db.get_for_update(key, false, Some(0), update_fn).await
    });

    let result1 = handle1.await.unwrap();
    let result2 = handle2.await.unwrap();

    // Worker 1 成功，Worker 2 可能超时或成功（取决于配置）
    assert!(result1.is_ok());
    // result2 的结果取决于 meta_transaction_lock_timeout 配置
}
```

## 参考文献

### 内部参考

| 文件 | 描述 | 行号 |
|------|------|------|
| `src/infra/src/db/mysql.rs` | MysqlDb 实现 | 99-726 |
| `src/infra/src/db/mysql.rs:228-471` | `get_for_update` 方法 | 228-471 |
| `src/infra/src/db/mod.rs:187-234` | Db trait 定义 | 187-234 |
| `tests/db_mysql_tests.rs` | 现有 MySQL 测试 | 全文 |
| `tests/common/db_helpers.rs` | 测试辅助工具 | 全文 |
| `.github/workflows/mysql-tests.yml` | CI 配置 | 全文 |

### 外部参考

- [SQLx 官方文档](https://docs.rs/sqlx/latest/sqlx/)
- [MySQL GET_LOCK 文档](https://dev.mysql.com/doc/refman/8.0/en/locking-functions.html)
- [Tokio 测试文档](https://docs.rs/tokio/latest/tokio/attr.test.html)
- [serial_test crate](https://docs.rs/serial_test/latest/serial_test/)

### 相关工作

- 现有 MySQL 测试模式参考 `tests/db_mysql_tests.rs`
- OceanBase 测试参考 `tests/db_oceanbase_tests.rs`
