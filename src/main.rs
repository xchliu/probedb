// ProbeDB — AI内建数据库
// 从第一天就将AI作为一等公民的原生数据库

mod sql;
mod storage;
mod types;
mod executor;
mod persistence;

use std::path::Path;

use executor::Executor;
use sql::parse_sql;

/// ProbeDB 数据库实例（嵌入式模式入口）
pub struct ProbeDB {
    executor: Executor,
    /// 绑定的数据库文件路径（None = 纯内存模式）
    path: Option<String>,
}

impl ProbeDB {
    /// 创建一个新的 ProbeDB 实例（纯内存模式）
    pub fn new() -> Self {
        ProbeDB {
            executor: Executor::new(),
            path: None,
        }
    }

    /// 打开（或创建）磁盘上的数据库
    ///
    /// - 文件已存在 → 加载持久化状态 + 重放 WAL（崩溃恢复）
    /// - 文件不存在 → 创建空库（首次 persist() 时落盘）
    ///
    /// 崩溃恢复流程：load 全量快照 → replay WAL 增量 → 若重放了记录，
    /// 立即把恢复结果固化回快照并截断 WAL（避免下次重复重放）。
    pub fn open(path: &str) -> Result<Self, String> {
        let (engine, wal) = if Path::new(path).exists() {
            let mut engine = persistence::load(path)?;
            let wal_path = format!("{}.wal", path);
            if Path::new(&wal_path).exists() {
                let mut wal = persistence::wal::WalLog::open(&wal_path)?;
                let replayed = wal.replay(&mut engine)?;
                if replayed > 0 {
                    persistence::save(&engine, path)?;
                    wal.truncate()?;
                }
                (engine, Some(wal))
            } else {
                (engine, None)
            }
        } else {
            // 新库：同时创建空引擎和 WAL（从第一条 DML 就开始记录）
            let wal_path = format!("{}.wal", path);
            let wal = persistence::wal::WalLog::open(&wal_path)?;
            (storage::StorageEngine::new(), Some(wal))
        };
        Ok(ProbeDB {
            executor: Executor { engine, wal },
            path: Some(path.to_string()),
        })
    }

    /// 将当前状态原子保存到磁盘（临时文件 + rename）
    ///
    /// 快照落盘成功后截断 WAL：增量日志已并入快照，避免下次重放重复。
    pub fn persist(&mut self) -> Result<(), String> {
        match &self.path {
            Some(path) => {
                persistence::save(&self.executor.engine, path)?;
                if let Some(wal) = &mut self.executor.wal {
                    wal.truncate()?;
                }
                Ok(())
            }
            None => Err("未绑定数据库文件，请使用 ProbeDB::open(path) 打开或创建数据库".to_string()),
        }
    }

    /// 执行 SQL 语句，返回格式化结果
    pub fn execute(&mut self, sql: &str) -> Result<String, String> {
        let stmts = parse_sql(sql)?;
        let results = self.executor.execute(stmts)?;
        
        let mut output = Vec::new();
        for result in results {
            match result {
                executor::ExecuteResult::TableCreated { name } => {
                    output.push(format!("表 '{}' 创建成功", name));
                }
                executor::ExecuteResult::Inserted { row_id } => {
                    output.push(format!("插入成功，行ID: {}", row_id));
                }
                executor::ExecuteResult::Deleted { count } => {
                    output.push(format!("删除成功，共 {} 行", count));
                }
                executor::ExecuteResult::Updated { count } => {
                    output.push(format!("更新成功，共 {} 行", count));
                }
                executor::ExecuteResult::SelectResult { columns, rows } => {
                    // 格式化输出
                    let header = columns.join(" | ");
                    let separator = columns.iter().map(|_| "---".to_string()).collect::<Vec<_>>().join(" | ");
                    output.push(format!("查询结果 ({} 行):", rows.len()));
                    output.push(header);
                    output.push(separator);
                    for row in &rows {
                        output.push(row.join(" | "));
                    }
                }
                executor::ExecuteResult::Message(msg) => {
                    output.push(msg);
                }
            }
        }
        Ok(output.join("\n"))
    }
}

