// ProbeDB 执行器 — 将解析后的 SQL 语句转换为存储操作

use crate::sql::SQLStatement;
use crate::storage::*;
use crate::types::Value;

/// 执行结果
#[derive(Debug)]
pub enum ExecuteResult {
    TableCreated { name: String },
    Inserted { row_id: u64 },
    SelectResult {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
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
                        if i >= col_types.len() { break; }
                        let clean = val_str.trim_matches('\'');
                        let value = parse_value(clean, &col_types[i])
                            .map_err(|_| format!("无法解析列 '{}' 的值: {}", col_names[i], clean))?;
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

            SQLStatement::Select { table_name, columns: _, where_clause, order_by, limit } => {
                let schema = self.engine.get_schema(&table_name)?;
                let col_names: Vec<String> = schema.columns.iter().map(|c| c.name.clone()).collect();

                // 扫描全表 — 需要clone为owned vec，因为filter_rows和sort_rows需要修改
                let rows = self.engine.scan_table(&table_name)?.clone();

                // WHERE 过滤
                let mut matched = rows;
                if let Some(ref condition) = where_clause {
                    matched = filter_rows(&matched, condition, &schema)?;
                }

                // 构建列名索引
                let col_indices: Vec<usize> = schema.columns.iter().map(|c| c.index).collect();

                // ORDER BY
                let mut sorted = if let Some(ref order_by_str) = order_by {
                    sort_rows(&matched, order_by_str, &schema)?
                } else {
                    matched
                };

                // 转换行为字符串
                let mut result_rows: Vec<Vec<String>> = sorted.iter().map(|row| {
                    col_indices.iter().map(|&i| {
                        if i < row.values.len() {
                            row.values[i].to_string()
                        } else {
                            String::new()
                        }
                    }).collect()
                }).collect();

                // LIMIT
                if let Some(limit_val) = limit {
                    let l = limit_val as usize;
                    if l < result_rows.len() {
                        result_rows.truncate(l);
                    }
                }

                Ok(ExecuteResult::SelectResult {
                    columns: col_names,
                    rows: result_rows,
                })
            }
        }
    }
}

// ===== WHERE 条件求值 =====

fn get_column_value<'a>(row: &'a Row, col_name: &str, schema: &TableSchema) -> Result<&'a Value, String> {
    let ci = schema.columns.iter().find(|c| c.name == col_name)
        .ok_or_else(|| format!("列 '{}' 不存在", col_name))?;
    row.values.get(ci.index)
        .ok_or_else(|| format!("列 '{}' 没有值", col_name))
}

/// 解析一个字面量值（数字、字符串、浮点数）
fn parse_literal(s: &str) -> Value {
    let s = s.trim();
    // 字符串（带引号）
    if (s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')) {
        return Value::Text(s[1..s.len()-1].to_string());
    }
    // 浮点数
    if s.contains('.') {
        if let Ok(v) = s.parse::<f64>() {
            return Value::Float(v);
        }
    }
    // 整数
    if let Ok(v) = s.parse::<i64>() {
        return Value::Integer(v);
    }
    // 浮点数 fallback
    if let Ok(v) = s.parse::<f64>() {
        return Value::Float(v);
    }
    Value::Text(s.to_string())
}

/// 比较两个值（支持跨类型比较）
fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Integer(ai), Value::Integer(bi)) => ai.cmp(bi),
        (Value::Float(af), Value::Float(bf)) => af.partial_cmp(bf).unwrap_or(std::cmp::Ordering::Equal),
        (Value::Integer(ai), Value::Float(bf)) => (*ai as f64).partial_cmp(bf).unwrap_or(std::cmp::Ordering::Equal),
        (Value::Float(af), Value::Integer(bi)) => af.partial_cmp(&(*bi as f64)).unwrap_or(std::cmp::Ordering::Equal),
        (Value::Text(at), Value::Text(bt)) => at.cmp(bt),
        _ => std::cmp::Ordering::Equal,
    }
}

