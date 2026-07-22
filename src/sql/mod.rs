// ProbeDB SQL 解析层 — 轻量自研解析器
// 支持我们需要的SQL子集，不依赖外部解析库

use crate::types::DataType;

/// ProbeDB 支持的 SQL 语句类型
#[derive(Debug, Clone)]
pub enum SQLStatement {
    CreateTable {
        name: String,
        columns: Vec<ColumnDef>,
    },
    Insert {
        table_name: String,
        columns: Vec<String>,
        values: Vec<Vec<String>>,
    },
    Select {
        table_name: String,
        columns: Vec<String>,
        where_clause: Option<String>,
        order_by: Option<String>,
        limit: Option<u64>,
    },
    Delete {
        table_name: String,
        where_clause: Option<String>,
    },
    Update {
        table_name: String,
        assignments: Vec<(String, String)>, // (column_name, new_value_expr)
        where_clause: Option<String>,
    },
}

/// 列定义
#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: DataType,
}

/// 解析 SQL 文本（支持 CREATE TABLE / INSERT / SELECT）
pub fn parse_sql(sql: &str) -> Result<Vec<SQLStatement>, String> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Err("SQL语句为空".to_string());
    }

    // 按行处理多条语句（简单分割）
    let statements: Vec<&str> = if trimmed.contains(';') {
        trimmed.split(';').map(|s| s.trim()).filter(|s| !s.is_empty()).collect()
    } else {
        vec![trimmed]
    };

    let mut result = Vec::new();
    for stmt in statements {
        result.push(parse_one(stmt)?);
    }
    Ok(result)
}

fn parse_one(sql: &str) -> Result<SQLStatement, String> {
    let upper = sql.trim().to_uppercase();

    if upper.starts_with("CREATE TABLE") {
        parse_create_table(sql)
    } else if upper.starts_with("INSERT INTO") {
        parse_insert(sql)
    } else if upper.starts_with("SELECT") {
        parse_select(sql)
    } else if upper.starts_with("DELETE FROM") {
        parse_delete(sql)
    } else if upper.starts_with("UPDATE ") {
        parse_update(sql)
    } else {
        Err(format!("不支持的SQL语句: {}", sql))
    }
}

/// 解析 CREATE TABLE 语句
/// CREATE TABLE name (col1 TYPE1, col2 TYPE2, ...)
fn parse_create_table(sql: &str) -> Result<SQLStatement, String> {
    let rest = sql.trim()
        .strip_prefix("CREATE TABLE")
        .or_else(|| sql.trim().strip_prefix("create table"))
        .ok_or_else(|| "无法解析CREATE TABLE".to_string())?
        .trim();

    // 提取表名（第一个单词）
    let (name, rest) = split_first_word(rest)?;

    // 提取列定义（括号内，处理嵌套括号如 VECTOR(3)）
    let rest = rest.trim();
    if !rest.starts_with('(') {
        return Err("缺少列定义左括号".to_string());
    }
    let mut depth = 0;
    let mut paren_end = 0;
    for (i, c) in rest[1..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            ')' if depth == 0 => { paren_end = i + 1; break; }
            _ => {}
        }
    }
    if paren_end == 0 {
        return Err("缺少列定义右括号".to_string());
    }
    let parens = &rest[1..paren_end];

    // 拆分列（按不在括号内的逗号拆分）
    let mut column_strs = Vec::new();
    let mut current = String::new();
    let mut in_paren = 0;
    for c in parens.chars() {
        match c {
            '(' => { in_paren += 1; current.push(c); }
            ')' => { in_paren -= 1; current.push(c); }
            ',' if in_paren == 0 => {
                column_strs.push(current.trim().to_string());
                current = String::new();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        column_strs.push(current.trim().to_string());
    }

    let columns: Result<Vec<ColumnDef>, String> = column_strs.iter()
        .map(|col| parse_column_def(col.trim()))
        .collect();

    Ok(SQLStatement::CreateTable {
        name: name.to_lowercase(),
        columns: columns?,
    })
}

fn parse_column_def(s: &str) -> Result<ColumnDef, String> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(format!("列定义不完整: {}", s));
    }
    let name = parts[0].to_lowercase();
    let type_str = parts[1..].join(" ").to_uppercase();

    let data_type = if type_str.starts_with("INT") {
        DataType::Integer
    } else if type_str.starts_with("FLOAT") || type_str.starts_with("DOUBLE") || type_str.starts_with("REAL") {
        DataType::Float
    } else if type_str.starts_with("TEXT") || type_str.starts_with("VARCHAR") || type_str.starts_with("CHAR") || type_str.starts_with("STRING") {
        DataType::Text
    } else if type_str.starts_with("VECTOR") {
        // 解析 VECTOR(n)
        let dim = if let Some(paren_start) = type_str.find('(') {
            let inner = &type_str[paren_start+1..];
            if let Some(paren_end) = inner.find(')') {
                inner[..paren_end].trim().parse::<usize>().unwrap_or(0)
            } else { 0 }
        } else { 0 };
        DataType::Vector(dim)
    } else {
        return Err(format!("不支持的数据类型: {}", type_str));
    };

    Ok(ColumnDef { name, data_type })
}

