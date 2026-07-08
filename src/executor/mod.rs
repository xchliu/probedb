// ProbeDB 执行器 — 将解析后的 SQL 语句转换为存储操作

use crate::sql::SQLStatement;
use crate::storage::{parse_value, ColumnInfo, StorageEngine, TableSchema};

/// 执行结果
#[derive(Debug)]
pub enum ExecuteResult {
    /// 创建表成功
    TableCreated { name: String },
    /// 插入成功，返回行ID
    Inserted { row_id: u64 },
    /// 查询结果
    SelectResult {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// 消息（无结构化结果）
    Message(String),
}

/// 执行器
pub struct Executor {
    pub engine: StorageEngine,
}

impl Executor {
    pub fn new() -> Self {
        Executor {
            engine: StorageEngine::new(),
        }
    }

    /// 执行一组 SQL 语句
    pub fn execute(&mut self, stmts: Vec<SQLStatement>) -> Result<Vec<ExecuteResult>, String> {
        let mut results = Vec::new();
        for stmt in stmts {
            results.push(self.execute_statement(stmt)?);
        }
        Ok(results)
    }

    fn execute_statement(&mut self, stmt: SQLStatement) -> Result<ExecuteResult, String> {
        match stmt {
            SQLStatement::CreateTable { name, columns } => {
                let schema = TableSchema {
                    name: name.clone(),
                    columns: columns.iter().enumerate().map(|(i, col)| {
                        ColumnInfo {
                            name: col.name.clone(),
                            data_type: col.data_type.clone(),
                            index: i,
                        }
                    }).collect(),
                };
                self.engine.create_table(schema)?;
                Ok(ExecuteResult::TableCreated { name })
            }

            SQLStatement::Insert { table_name, columns: _, values } => {
                let schema = self.engine.get_schema(&table_name)?;
                let col_types: Vec<_> = schema.columns.iter().map(|c| c.data_type.clone()).collect();
                let col_names: Vec<_> = schema.columns.iter().map(|c| c.name.clone()).collect();
                let rows_parsed: Result<Vec<Vec<_>>, String> = values.iter().map(|row_values| {
                    let mut parsed = Vec::new();
                    for (i, val_str) in row_values.iter().enumerate() {
                        if i >= col_types.len() {
                            break;
                        }
                        let clean = val_str.trim_matches('\'');
                        let value = parse_value(clean, &col_types[i]).map_err(|_| {
                            format!("无法解析列 '{}' 的值: {}", col_names[i], clean)
                        })?;
                        parsed.push(value);
                    }
                    Ok(parsed)
                }).collect();
                let all_parsed = rows_parsed?;
                let count = all_parsed.len();
                for row in all_parsed {
                    self.engine.insert(&table_name, row)?;
                }
                Ok(ExecuteResult::Message(format!("插入 {} 行数据", count)))
            }

            SQLStatement::Select { table_name, columns: _, where_clause: _, order_by: _, limit } => {
                let schema = self.engine.get_schema(&table_name)?;
                let col_names: Vec<String> = schema.columns.iter().map(|c| c.name.clone()).collect();

                // 因为 rows 的引用和后面的 mutate 冲突，先复制数据
                let rows = self.engine.scan_table(&table_name)?;
                let result_rows: Vec<Vec<String>> = rows.iter().map(|row| {
                    row.values.iter().map(|v| v.to_string()).collect()
                }).collect();

                // 处理 LIMIT
                let mut output = result_rows;
                if let Some(limit_val) = limit {
                    let limit = limit_val as usize;
                    if limit < output.len() {
                        output.truncate(limit);
                    }
                }

                Ok(ExecuteResult::SelectResult {
                    columns: col_names,
                    rows: output,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::parse_sql;

    #[test]
    fn test_create_and_insert_and_select() {
        let mut executor = Executor::new();

        // CREATE TABLE
        let sql = "CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)";
        let stmts = parse_sql(sql).unwrap();
        let results = executor.execute(stmts).unwrap();
        assert!(matches!(&results[0], ExecuteResult::TableCreated { .. }));

        // INSERT
        let sql = "INSERT INTO users (id, name, age) VALUES (1, 'alice', 30)";
        let stmts = parse_sql(sql).unwrap();
        let results = executor.execute(stmts).unwrap();
        assert!(matches!(&results[0], ExecuteResult::Message(_)));

        // SELECT
        let sql = "SELECT id, name, age FROM users";
        let stmts = parse_sql(sql).unwrap();
        let results = executor.execute(stmts).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns.len(), 3);
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], "1"); // id
                assert_eq!(rows[0][1], "alice"); // name
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_vector_create_insert_select() {
        let mut executor = Executor::new();

        // 建表
        let sql = "CREATE TABLE items (id INTEGER, embedding VECTOR(3))";
        let stmts = parse_sql(sql).unwrap();
        executor.execute(stmts).unwrap();

        // 插入向量数据
        let sql = "INSERT INTO items (id, embedding) VALUES (1, '[0.1,0.2,0.3]')";
        let stmts = parse_sql(sql).unwrap();
        executor.execute(stmts).unwrap();

        // 查询
        let sql = "SELECT id, embedding FROM items";
        let stmts = parse_sql(sql).unwrap();
        let results = executor.execute(stmts).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns.len(), 2);
                assert_eq!(rows.len(), 1);
                assert!(rows[0][1].contains("0.1"));
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_full_pipeline() {
        let mut executor = Executor::new();

        // 完整的建表→插入→查询链路
        let sqls = vec![
            "CREATE TABLE products (id INTEGER, name TEXT, price FLOAT)",
            "INSERT INTO products (id, name, price) VALUES (1, 'laptop', 999.99)",
            "INSERT INTO products (id, name, price) VALUES (2, 'mouse', 29.99)",
            "SELECT id, name, price FROM products",
        ];

        let mut final_result = None;
        for sql in sqls {
            let stmts = parse_sql(sql).unwrap();
            let results = executor.execute(stmts).unwrap();
            final_result = Some(results);
        }

        let results = final_result.unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2);
            }
            _ => panic!("Expected SelectResult"),
        }
    }
}