/// 求值单个条件（如 "age > 30"）
fn eval_condition(condition: &str, row: &Row, schema: &TableSchema) -> Result<bool, String> {
    let c = condition.trim();

    // 处理 LIKE 操作
    if let Some(pos) = c.to_uppercase().find(" LIKE ") {
        let col_name = c[..pos].trim();
        let pattern = c[pos + 6..].trim().trim_matches('\'');
        let col_val = get_column_value(row, col_name, schema)?;
        let text = match col_val {
            Value::Text(t) => t.clone(),
            Value::Integer(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Vector(_) => return Ok(false),
        };
        return Ok(like_match(&text, pattern));
    }

    // 找到操作符位置
    let ops = [">=", "<=", "!=", "=", ">", "<"];
    let mut op_pos = None;
    let mut found_op = "";

    for op in &ops {
        if let Some(pos) = c.find(op) {
            // 确保不是单词中间（比如"name"里的"="不会有）
            if pos > 0 {
                let before = c.chars().nth(pos - 1).unwrap_or(' ');
                if before.is_alphanumeric() || before == '_' || before == '`' {
                    // 可能是在列名里的匹配，跳过
                    continue;
                }
            }
            op_pos = Some(pos);
            found_op = op;
            break;
        }
    }

    let pos = op_pos.ok_or_else(|| format!("无法解析条件: {}", c))?;

    let col_name = c[..pos].trim().trim_matches('`');
    let val_str = c[pos + found_op.len()..].trim();

    let col_val = get_column_value(row, col_name, schema)?;
    let literal = parse_literal(val_str);

    let cmp = compare_values(col_val, &literal);

    let result = match found_op {
        ">"  => cmp == std::cmp::Ordering::Greater,
        ">=" => cmp == std::cmp::Ordering::Greater || cmp == std::cmp::Ordering::Equal,
        "<"  => cmp == std::cmp::Ordering::Less,
        "<=" => cmp == std::cmp::Ordering::Less || cmp == std::cmp::Ordering::Equal,
        "="  => {
            match (col_val, &literal) {
                (Value::Text(t1), Value::Text(t2)) => t1 == t2,
                _ => cmp == std::cmp::Ordering::Equal,
            }
        }
        "!=" => {
            match (col_val, &literal) {
                (Value::Text(t1), Value::Text(t2)) => t1 != t2,
                _ => cmp != std::cmp::Ordering::Equal,
            }
        }
        _ => return Err(format!("不支持的操作符: {}", found_op)),
    };

    Ok(result)
}

/// 简单 LIKE 模式匹配（不含外部正则库）
fn like_match(text: &str, pattern: &str) -> bool {
    let text = text.to_lowercase();
    let pattern = pattern.to_lowercase();

    // 将 pattern 拆分为不含特殊字符的段
    let segments: Vec<&str> = pattern.split('%').collect();
    if segments.len() == 1 {
        // 没有 %，精确匹配（考虑 _ 作为单字符通配符）
        return wildcard_match(&text, segments[0]);
    }

    // 有 % 的情况
    let mut pos = 0;
    let text_chars: Vec<char> = text.chars().collect();

    for (i, segment) in segments.iter().enumerate() {
        if segment.is_empty() { continue; }

        // 将段中的 _ 转为可匹配的任意字符
        if i == 0 {
            // 第一个段必须在开头匹配
            if !wildcard_match_prefix(&text_chars, pos, segment) { return false; }
            pos += count_match_len(&text_chars, pos, segment);
        } else if i == segments.len() - 1 {
            // 最后一个段必须在结尾匹配
            let text_suffix: String = text_chars.iter().skip(pos).collect();
            if !wildcard_match_suffix(&text_suffix, segment) { return false; }
        } else {
            // 中间的段任意位置匹配
            if let Some(found) = wildcard_find(&text_chars, pos, segment) {
                pos = found;
            } else {
                return false;
            }
        }
    }
    true
}

fn wildcard_match(text: &str, pattern: &str) -> bool {
    if text.len() != pattern.len() { return false; }
    for (tc, pc) in text.chars().zip(pattern.chars()) {
        if pc != '_' && tc != pc { return false; }
    }
    true
}

fn wildcard_match_prefix(chars: &[char], start: usize, pattern: &str) -> bool {
    let pchars: Vec<char> = pattern.chars().collect();
    if start + pchars.len() > chars.len() { return false; }
    for (i, &pc) in pchars.iter().enumerate() {
        if pc != '_' && chars[start + i] != pc { return false; }
    }
    true
}

fn wildcard_match_suffix(text: &str, pattern: &str) -> bool {
    let tchars: Vec<char> = text.chars().collect();
    let pchars: Vec<char> = pattern.chars().collect();
    if tchars.len() < pchars.len() { return false; }
    let offset = tchars.len() - pchars.len();
    for (i, &pc) in pchars.iter().enumerate() {
        if pc != '_' && tchars[offset + i] != pc { return false; }
    }
    true
}

fn count_match_len(chars: &[char], start: usize, pattern: &str) -> usize {
    let pchars: Vec<char> = pattern.chars().collect();
    let mut count = 0;
    for (i, &pc) in pchars.iter().enumerate() {
        if start + i < chars.len() && (pc == '_' || chars[start + i] == pc) {
            count += 1;
        } else { break; }
    }
    count
}

fn wildcard_find(chars: &[char], start: usize, pattern: &str) -> Option<usize> {
    let pchars: Vec<char> = pattern.chars().collect();
    for i in start..=chars.len().saturating_sub(pchars.len()) {
        let mut matched = true;
        for (j, &pc) in pchars.iter().enumerate() {
            if pc != '_' && chars[i + j] != pc { matched = false; break; }
        }
        if matched { return Some(i + pchars.len()); }
    }
    None
}

/// 对一组行应用 WHERE 过滤
fn filter_rows(rows: &[Row], where_clause: &str, schema: &TableSchema) -> Result<Vec<Row>, String> {
    // 按 OR 分割
    let or_parts: Vec<&str> = split_top_level(where_clause, " OR ");

    let mut all_matched = Vec::new();
    for or_part in &or_parts {
        // 按 AND 分割
        let and_parts: Vec<&str> = split_top_level(or_part, " AND ");

        let mut or_matched = Vec::new();
        for row in rows {
            let all_true = and_parts.iter().all(|cond| {
                eval_condition(cond, row, schema).unwrap_or(false)
            });
            if all_true {
                or_matched.push(row.clone());
            }
        }
        all_matched.extend(or_matched);
    }

    // 去重（如果同一个行匹配了多个OR分支）
    all_matched.sort_by_key(|r| r.id);
    all_matched.dedup_by_key(|r| r.id);

    Ok(all_matched)
}

/// 在顶层（不在括号内）分割
fn split_top_level<'a>(s: &'a str, delimiter: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    let mut i = 0;
    let bytes = s.as_bytes();
    let delim_bytes = delimiter.as_bytes();

    while i < s.len() {
        if bytes[i] == b'(' { depth += 1; }
        else if bytes[i] == b')' && depth > 0 { depth -= 1; }
        else if depth == 0 && i + delim_bytes.len() <= s.len() {
            if &bytes[i..i + delim_bytes.len()] == delim_bytes {
                parts.push(&s[start..i]);
                i += delim_bytes.len();
                start = i;
                continue;
            }
        }
        i += 1;
    }
    if start < s.len() {
        parts.push(&s[start..]);
    }

    // trim whitespace from each part
    parts.into_iter().map(|p| p.trim()).filter(|p| !p.is_empty()).collect()
}