fn main() {
    println!("🚀 ProbeDB v0.1.0 — AI内建数据库");
    println!("嵌入式模式，直接执行SQL\n");

    let mut db = ProbeDB::new();

    // 建表 + 插入 + 查询 演示
    let sqls = vec![
        "CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)",
        "INSERT INTO users (id, name, age) VALUES (1, 'alice', 30)",
        "INSERT INTO users (id, name, age) VALUES (2, 'bob', 25)",
        "SELECT id, name, age FROM users",
        "CREATE TABLE items (id INTEGER, embedding VECTOR(3))",
        "INSERT INTO items (id, embedding) VALUES (1, '[0.1,0.2,0.3]')",
        "SELECT id, embedding FROM items",
    ];

    for sql in sqls {
        println!("▶ 执行: {}", sql);
        match db.execute(sql) {
            Ok(result) => println!("{}\n", result),
            Err(e) => println!("❌ 错误: {}\n", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_probedb_full_pipeline() {
        let mut db = ProbeDB::new();
        
        assert!(db.execute("CREATE TABLE t (id INTEGER, name TEXT)").is_ok());
        assert!(db.execute("INSERT INTO t (id, name) VALUES (1, 'hello')").is_ok());
        
        let result = db.execute("SELECT id, name FROM t").unwrap();
        assert!(result.contains("hello"));
        assert!(result.contains("1"));
    }

    #[test]
    fn test_probedb_vector() {
        let mut db = ProbeDB::new();
        
        assert!(db.execute("CREATE TABLE v (id INTEGER, emb VECTOR(2))").is_ok());
        assert!(db.execute("INSERT INTO v (id, emb) VALUES (1, '[0.5,0.5]')").is_ok());
        
        let result = db.execute("SELECT id, emb FROM v").unwrap();
        assert!(result.contains("0.5"));
    }

    #[test]
    fn test_probedb_error_handling() {
        let mut db = ProbeDB::new();
        
        // 查不存在的表
        assert!(db.execute("SELECT * FROM nonexistent").is_err());
        
        // 重复建表
        assert!(db.execute("CREATE TABLE t (id INTEGER)").is_ok());
        assert!(db.execute("CREATE TABLE t (id INTEGER)").is_err());

        // 插入不存在的表
        assert!(db.execute("INSERT INTO ghost (id) VALUES (1)").is_err());

        // 列数不匹配
        assert!(db.execute("CREATE TABLE ctest (id INTEGER, name TEXT)").is_ok());
        assert!(db.execute("INSERT INTO ctest (id) VALUES (1)").is_err(), "缺列插入当前版本不支持，应返回错误");
    }

    #[test]
    fn test_empty_table_operations() {
        let mut db = ProbeDB::new();
        assert!(db.execute("CREATE TABLE empty (id INTEGER, name TEXT)").is_ok());

        // 空表 SELECT
        let r = db.execute("SELECT id, name FROM empty").unwrap();
        assert!(r.contains("0 行"), "空表查询应返回 0 行");

        // 空表 DELETE
        let r = db.execute("DELETE FROM empty").unwrap();
        assert!(r.contains("0 行"), "空表 DELETE 应返回 0");

        // 空表 UPDATE
        let r = db.execute("UPDATE empty SET name = 'xxx'").unwrap();
        assert!(r.contains("0 行"), "空表 UPDATE 应返回 0");

        // 空表 ORDER BY
        let r = db.execute("SELECT id FROM empty ORDER BY id DESC").unwrap();
        assert!(r.contains("0 行"), "空表 ORDER BY 应返回 0 行");
    }

    #[test]
    fn test_where_no_match() {
        let mut db = ProbeDB::new();
        assert!(db.execute("CREATE TABLE t (id INTEGER, name TEXT)").is_ok());
        assert!(db.execute("INSERT INTO t (id, name) VALUES (1, 'hello')").is_ok());

        // WHERE 无匹配
        let r = db.execute("SELECT id FROM t WHERE id = 999").unwrap();
        assert!(r.contains("0 行"), "无匹配 WHERE 应返回 0 行");

        // DELETE WHERE 无匹配
        let r = db.execute("DELETE FROM t WHERE id = 999").unwrap();
        assert!(r.contains("0 行"), "DELETE 无匹配应返回 0 行");

        // UPDATE WHERE 无匹配
        let r = db.execute("UPDATE t SET name = 'x' WHERE id = 999").unwrap();
        assert!(r.contains("0 行"), "UPDATE 无匹配应返回 0 行");
    }

    #[test]
    fn test_like_patterns_integration() {
        let mut db = ProbeDB::new();
        assert!(db.execute("CREATE TABLE t (id INTEGER, name TEXT)").is_ok());
        assert!(db.execute("INSERT INTO t (id, name) VALUES (1, 'apple')").is_ok());
        assert!(db.execute("INSERT INTO t (id, name) VALUES (2, 'appetizer')").is_ok());
        assert!(db.execute("INSERT INTO t (id, name) VALUES (3, 'banana')").is_ok());
        assert!(db.execute("INSERT INTO t (id, name) VALUES (4, 'alphabet')").is_ok());

        // LIKE 'app%'
        let r = db.execute("SELECT id, name FROM t WHERE name LIKE 'app%'").unwrap();
        assert!(r.contains("apple"), "LIKE 'app%' 应匹配 apple");
        assert!(r.contains("appetizer"), "LIKE 'app%' 应匹配 appetizer");

        // LIKE '%ana'
        let r = db.execute("SELECT id, name FROM t WHERE name LIKE '%ana'").unwrap();
        assert!(r.contains("banana"), "LIKE '%ana' 应匹配 banana");

        // LIKE '%pp%'
        let r = db.execute("SELECT id, name FROM t WHERE name LIKE '%pp%'").unwrap();
        assert!(r.contains("apple"), "LIKE '%pp%' 应匹配 apple");
    }

    #[test]
    fn test_multi_statement_batch() {
        let mut db = ProbeDB::new();
        let r = db.execute("CREATE TABLE t (id INTEGER); INSERT INTO t (id) VALUES (1); SELECT id FROM t").unwrap();
        assert!(r.contains("1"), "多语句批处理应返回 INSERT 和 SELECT 结果");
    }

    #[test]
    fn test_combined_crud_pipeline() {
        let mut db = ProbeDB::new();
        assert!(db.execute("CREATE TABLE t (id INTEGER, val TEXT)").is_ok());
        assert!(db.execute("INSERT INTO t (id, val) VALUES (1, 'a'), (2, 'b'), (3, 'c')").is_ok());

        // UPDATE
        assert!(db.execute("UPDATE t SET val = 'updated' WHERE id = 1").is_ok());
        let r = db.execute("SELECT val FROM t WHERE id = 1").unwrap();
        assert!(r.contains("updated"), "UPDATE 应生效");

        // DELETE
        assert!(db.execute("DELETE FROM t WHERE id = 3").is_ok());
        let r = db.execute("SELECT id FROM t ORDER BY id ASC").unwrap();
        assert!(r.contains("2 行"), "DELETE 后应剩 2 行");
    }

    // ===== 持久化测试（Phase 3） =====

    fn temp_db_path(name: &str) -> String {
        std::env::temp_dir().join(name).to_str().unwrap().to_string()
    }

    #[test]
    fn test_probedb_open_persist_reload() {
        let path = temp_db_path("probedb_open_persist_test.pdb");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}.tmp", path));

        // 第一次：open（新建）→ 写入数据 → persist
        {
            let mut db = ProbeDB::open(&path).unwrap();
            assert!(db.execute("CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)").is_ok());
            assert!(db.execute("INSERT INTO users (id, name, age) VALUES (1, 'alice', 30)").is_ok());
            assert!(db.execute("INSERT INTO users (id, name, age) VALUES (2, 'bob', 25)").is_ok());
            db.persist().unwrap();
        }

        // 第二次：open（加载）→ 数据在 → 继续插入 id 连续
        {
            let mut db = ProbeDB::open(&path).unwrap();
            let r = db.execute("SELECT id, name FROM users ORDER BY id ASC").unwrap();
            assert!(r.contains("alice"), "持久化后应能查到 alice");
            assert!(r.contains("bob"), "持久化后应能查到 bob");
            assert!(r.contains("2 行"), "应恢复 2 行数据");

            // next_id 恢复正确：继续插入得到 id=3
            assert!(db.execute("INSERT INTO users (id, name, age) VALUES (3, 'charlie', 35)").is_ok());
            let r2 = db.execute("SELECT id FROM users WHERE id = 3").unwrap();
            assert!(r2.contains("charlie"));
            db.persist().unwrap();
        }

        // 第三次：再次加载验证增量持久化
        {
            let mut db = ProbeDB::open(&path).unwrap();
            let r = db.execute("SELECT id FROM users ORDER BY id ASC").unwrap();
            assert!(r.contains("3 行"), "增量持久化后应 3 行");
        }

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}.tmp", path));
    }

    #[test]
    fn test_probedb_persist_with_vector() {
        let path = temp_db_path("probedb_persist_vector_test.pdb");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}.tmp", path));

        {
            let mut db = ProbeDB::open(&path).unwrap();
            assert!(db.execute("CREATE TABLE items (id INTEGER, emb VECTOR(2))").is_ok());
            assert!(db.execute("INSERT INTO items (id, emb) VALUES (1, '[0.5,0.5]')").is_ok());
            db.persist().unwrap();
        }

        {
            let mut db = ProbeDB::open(&path).unwrap();
            // 向量数据恢复后仍可做相似度查询
            let r = db.execute("SELECT id FROM items WHERE vector_similarity(emb, '[0.5,0.5]') > 0.9").unwrap();
            assert!(r.contains("1"), "向量数据持久化后相似度查询应命中");
        }

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}.tmp", path));
    }

    #[test]
    fn test_probedb_persist_unbound_errors() {
        let mut db = ProbeDB::new();
        let r = db.persist();
        assert!(r.is_err(), "纯内存模式 persist 应报错");
    }

    #[test]
    fn test_probedb_wal_crash_recovery() {
        // 场景：open → 建表+插入 → persist（快照+WAL清空）
        //       → 继续插入/更新/删除（不 persist，只写 WAL）
        //       → 重新 open → WAL 重放 → 数据完整恢复
        let path = temp_db_path("probedb_wal_crash_test.pdb");
        let wal_path = format!("{}.wal", path);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}.tmp", path));
        let _ = std::fs::remove_file(&wal_path);

        // 第一次会话：建表+插入2行 → persist（快照已落盘，WAL 清空）
        {
            let mut db = ProbeDB::open(&path).unwrap();
            assert!(db.execute("CREATE TABLE t (id INTEGER, name TEXT)").is_ok());
            assert!(db.execute("INSERT INTO t (id, name) VALUES (1, 'alice')").is_ok());
            assert!(db.execute("INSERT INTO t (id, name) VALUES (2, 'bob')").is_ok());
            db.persist().unwrap();
        }

        // 第二次会话（模拟崩溃前）：插入3行、更新1行、删除1行，全部只写 WAL 不 persist
        {
            let mut db = ProbeDB::open(&path).unwrap();
            assert!(db.execute("INSERT INTO t (id, name) VALUES (3, 'charlie')").is_ok());
            assert!(db.execute("INSERT INTO t (id, name) VALUES (4, 'dave')").is_ok());
            assert!(db.execute("INSERT INTO t (id, name) VALUES (5, 'eve')").is_ok());
            assert!(db.execute("UPDATE t SET name = 'alice2' WHERE id = 1").is_ok());
            assert!(db.execute("DELETE FROM t WHERE id = 2").is_ok());
            // 崩溃：不 persist，直接 drop
        }

        // WAL 应有记录（3插入+1更新+1删除 = 5条）
        let wal = persistence::wal::WalLog::open(&wal_path).unwrap();
        assert_eq!(wal.entry_count().unwrap(), 5, "崩溃前应有5条WAL记录");

        // 第三次会话：重新 open → 自动重放 WAL 恢复
        {
            let mut db = ProbeDB::open(&path).unwrap();
            let r = db.execute("SELECT id, name FROM t ORDER BY id ASC").unwrap();
            assert!(r.contains("alice2"), "UPDATE 应恢复（alice → alice2）");
            assert!(r.contains("charlie"), "未落盘 INSERT 应恢复");
            assert!(r.contains("eve"), "未落盘 INSERT 应恢复");
            assert!(!r.contains("bob"), "未落盘 DELETE 应生效");
            assert!(r.contains("4 行"), "恢复后应 4 行（1,3,4,5）");
        }

        // 重放后 WAL 已固化并截断
        let wal2 = persistence::wal::WalLog::open(&wal_path).unwrap();
        assert_eq!(wal2.entry_count().unwrap(), 0, "重放固化后 WAL 应清空");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}.tmp", path));
        let _ = std::fs::remove_file(&wal_path);
    }

    #[test]
    fn test_probedb_wal_persist_truncates() {
        // persist 后 WAL 必须清空，再次崩溃重启不会重复重放
        let path = temp_db_path("probedb_wal_truncate_test.pdb");
        let wal_path = format!("{}.wal", path);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}.tmp", path));
        let _ = std::fs::remove_file(&wal_path);

        {
            let mut db = ProbeDB::open(&path).unwrap();
            assert!(db.execute("CREATE TABLE t (id INTEGER)").is_ok());
            assert!(db.execute("INSERT INTO t (id) VALUES (1)").is_ok());
            assert!(db.execute("INSERT INTO t (id) VALUES (2)").is_ok());
            db.persist().unwrap(); // 快照+清WAL
            assert!(db.execute("INSERT INTO t (id) VALUES (3)").is_ok());
            assert!(db.execute("INSERT INTO t (id) VALUES (4)").is_ok());
            db.persist().unwrap(); // 再次快照+清WAL
        }

        // WAL 应为空
        let wal = persistence::wal::WalLog::open(&wal_path).unwrap();
        assert_eq!(wal.entry_count().unwrap(), 0, "persist 后 WAL 应清空");

        // 重启后 4 行都在，且无重复
        let mut db = ProbeDB::open(&path).unwrap();
        let r = db.execute("SELECT id FROM t ORDER BY id ASC").unwrap();
        assert!(r.contains("4 行"), "应有 4 行");
        assert!(!r.contains("8 行"), "不得重复重放");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}.tmp", path));
        let _ = std::fs::remove_file(&wal_path);
    }
}