/// 解析 INSERT 语句
/// INSERT INTO table (col1, col2) VALUES (v1, v2), (v3, v4)
fn parse_insert(sql: &str) -> Result<SQLStatement, String> {
    let s = sql.trim();
    // 去掉 INSERT INTO
    let rest = s.strip_prefix("INSERT INTO")
        .or_else(|| s.strip_prefix("insert into"))
        .ok_or_else(|| "无法解析INSERT INTO".to_string())?
        .trim();

    // 提取表名
    let (table_name, rest) = split_first_word(rest)?;

    // 提取列名（括号内）
    let rest = rest.trim();
    if !rest.starts_with('(') {
        return Err("缺少列名括号".to_string());
    }
    let (cols_str, values_part) = split_parenthesized(rest)?;
    let columns: Vec<String> = cols_str.split(',')
        .map(|s| s.trim().trim_matches('`').to_lowercase())
        .collect();

    // 提取 VALUES 部分
    let values_part_str = values_part.trim().to_string();
    if !values_part_str.to_uppercase().starts_with("VALUES") {
        return Err("缺少VALUES子句".to_string());
    }
    let vals_str = values_part_str[6..].trim().to_string(); // 去掉 "VALUES"

    // 解析多行值
    let mut values = Vec::new();
    let mut remaining_s = vals_str; // owned String for loop
    loop {
        let (row, rest) = match parse_parenthesized_list(&remaining_s) {
            Ok(r) => r,
            Err(_) => break,
        };
        values.push(row);
        let trimmed = rest.trim();
        if trimmed.is_empty() {
            break;
        } else if trimmed.starts_with(',') {
            remaining_s = trimmed[1..].trim().to_string();
        } else {
            break;
        }
    }

    Ok(SQLStatement::Insert {
        table_name: table_name.to_lowercase(),
        columns,
        values,
    })
}