// ===== ORDER BY 排序 =====

fn sort_rows(
    rows: &[Row],
    order_by_str: &str,
    schema: &TableSchema,
) -> Result<Vec<Row>, String> {
    let mut sorted = rows.to_vec();

    // 解析 "col ASC" 或 "col DESC"
    let order_by_str = order_by_str.trim();
    let (col_name, descending) = if let Some(pos) = order_by_str.to_uppercase().rfind(" DESC") {
        let col = order_by_str[..pos].trim();
        (col.to_string(), true)
    } else if let Some(pos) = order_by_str.to_uppercase().rfind(" ASC") {
        let col = order_by_str[..pos].trim();
        (col.to_string(), false)
    } else {
        (order_by_str.to_string(), false)
    };

    // 获取列索引
    let ci = schema.columns.iter().find(|c| c.name == col_name)
        .ok_or_else(|| format!("排序列 '{}' 不存在", col_name))?;
    let col_idx = ci.index;

    sorted.sort_by(|a, b| {
        let va = a.values.get(col_idx);
        let vb = b.values.get(col_idx);
        match (va, vb) {
            (Some(va), Some(vb)) => {
                let cmp = compare_values(va, vb);
                if descending { cmp.reverse() } else { cmp }
            }
            _ => std::cmp::Ordering::Equal,
        }
    });

    Ok(sorted)
}

// ===== 允许 cargo build 通过 =====

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::parse_sql;

    #[test]
    fn test_create_and_insert_and_select() {
        let mut executor = Executor::new();
        let sql = "CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)";
        executor.execute(parse_sql(sql).unwrap()).unwrap();

        let sql = "INSERT INTO users (id, name, age) VALUES (1, 'alice', 30)";
        executor.execute(parse_sql(sql).unwrap()).unwrap();

        let sql = "SELECT id, name, age FROM users";
        let results = executor.execute(parse_sql(sql).unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns.len(), 3);
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], "1");
                assert_eq!(rows[0][1], "alice");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_where_filter() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT, age INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (1, 'alice', 30)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (2, 'bob', 25)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (3, 'charlie', 35)").unwrap()).unwrap();

        let results = executor.execute(parse_sql("SELECT id, name, age FROM t WHERE age > 30").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 1, "age > 30 should match 1 row");
                assert_eq!(rows[0][1], "charlie");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_where_equal() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (1, 'alice')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (2, 'bob')").unwrap()).unwrap();

        let results = executor.execute(parse_sql("SELECT id, name FROM t WHERE name = 'alice'").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][1], "alice");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_where_and() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT, age INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (1, 'alice', 30)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (2, 'bob', 25)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (3, 'alice', 35)").unwrap()).unwrap();

        let results = executor.execute(parse_sql("SELECT id FROM t WHERE name = 'alice' AND age >= 30").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2);
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_order_by() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (2, 'bob')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (1, 'alice')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (3, 'charlie')").unwrap()).unwrap();

        let results = executor.execute(parse_sql("SELECT id, name FROM t ORDER BY id ASC").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows[0][0], "1");
                assert_eq!(rows[2][0], "3");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_where_and_order_by() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT, age INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (1, 'alice', 30)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (2, 'bob', 25)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (3, 'charlie', 35)").unwrap()).unwrap();

        let results = executor.execute(parse_sql("SELECT id, name FROM t WHERE age > 20 ORDER BY id DESC").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 3);
                assert_eq!(rows[0][0], "3"); // DESC 排序
            }
            _ => panic!("Expected SelectResult"),
        }
    }
}