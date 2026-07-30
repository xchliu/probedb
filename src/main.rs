// ProbeDB — AI内建数据库
// 从第一天就将AI作为一等公民的原生数据库

mod sql;
mod storage;
mod types;
mod executor;

use executor::Executor;
use sql::parse_sql;

/// ProbeDB 数据库实例（嵌入式模式入口）
pub struct ProbeDB {
    executor: Executor,
}

impl ProbeDB {
    /// 创建一个新的 ProbeDB 实例
    pub fn new() -> Self {
        ProbeDB {
            executor: Executor::new(),
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
}