/// 解析 SELECT 语句
/// SELECT col1, col2 FROM table WHERE cond ORDER BY col LIMIT n
fn parse_select(sql: &str) -> Result<SQLStatement, String> {
    let s = sql.trim();
    // 去掉 SELECT
    let rest = s.strip_prefix("SELECT")
        .or_else(|| s.strip_prefix("select"))
        .ok_or_else(|| "无法解析SELECT".to_string())?
        .trim();

    // 提取列名到 FROM
    let from_pos = rest.to_uppercase().find(" FROM ")
        .ok_or_else(|| "缺少FROM子句".to_string())?;
    let columns_str = &rest[..from_pos];
    let columns: Vec<String> = columns_str.split(',')
        .map(|s| s.trim().to_lowercase())
        .collect();

    // 提取 FROM 后面的部分
    let rest_after_from = rest[from_pos + 6..].trim();

    // 提取表名
    let (table_name, rest_after_table) = split_first_word(rest_after_from)?;

    let mut where_clause = None;
    let mut order_by = None;
    let mut limit = None;

    let mut remaining = rest_after_table.trim().to_string();
    let upper = remaining.to_uppercase();

    // WHERE — 处理"WHERE age > 30"开头的情况
    if upper.starts_with("WHERE ") || upper.starts_with("WHERE\n") {
        // remaining 现在是 "WHERE age > 30 ..."
        // 提取条件到 ORDER BY 或 LIMIT
        let rest = remaining[6..].trim().to_string(); // 去掉 "WHERE "
        let rest_upper = rest.to_uppercase();
        let where_end = rest_upper.find(" ORDER BY ")
            .or_else(|| rest_upper.find(" LIMIT "))
            .unwrap_or(rest.len());
        where_clause = Some(rest[..where_end].trim().to_string());
        // 更新 remaining 用于后续解析
        if where_end < rest.len() {
            remaining = rest[where_end..].to_string();
        } else {
            remaining = String::new();
        }
    } else if let Some(pos) = upper.find(" WHERE ") {
        let cond = remaining[pos + 7..].trim().to_string();
        where_clause = Some(cond.clone());
        remaining = cond;
    }

    // ORDER BY — 先检查是否以 ORDER BY 开头
    let upper2 = remaining.to_uppercase();
    if upper2.starts_with("ORDER BY ") {
        let order_rest = remaining[9..].trim().to_string();
        let end = order_rest.to_uppercase().find(" LIMIT").unwrap_or(order_rest.len());
        order_by = Some(order_rest[..end].trim().to_string());
        // 更新remaining用于后续LIMIT解析
        if end < order_rest.len() {
            remaining = order_rest[end..].to_string();
        }
    } else if let Some(pos) = upper2.find(" ORDER BY ") {
        let order_rest = remaining[pos + 10..].trim().to_string();
        let end = order_rest.to_uppercase().find(" LIMIT").unwrap_or(order_rest.len());
        order_by = Some(order_rest[..end].trim().to_string());
    } else if let Some(pos) = upper2.find(" ORDER") {
        if remaining[pos..].to_uppercase().starts_with(" ORDER BY ") {
            let order_rest = remaining[pos + 9..].trim().to_string();
            let end = order_rest.to_uppercase().find(" LIMIT").unwrap_or(order_rest.len());
            order_by = Some(order_rest[..end].trim().to_string());
        }
    }

    // LIMIT — 先检查是否以 LIMIT 开头（如 "SELECT ... FROM t LIMIT 10"）
    let remaining_upper = remaining.to_uppercase();
    if remaining_upper.starts_with("LIMIT ") {
        let limit_str = remaining[6..].trim();
        if let Ok(n) = limit_str.split_whitespace().next().unwrap_or("0").parse::<u64>() {
            limit = Some(n);
        }
    } else if let Some(pos) = remaining_upper.find(" LIMIT ") {
        let limit_str = remaining[pos + 7..].trim();
        if let Ok(n) = limit_str.split_whitespace().next().unwrap_or("0").parse::<u64>() {
            limit = Some(n);
        }
    }

    Ok(SQLStatement::Select {
        table_name: table_name.to_lowercase(),
        columns,
        where_clause,
        order_by,
        limit,
    })
}

/// 解析 DELETE 语句
/// DELETE FROM table WHERE condition
fn parse_delete(sql: &str) -> Result<SQLStatement, String> {
    let s = sql.trim();
    let rest = s.strip_prefix("DELETE FROM")
        .or_else(|| s.strip_prefix("delete from"))
        .ok_or_else(|| "无法解析DELETE FROM".to_string())?
        .trim();

    // 提取表名
    let (table_name, rest) = split_first_word(rest)?;

    let mut where_clause = None;
    let remaining = rest.trim().to_string();
    let upper = remaining.to_uppercase();

    if upper.starts_with("WHERE ") || upper.starts_with("WHERE\n") {
        let cond = remaining[6..].trim().to_string();
        where_clause = Some(cond);
    } else if let Some(pos) = upper.find(" WHERE ") {
        let cond = remaining[pos + 7..].trim().to_string();
        where_clause = Some(cond);
    } else if !remaining.is_empty() {
        return Err(format!("DELETE语法错误: {}", sql));
    }

    Ok(SQLStatement::Delete {
        table_name: table_name.to_lowercase(),
        where_clause,
    })
}

/// 解析 UPDATE 语句
/// UPDATE table SET col1=val1, col2=val2 WHERE condition
fn parse_update(sql: &str) -> Result<SQLStatement, String> {
    let s = sql.trim();
    let rest = s.strip_prefix("UPDATE")
        .or_else(|| s.strip_prefix("update"))
        .ok_or_else(|| "无法解析UPDATE".to_string())?
        .trim();

    // 提取表名
    let (table_name, rest) = split_first_word(rest)?;
    let rest = rest.trim();

    // 检查 SET
    let upper_rest = rest.to_uppercase();
    if !upper_rest.starts_with("SET ") {
        return Err("UPDATE缺少SET子句".to_string());
    }
    let after_set = rest[4..].trim().to_string(); // 去掉 "SET "

    // 解析赋值列表到 WHERE 或结尾
    let upper_after_set = after_set.to_uppercase();
    let where_pos = upper_after_set.find(" WHERE ");
    let assign_part = if let Some(pos) = where_pos {
        after_set[..pos].trim().to_string()
    } else {
        after_set.trim().to_string()
    };

    // 解析 "col1=val1, col2=val2"
    let mut assignments = Vec::new();
    for part in assign_part.split(',') {
        let part = part.trim();
        if let Some(eq_pos) = part.find('=') {
            let col = part[..eq_pos].trim().to_lowercase();
            let val = part[eq_pos + 1..].trim().to_string();
            assignments.push((col, val));
        } else {
            return Err(format!("UPDATE赋值语法错误: {}", part));
        }
    }

    if assignments.is_empty() {
        return Err("UPDATE至少需要一个赋值".to_string());
    }

    // 提取 WHERE
    let mut where_clause = None;
    if let Some(pos) = where_pos {
        let cond = after_set[pos + 7..].trim().to_string();
        where_clause = Some(cond);
    }

    Ok(SQLStatement::Update {
        table_name: table_name.to_lowercase(),
        assignments,
        where_clause,
    })
}

// ========== 辅助函数 ==========

/// 分割第一个单词和剩余部分
fn split_first_word(s: &str) -> Result<(String, &str), String> {
    let s = s.trim();
    let end = s.find(|c: char| c.is_whitespace()).unwrap_or(s.len());
    if end == 0 {
        return Err("语法错误: 期望标识符".to_string());
    }
    Ok((s[..end].to_string(), s[end..].trim()))
}

/// 拆分括号内的内容和剩余部分
fn split_parenthesized(s: &str) -> Result<(String, String), String> {
    let s = s.trim();
    if !s.starts_with('(') {
        return Err("期望左括号".to_string());
    }
    let mut depth = 0;
    let mut content_start = 1;
    let mut content_end = 0;
    let mut found = false;

    for (i, c) in s[1..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            ')' if depth == 0 => {
                content_end = i + 1;
                found = true;
                break;
            }
            _ => {}
        }
    }

    if !found {
        return Err("括号不匹配".to_string());
    }

    Ok((
        s[content_start..content_end].to_string(),
        s[content_end + 1..].to_string(),
    ))
}

/// 解析括号括起来的值列表
fn parse_parenthesized_list(s: &str) -> Result<(Vec<String>, String), String> {
    let s = s.trim();
    if !s.starts_with('(') {
        return Err("期望左括号".to_string());
    }

    let (content, rest) = split_parenthesized(s)?;

    // 分割逗号，注意引号内的逗号
    let mut values = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;

    for c in content.chars() {
        match c {
            '\'' if in_quote => {
                current.push(c);
                in_quote = false;
            }
            '\'' => {
                current.push(c);
                in_quote = true;
            }
            ',' if !in_quote => {
                values.push(current.trim().to_string());
                current = String::new();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        values.push(current.trim().to_string());
    }

    Ok((values, rest.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_create_table() {
        let stmts = parse_sql("CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)").unwrap();
        match &stmts[0] {
            SQLStatement::CreateTable { name, columns } => {
                assert_eq!(name, "users");
                assert_eq!(columns.len(), 3);
                assert_eq!(columns[0].data_type, DataType::Integer);
                assert_eq!(columns[1].data_type, DataType::Text);
            }
            _ => panic!("Expected CreateTable"),
        }
    }

    #[test]
    fn test_parse_create_table_with_vector() {
        let stmts = parse_sql("CREATE TABLE items (id INTEGER, embedding VECTOR(3))").unwrap();
        match &stmts[0] {
            SQLStatement::CreateTable { columns, .. } => {
                assert_eq!(columns[1].data_type, DataType::Vector(3));
            }
            _ => panic!("Expected CreateTable"),
        }
    }

    #[test]
    fn test_parse_insert() {
        let stmts = parse_sql("INSERT INTO users (id, name) VALUES (1, 'alice')").unwrap();
        match &stmts[0] {
            SQLStatement::Insert { table_name, columns, values } => {
                assert_eq!(table_name, "users");
                assert_eq!(columns.len(), 2);
                assert_eq!(values.len(), 1);
                assert_eq!(values[0][0], "1");
                assert_eq!(values[0][1], "'alice'");
            }
            _ => panic!("Expected Insert"),
        }
    }

    #[test]
    fn test_parse_insert_multi_row() {
        let stmts = parse_sql("INSERT INTO t (id, name) VALUES (1, 'a'), (2, 'b')").unwrap();
        match &stmts[0] {
            SQLStatement::Insert { values, .. } => {
                assert_eq!(values.len(), 2);
            }
            _ => panic!("Expected Insert"),
        }
    }

    #[test]
    fn test_parse_select() {
        let stmts = parse_sql("SELECT id, name FROM users WHERE age > 30").unwrap();
        match &stmts[0] {
            SQLStatement::Select { table_name, columns, where_clause, .. } => {
                assert_eq!(table_name, "users");
                assert_eq!(columns.len(), 2);
                assert!(where_clause.is_some());
                assert!(where_clause.as_ref().unwrap().contains("age > 30"));
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_parse_select_limit() {
        let stmts = parse_sql("SELECT * FROM users LIMIT 10").unwrap();
        match &stmts[0] {
            SQLStatement::Select { limit, .. } => {
                assert_eq!(limit.unwrap(), 10);
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_multiple_statements() {
        let sql = "CREATE TABLE t (id INTEGER); INSERT INTO t (id) VALUES (1); SELECT id FROM t";
        let stmts = parse_sql(sql).unwrap();
        assert_eq!(stmts.len(), 3);
    }

    #[test]
    fn test_vector_insert() {
        let stmts = parse_sql("INSERT INTO items (id, emb) VALUES (1, '[0.1,0.2,0.3]')").unwrap();
        match &stmts[0] {
            SQLStatement::Insert { values, .. } => {
                assert_eq!(values[0][1], "'[0.1,0.2,0.3]'");
            }
            _ => panic!("Expected Insert"),
        }
    }

    #[test]
    fn test_parse_delete() {
        let stmts = parse_sql("DELETE FROM users WHERE id = 1").unwrap();
        match &stmts[0] {
            SQLStatement::Delete { table_name, where_clause } => {
                assert_eq!(table_name, "users");
                assert!(where_clause.is_some());
                assert!(where_clause.as_ref().unwrap().contains("id = 1"));
            }
            _ => panic!("Expected Delete"),
        }
    }

    #[test]
    fn test_parse_delete_all() {
        let stmts = parse_sql("DELETE FROM users").unwrap();
        match &stmts[0] {
            SQLStatement::Delete { table_name, where_clause } => {
                assert_eq!(table_name, "users");
                assert!(where_clause.is_none());
            }
            _ => panic!("Expected Delete"),
        }
    }

    #[test]
    fn test_parse_update() {
        let stmts = parse_sql("UPDATE users SET name = 'bob' WHERE id = 1").unwrap();
        match &stmts[0] {
            SQLStatement::Update { table_name, assignments, where_clause } => {
                assert_eq!(table_name, "users");
                assert_eq!(assignments.len(), 1);
                assert_eq!(assignments[0].0, "name");
                assert!(where_clause.is_some());
            }
            _ => panic!("Expected Update"),
        }
    }

    #[test]
    fn test_parse_update_multi_assign() {
        let stmts = parse_sql("UPDATE users SET name = 'bob', age = 30 WHERE id = 1").unwrap();
        match &stmts[0] {
            SQLStatement::Update { assignments, .. } => {
                assert_eq!(assignments.len(), 2);
                assert_eq!(assignments[0].0, "name");
                assert_eq!(assignments[1].0, "age");
            }
            _ => panic!("Expected Update"),
        }
    }
}