// ProbeDB SQL 解析层 — 轻量自研解析器
// 支持我们需要的SQL子集，不依赖外部解析库

use crate::types::DataType;

/// ProbeDB 支持的 SQL 语句类型
#[derive(Debug, Clone)]
pub enum SQLStatement {
    CreateTable {
        name: String,
        columns: Vec<ColumnDef>,
        /// CREATE TABLE IF NOT EXISTS：表已存在时跳过而不是报错
        if_not_exists: bool,
    },
    /// CREATE [UNIQUE] INDEX [IF NOT EXISTS] name ON table (col, ...)
    ///
    /// 当前实现：校验表/列存在并登记索引名；查询暂不利用索引（全表扫描），
    /// 属于"SQL 兼容层"而非性能索引。
    CreateIndex {
        name: String,
        table: String,
        columns: Vec<String>,
        if_not_exists: bool,
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
        group_by: Option<String>,
        having: Option<String>,
        limit: Option<u64>,
        offset: Option<u64>,
        distinct: bool,
        join: Option<JoinClause>,
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
    DropTable {
        table_name: String,
    },
}

/// 列定义
#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: DataType,
}

/// JOIN 子句定义
/// 支持 INNER JOIN table ON table1.col = table2.col
#[derive(Debug, Clone)]
pub struct JoinClause {
    pub table: String,
    pub left_col: String,
    pub right_col: String,
}

/// 解析 SQL 文本（支持 CREATE TABLE / INSERT / SELECT）
pub fn parse_sql(sql: &str) -> Result<Vec<SQLStatement>, String> {
    // 先去掉 `--` 行注释（字符串内部的 -- 保留）
    let cleaned = strip_line_comments(sql);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return Err("SQL语句为空".to_string());
    }

    // 按分号拆分多条语句（引号内的分号不算分隔符，'' 为转义引号）
    let statements = split_sql_statements(trimmed);

    let mut result = Vec::new();
    for stmt in statements {
        result.push(parse_one(&stmt)?);
    }
    Ok(result)
}

/// 去除 SQL 行注释 `-- ...`（引号内不处理），保留换行以维持语句边界。
fn strip_line_comments(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut in_quote = false;
    while let Some(c) = chars.next() {
        if in_quote {
            out.push(c);
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    out.push(chars.next().expect("peek 已确认存在"));
                    continue;
                }
                in_quote = false;
            }
            continue;
        }
        match c {
            '\'' => {
                in_quote = true;
                out.push(c);
            }
            '-' if chars.peek() == Some(&'-') => {
                chars.next(); // 吃掉第二个 '-'
                while let Some(&n) = chars.peek() {
                    if n == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// 按分号拆分语句，跳过单引号字符串内部；支持 SQL 标准 `''` 转义。
fn split_sql_statements(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut in_quote = false;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                if in_quote && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 2; // 转义引号，仍在字符串内
                    continue;
                }
                in_quote = !in_quote;
                i += 1;
            }
            b';' if !in_quote => {
                let stmt = sql[start..i].trim();
                if !stmt.is_empty() {
                    out.push(stmt.to_string());
                }
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    let tail = sql[start..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

fn parse_one(sql: &str) -> Result<SQLStatement, String> {
    let upper = sql.trim().to_uppercase();

    if upper.starts_with("CREATE INDEX") || upper.starts_with("CREATE UNIQUE INDEX") {
        parse_create_index(sql)
    } else if upper.starts_with("CREATE TABLE") {
        parse_create_table(sql)
    } else if upper.starts_with("INSERT INTO") {
        parse_insert(sql)
    } else if upper.starts_with("SELECT") {
        parse_select(sql)
    } else if upper.starts_with("DELETE FROM") {
        parse_delete(sql)
    } else if upper.starts_with("UPDATE ") {
        parse_update(sql)
    } else if upper.starts_with("DROP TABLE ") {
        parse_drop_table(sql)
    } else {
        Err(format!("不支持的SQL语句: {}", sql))
    }
}

/// 解析 CREATE TABLE 语句
/// CREATE TABLE [IF NOT EXISTS] name (col1 TYPE1, col2 TYPE2, ...)
fn parse_create_table(sql: &str) -> Result<SQLStatement, String> {
    let rest = sql.trim()
        .strip_prefix("CREATE TABLE")
        .or_else(|| sql.trim().strip_prefix("create table"))
        .ok_or_else(|| "无法解析CREATE TABLE".to_string())?
        .trim();

    // 可选 IF NOT EXISTS
    let (if_not_exists, rest) = strip_if_not_exists(rest);

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

    // 表级约束（FOREIGN KEY / PRIMARY KEY / UNIQUE / CHECK / CONSTRAINT）当前不建模，
    // 直接跳过——否则会被当成列定义解析失败（SelfMind schema 需要）。
    let columns: Result<Vec<ColumnDef>, String> = column_strs.iter()
        .filter(|col| !is_table_constraint(col))
        .map(|col| parse_column_def(col.trim()))
        .collect();

    Ok(SQLStatement::CreateTable {
        name: name.to_lowercase(),
        columns: columns?,
        if_not_exists,
    })
}

/// 去掉可选的 `IF NOT EXISTS` 前缀（大小写不敏感），返回 (是否存在, 剩余部分)
fn strip_if_not_exists(s: &str) -> (bool, &str) {
    let t = s.trim();
    const PREFIX: &str = "IF NOT EXISTS";
    if t.len() >= PREFIX.len() && t[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        (true, t[PREFIX.len()..].trim())
    } else {
        (false, t)
    }
}

/// 是否是 CREATE TABLE 里的表级约束子句（当前不建模，直接忽略）
fn is_table_constraint(clause: &str) -> bool {
    let upper = clause.trim().to_uppercase();
    ["FOREIGN KEY", "PRIMARY KEY", "UNIQUE (", "UNIQUE(", "CHECK (", "CHECK(", "CONSTRAINT "]
        .iter()
        .any(|prefix| upper.starts_with(prefix))
}

/// 解析 CREATE [UNIQUE] INDEX [IF NOT EXISTS] name ON table (col, ...)
///
/// 当前只接受并校验（表/列存在），不建立物理索引——查询仍是全表扫描。
/// 目的是兼容 SelfMind 的 schema 初始化脚本。
fn parse_create_index(sql: &str) -> Result<SQLStatement, String> {
    let s = sql.trim();
    let rest = if let Some(r) = s.strip_prefix("CREATE UNIQUE INDEX")
        .or_else(|| s.strip_prefix("create unique index"))
    {
        r.trim()
    } else {
        s.strip_prefix("CREATE INDEX")
            .or_else(|| s.strip_prefix("create index"))
            .ok_or_else(|| "无法解析CREATE INDEX".to_string())?
            .trim()
    };

    let (if_not_exists, rest) = strip_if_not_exists(rest);
    let (name, rest) = split_first_word(rest)?;
    let rest = rest.trim();

    let upper = rest.to_uppercase();
    if !upper.starts_with("ON ") {
        return Err("CREATE INDEX 缺少 ON 子句".to_string());
    }
    let after_on = rest[3..].trim();
    // 注意 `ON entries(status)` 表名与括号之间可能没有空格
    let paren_pos = after_on.find('(')
        .ok_or_else(|| "CREATE INDEX 缺少列清单括号".to_string())?;
    let table_part = after_on[..paren_pos].trim();
    let (table, table_alias) = split_first_word(table_part)?;
    let _ = table_alias; // 索引定义允许表别名，忽略
    let after_table = after_on[paren_pos..].trim();

    if !after_table.starts_with('(') {
        return Err("CREATE INDEX 缺少列清单括号".to_string());
    }
    let (cols_str, _) = split_parenthesized(after_table)?;
    let columns: Vec<String> = cols_str
        .split(',')
        .map(|c| c.trim().to_lowercase())
        .filter(|c| !c.is_empty())
        .collect();
    if columns.is_empty() {
        return Err("CREATE INDEX 列清单为空".to_string());
    }

    Ok(SQLStatement::CreateIndex {
        name: name.to_lowercase(),
        table: table.to_lowercase(),
        columns,
        if_not_exists,
    })
}

fn parse_column_def(s: &str) -> Result<ColumnDef, String> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(format!("列定义不完整: {}", s));
    }
    let name = parts[0].to_lowercase();
    let type_str = parts[1..].join(" ").to_uppercase();
    // 基础类型 token（去掉 VECTOR(n) 的括号部分），用于精确匹配 DATE/TIME，
    // 避免 "DATETIME"/"TIMESTAMP" 被 "DATE"/"TIME" 前缀误吞
    let base = type_str.split('(').next().unwrap_or("").trim();

    let data_type = if type_str.starts_with("INT") {
        DataType::Integer
    } else if type_str.starts_with("FLOAT") || type_str.starts_with("DOUBLE") || type_str.starts_with("REAL") {
        DataType::Float
    } else if type_str.starts_with("TEXT") || type_str.starts_with("VARCHAR") || type_str.starts_with("CHAR") || type_str.starts_with("STRING") {
        DataType::Text
    } else if type_str.starts_with("BOOL") {
        DataType::Boolean
    } else if base == "DATE" {
        DataType::Date
    } else if base == "TIME" {
        DataType::Time
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

    // 提取列名（括号内）。省略列清单时（INSERT INTO t VALUES ...）按表结构顺序全列插入。
    let rest = rest.trim();
    let (columns, values_part) = if rest.to_uppercase().starts_with("VALUES") {
        (Vec::new(), rest.to_string())
    } else if rest.starts_with('(') {
        let (cols_str, values_part) = split_parenthesized(rest)?;
        let columns: Vec<String> = cols_str.split(',')
            .map(|s| s.trim().trim_matches('`').to_lowercase())
            .collect();
        (columns, values_part)
    } else {
        return Err("缺少列名括号或VALUES子句".to_string());
    };

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

    // 检查 DISTINCT 关键字
    let (distinct, columns_str) = {
        let trimmed = columns_str.trim();
        if trimmed.to_uppercase().starts_with("DISTINCT ") {
            (true, trimmed[9..].trim())
        } else {
            (false, trimmed)
        }
    };

    let columns: Vec<String> = columns_str.split(',')
        .map(|s| s.trim().to_lowercase())
        .collect();

    // 提取 FROM 后面的部分
    let rest_after_from = rest[from_pos + 6..].trim();

    // 提取表名（支持 table alias，如 `FROM entries e`）
    let (table_name, rest_after_table) = split_table_ref(rest_after_from)?;

    let mut where_clause = None;
    let mut order_by = None;
    let mut group_by = None;
    let mut having = None;
    let mut limit = None;
    let mut offset = None;
    let mut join: Option<JoinClause> = None;

    let mut remaining = rest_after_table.trim().to_string();

    // ===== JOIN 解析 =====
    // 语法: [INNER] JOIN table [alias] ON left_col = right_col
    // JOIN 必须在 WHERE 之前解析
    let upper_check = remaining.to_uppercase();
    let join_match: Option<(usize, usize)> = if let Some(p) = upper_check.find(" INNER JOIN ") {
        Some((p, 12))
    } else if let Some(p) = upper_check.find(" JOIN ") {
        Some((p, 6))
    } else if upper_check.starts_with("INNER JOIN ") {
        Some((0, 11))
    } else if upper_check.starts_with("JOIN ") {
        Some((0, 5))
    } else {
        None
    };
    if let Some((jpos, kw_len)) = join_match {
        // JOIN 关键词后面的部分：table [alias] ON ...
        let after_join_kw = remaining[jpos + kw_len..].trim().to_string();
        // 解析 JOIN 表名（支持别名，如 `decay_history dh`）
        let (join_table, after_join_table) = split_table_ref(&after_join_kw)?;
        let after_join_table = after_join_table.trim().to_string();
        let after_table_upper = after_join_table.to_uppercase();
        let after_on = if after_table_upper.starts_with("ON ") {
            after_join_table[3..].trim().to_string()
        } else if let Some(p) = after_table_upper.find(" ON ") {
            after_join_table[p + 4..].trim().to_string()
        } else {
            return Err("JOIN 缺少 ON 子句".to_string());
        };
        // ON left_col = right_col —— 找到等号
        // 先找后续子句的开始位置，截取 ON 条件
        let after_on_upper = after_on.to_uppercase();
        let on_end = after_on_upper.find(" WHERE ")
            .or_else(|| after_on_upper.find(" GROUP BY "))
            .or_else(|| after_on_upper.find(" HAVING "))
            .or_else(|| after_on_upper.find(" ORDER BY "))
            .or_else(|| after_on_upper.find(" LIMIT "))
            .or_else(|| after_on_upper.find(" OFFSET"))
            .unwrap_or(after_on.len());
        let on_cond = after_on[..on_end].trim();
        // 解析 left_col = right_col（只支持等值连接条件）
        let eq_pos = on_cond.find('=')
            .ok_or_else(|| format!("JOIN ON 条件缺少等号: {}", on_cond))?;
        let left_col = on_cond[..eq_pos].trim().to_lowercase();
        let right_col = on_cond[eq_pos + 1..].trim().to_lowercase();
        join = Some(JoinClause {
            table: join_table,
            left_col,
            right_col,
        });
        // remaining 设为 JOIN 后面的剩余部分（WHERE 等后续子句）
        if on_end < after_on.len() {
            remaining = after_on[on_end..].trim().to_string();
        } else {
            remaining = String::new();
        }
    }

    let upper = remaining.to_uppercase();

    // WHERE — 处理"WHERE age > 30"开头的情况
    if upper.starts_with("WHERE ") || upper.starts_with("WHERE\n") {
        // remaining 现在是 "WHERE age > 30 ..."
        // 提取条件到 GROUP BY / HAVING / ORDER BY / LIMIT
        let rest = remaining[6..].trim().to_string(); // 去掉 "WHERE "
        let rest_upper = rest.to_uppercase();
        let where_end = rest_upper.find(" GROUP BY ")
            .or_else(|| rest_upper.find(" HAVING "))
            .or_else(|| rest_upper.find(" ORDER BY "))
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
        let cond_start = pos + 7;
        // WHERE 条件到 GROUP BY / HAVING / ORDER BY / LIMIT
        let after_where = &remaining[cond_start..];
        let after_upper = after_where.to_uppercase();
        let where_end = after_upper.find(" GROUP BY ")
            .or_else(|| after_upper.find(" HAVING "))
            .or_else(|| after_upper.find(" ORDER BY "))
            .or_else(|| after_upper.find(" LIMIT "))
            .unwrap_or(after_where.len());
        where_clause = Some(after_where[..where_end].trim().to_string());
        remaining = after_where[where_end..].to_string();
    }

    // GROUP BY — 检查是否以 GROUP BY 开头
    let upper_gb = remaining.to_uppercase();
    if upper_gb.starts_with("GROUP BY ") {
        let gb_rest = remaining[9..].trim().to_string();
        let gb_upper = gb_rest.to_uppercase();
        let end = gb_upper.find(" HAVING ")
            .or_else(|| gb_upper.find(" ORDER BY "))
            .or_else(|| gb_upper.find(" LIMIT "))
            .or_else(|| gb_upper.find(" OFFSET"))
            .unwrap_or(gb_rest.len());
        group_by = Some(gb_rest[..end].trim().to_string());
        if end < gb_rest.len() {
            remaining = gb_rest[end..].to_string();
        } else {
            remaining = String::new();
        }
    } else if let Some(pos) = upper_gb.find(" GROUP BY ") {
        let gb_rest = remaining[pos + 10..].trim().to_string();
        let gb_upper = gb_rest.to_uppercase();
        let end = gb_upper.find(" HAVING ")
            .or_else(|| gb_upper.find(" ORDER BY "))
            .or_else(|| gb_upper.find(" LIMIT "))
            .or_else(|| gb_upper.find(" OFFSET"))
            .unwrap_or(gb_rest.len());
        group_by = Some(gb_rest[..end].trim().to_string());
        remaining = gb_rest[end..].to_string();
    }

    // HAVING — 检查是否以 HAVING 开头（GROUP BY 之后、ORDER BY 之前）
    let upper_hv = remaining.to_uppercase();
    if upper_hv.starts_with("HAVING ") {
        let hv_rest = remaining[7..].trim().to_string();
        let hv_upper = hv_rest.to_uppercase();
        let end = hv_upper.find(" ORDER BY ")
            .or_else(|| hv_upper.find(" LIMIT "))
            .or_else(|| hv_upper.find(" OFFSET"))
            .unwrap_or(hv_rest.len());
        having = Some(hv_rest[..end].trim().to_string());
        if end < hv_rest.len() {
            remaining = hv_rest[end..].to_string();
        } else {
            remaining = String::new();
        }
    } else if let Some(pos) = upper_hv.find(" HAVING ") {
        let hv_rest = remaining[pos + 8..].trim().to_string();
        let hv_upper = hv_rest.to_uppercase();
        let end = hv_upper.find(" ORDER BY ")
            .or_else(|| hv_upper.find(" LIMIT "))
            .or_else(|| hv_upper.find(" OFFSET"))
            .unwrap_or(hv_rest.len());
        having = Some(hv_rest[..end].trim().to_string());
        remaining = hv_rest[end..].to_string();
    }

    // ORDER BY — 先检查是否以 ORDER BY 开头
    let upper2 = remaining.to_uppercase();
    if upper2.starts_with("ORDER BY ") {
        let order_rest = remaining[9..].trim().to_string();
        let end = order_rest.to_uppercase().find(" LIMIT")
            .or_else(|| order_rest.to_uppercase().find(" OFFSET"))
            .unwrap_or(order_rest.len());
        order_by = Some(order_rest[..end].trim().to_string());
        if end < order_rest.len() {
            remaining = order_rest[end..].to_string();
        }
    } else if let Some(pos) = upper2.find(" ORDER BY ") {
        let order_rest = remaining[pos + 10..].trim().to_string();
        let end = order_rest.to_uppercase().find(" LIMIT")
            .or_else(|| order_rest.to_uppercase().find(" OFFSET"))
            .unwrap_or(order_rest.len());
        order_by = Some(order_rest[..end].trim().to_string());
        remaining = order_rest[end..].to_string();
    } else if let Some(pos) = upper2.find(" ORDER") {
        if remaining[pos..].to_uppercase().starts_with(" ORDER BY ") {
            let order_rest = remaining[pos + 9..].trim().to_string();
            let end = order_rest.to_uppercase().find(" LIMIT")
                .or_else(|| order_rest.to_uppercase().find(" OFFSET"))
                .unwrap_or(order_rest.len());
            order_by = Some(order_rest[..end].trim().to_string());
            remaining = order_rest[end..].to_string();
        }
    }

    // LIMIT — 先检查是否以 LIMIT 开头（如 "SELECT ... FROM t LIMIT 10"）
    let remaining_upper = remaining.to_uppercase();
    if remaining_upper.starts_with("LIMIT ") {
        let limit_str = remaining[6..].trim();
        if let Ok(n) = limit_str.split_whitespace().next().unwrap_or("0").parse::<u64>() {
            limit = Some(n);
        }
        // LIMIT 后面可能跟 OFFSET
        let after_limit = remaining[6..].trim();
        if let Some(offset_pos) = after_limit.to_uppercase().find(" OFFSET ") {
            let offset_str = after_limit[offset_pos + 8..].trim();
            if let Ok(n) = offset_str.split_whitespace().next().unwrap_or("0").parse::<u64>() {
                offset = Some(n);
            }
        } else if let Some(offset_pos) = after_limit.to_uppercase().find(" OFFSET") {
            // LIMIT 10 OFFSET 5 — OFFSET 在数字之后
            let offset_str = after_limit[offset_pos + 7..].trim();
            if let Ok(n) = offset_str.split_whitespace().next().unwrap_or("0").parse::<u64>() {
                offset = Some(n);
            }
        }
    } else if let Some(pos) = remaining_upper.find(" LIMIT ") {
        let limit_str = remaining[pos + 7..].trim();
        if let Ok(n) = limit_str.split_whitespace().next().unwrap_or("0").parse::<u64>() {
            limit = Some(n);
        }
        let after_limit = remaining[pos + 7..].trim();
        if let Some(offset_pos) = after_limit.to_uppercase().find(" OFFSET ") {
            let offset_str = after_limit[offset_pos + 8..].trim();
            if let Ok(n) = offset_str.split_whitespace().next().unwrap_or("0").parse::<u64>() {
                offset = Some(n);
            }
        }
    }

    // OFFSET 单独出现（无 LIMIT）：SELECT * FROM t OFFSET 5
    if offset.is_none() {
        let ru = remaining.to_uppercase();
        // OFFSET 可能在开头（如 "OFFSET 5"）或中间（如 " OFFSET 5"）
        if ru.starts_with("OFFSET ") {
            let offset_str = remaining[7..].trim();
            if let Ok(n) = offset_str.split_whitespace().next().unwrap_or("0").parse::<u64>() {
                offset = Some(n);
            }
        } else if let Some(pos) = ru.find(" OFFSET ") {
            let offset_str = remaining[pos + 8..].trim();
            if let Ok(n) = offset_str.split_whitespace().next().unwrap_or("0").parse::<u64>() {
                offset = Some(n);
            }
        }
    }

    Ok(SQLStatement::Select {
        table_name: table_name.to_lowercase(),
        columns,
        where_clause,
        order_by,
        group_by,
        having,
        limit,
        offset,
        distinct,
        join,
    })
}

/// 解析 DELETE 语句
/// DELETE FROM table WHERE condition
/// 在引号外查找关键字（大小写不敏感），返回字节起点。
///
/// 用于从 UPDATE 的赋值区里定位真正的 ` WHERE `——字符串里的
/// "where" 不能被当成子句边界。
fn find_keyword_outside_quotes(s: &str, keyword: &str) -> Option<usize> {
    let mut in_quote = false;
    let mut iter = s.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        if in_quote {
            if c == '\'' {
                // '' 转义：跳过第二个，仍在字符串内
                if let Some(&(_, '\'')) = iter.peek() {
                    iter.next();
                    continue;
                }
                in_quote = false;
            }
            continue;
        }
        if c == '\'' {
            in_quote = true;
            continue;
        }
        // get() 返回 None 表示 i+len 不在字符边界，直接跳过
        if let Some(candidate) = s.get(i..i + keyword.len()) {
            if candidate.eq_ignore_ascii_case(keyword) {
                return Some(i);
            }
        }
    }
    None
}

/// 在引号外按分隔符切分（支持 SQL 标准 `''` 转义），返回原串切片。
fn split_outside_quotes(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_quote = false;
    let mut iter = s.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        if in_quote {
            if c == '\'' {
                if let Some(&(_, '\'')) = iter.peek() {
                    iter.next();
                    continue;
                }
                in_quote = false;
            }
            continue;
        }
        if c == '\'' {
            in_quote = true;
            continue;
        }
        if c == sep {
            parts.push(&s[start..i]);
            start = i + c.len_utf8();
        }
    }
    parts.push(&s[start..]);
    parts
}

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
    } else if let Some(pos) = find_keyword_outside_quotes(&remaining, " WHERE ") {
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

    // 解析赋值列表到 WHERE 或结尾（WHERE 必须在引号外）
    let where_pos = find_keyword_outside_quotes(&after_set, " WHERE ");
    let assign_part = if let Some(pos) = where_pos {
        after_set[..pos].trim().to_string()
    } else {
        after_set.trim().to_string()
    };

    // 解析 "col1=val1, col2=val2"（值里的逗号不能当分隔符）
    let mut assignments = Vec::new();
    for part in split_outside_quotes(&assign_part, ',') {
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

/// 解析 DROP TABLE 语句
/// DROP TABLE table_name
fn parse_drop_table(sql: &str) -> Result<SQLStatement, String> {
    let s = sql.trim();
    let rest = s.strip_prefix("DROP TABLE ")
        .or_else(|| s.strip_prefix("drop table "))
        .ok_or_else(|| "无法解析DROP TABLE".to_string())?
        .trim();

    // 支持 IF EXISTS 可选子句
    let rest = if rest.to_uppercase().starts_with("IF EXISTS ") {
        rest[10..].trim()
    } else {
        rest
    };

    if rest.is_empty() {
        return Err("DROP TABLE 缺少表名".to_string());
    }

    let table_name = rest.split_whitespace().next().unwrap().to_lowercase();
    Ok(SQLStatement::DropTable { table_name })
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

/// 解析表引用 `table [alias]`，返回 (表名, 别名之后的剩余 SQL)。
///
/// 列限定名（`e.id` / `dh.entry_id`）在投影/WHERE/ORDER/ON 解析时已按 `.` 前缀剥离，
/// 因此别名只需在表名解析阶段被识别并丢弃，避免出现"表 'decay_history dh' 不存在"。
fn split_table_ref(s: &str) -> Result<(String, &str), String> {
    let (table, rest) = split_first_word(s)?;
    let table = table.to_lowercase();
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok((table, rest));
    }
    // 第二个 token 若为子句关键字，则不是别名
    let (next, after_next) = split_first_word(rest)?;
    let next_upper = next.to_uppercase();
    const KEYWORDS: [&str; 14] = [
        "INNER", "LEFT", "RIGHT", "FULL", "CROSS", "JOIN", "ON", "USING", "WHERE",
        "GROUP", "HAVING", "ORDER", "LIMIT", "OFFSET",
    ];
    if KEYWORDS.contains(&next_upper.as_str()) {
        Ok((table, rest))
    } else {
        Ok((table, after_next.trim()))
    }
}

/// 拆分括号内的内容和剩余部分
fn split_parenthesized(s: &str) -> Result<(String, String), String> {
    let s = s.trim();
    if !s.starts_with('(') {
        return Err("期望左括号".to_string());
    }
    let mut depth = 0;
    let content_start = 1;
    let mut content_end = 0;
    let mut found = false;
    // 字符串内部的括号不参与配对（如 VALUES ('a ( b')）
    let mut in_quote = false;

    let mut iter = s[1..].char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        if in_quote {
            if c == '\'' {
                // '' 为转义引号：跳过第二个，字符串继续
                if let Some(&(_, '\'')) = iter.peek() {
                    iter.next();
                    continue;
                }
                in_quote = false;
            }
            continue;
        }
        match c {
            '\'' => in_quote = true,
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            ')' => {
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

    // 分割逗号，注意引号内的逗号；'' 为转义引号，保留原文交给值解析层还原
    let mut values = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;

    let chars: Vec<char> = content.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if in_quote {
            current.push(c);
            if c == '\'' {
                if i + 1 < chars.len() && chars[i + 1] == '\'' {
                    current.push(chars[i + 1]); // 保留 ''，由 unquote 还原为 '
                    i += 2;
                    continue;
                }
                in_quote = false;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' => {
                current.push(c);
                in_quote = true;
            }
            ',' => {
                values.push(current.trim().to_string());
                current = String::new();
            }
            _ => current.push(c),
        }
        i += 1;
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
            SQLStatement::CreateTable { name, columns, .. } => {
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
    fn test_parse_create_table_boolean_date_time() {
        let stmts = parse_sql(
            "CREATE TABLE events (id INTEGER, day DATE, at TIME, done BOOLEAN, active BOOL)",
        ).unwrap();
        match &stmts[0] {
            SQLStatement::CreateTable { columns, .. } => {
                assert_eq!(columns[1].data_type, DataType::Date);
                assert_eq!(columns[2].data_type, DataType::Time);
                assert_eq!(columns[3].data_type, DataType::Boolean);
                assert_eq!(columns[4].data_type, DataType::Boolean, "BOOL 是 BOOLEAN 别名");
            }
            _ => panic!("Expected CreateTable"),
        }
    }

    #[test]
    fn test_parse_create_table_datetime_is_not_date() {
        // 精确匹配保护：DATETIME 未被 DATE 前缀吞掉，明确报错而不是静默当 DATE 用
        let err = parse_sql("CREATE TABLE t (id INTEGER, ts DATETIME)").unwrap_err();
        assert!(err.contains("不支持的数据类型"), "实际错误: {}", err);
        let err2 = parse_sql("CREATE TABLE t (id INTEGER, ts TIMESTAMP)").unwrap_err();
        assert!(err2.contains("不支持的数据类型"), "实际错误: {}", err2);
    }

    #[test]
    fn test_parse_select_with_new_type_filters() {
        let stmts = parse_sql(
            "SELECT * FROM events WHERE day > '2026-01-01' AND done = true",
        ).unwrap();
        match &stmts[0] {
            SQLStatement::Select { where_clause, .. } => {
                let wc = where_clause.as_ref().unwrap();
                assert!(wc.contains("day > '2026-01-01'"));
                assert!(wc.contains("done = true"));
            }
            _ => panic!("Expected Select"),
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

    // ===== SQL 字符串字面量健壮性（引号 / 分号 / 括号）=====

    #[test]
    fn test_split_statements_ignores_semicolon_in_string() {
        let stmts = parse_sql(
            "INSERT INTO t (id, note) VALUES (1, 'a;b'); SELECT id FROM t"
        ).unwrap();
        assert_eq!(stmts.len(), 2, "字符串里的分号不应拆句");
        match &stmts[0] {
            SQLStatement::Insert { values, .. } => {
                assert_eq!(values[0][1], "'a;b'");
            }
            _ => panic!("Expected Insert"),
        }
    }

    #[test]
    fn test_split_parenthesized_ignores_parens_in_string() {
        // 字符串里的括号（含不配对）不影响括号配对
        let stmts = parse_sql(
            "INSERT INTO t (id, note) VALUES (1, 'a ( b ) c')"
        ).unwrap();
        match &stmts[0] {
            SQLStatement::Insert { values, .. } => {
                assert_eq!(values[0][1], "'a ( b ) c'");
            }
            _ => panic!("Expected Insert"),
        }

        let stmts = parse_sql(
            "INSERT INTO t (id, note) VALUES (1, 'unbalanced ( here')"
        ).unwrap();
        match &stmts[0] {
            SQLStatement::Insert { values, .. } => {
                assert_eq!(values[0][1], "'unbalanced ( here'");
            }
            _ => panic!("Expected Insert"),
        }
    }

    #[test]
    fn test_parse_insert_without_column_list() {
        let stmts = parse_sql("INSERT INTO t VALUES (1, 'alice')").unwrap();
        match &stmts[0] {
            SQLStatement::Insert { columns, values, .. } => {
                assert!(columns.is_empty(), "省略列清单时 columns 应为空");
                assert_eq!(values[0], vec!["1".to_string(), "'alice'".to_string()]);
            }
            _ => panic!("Expected Insert"),
        }
    }

    #[test]
    fn test_parse_escaped_quote_kept_for_unquote() {
        // 解析层保留 '' 原样，由存储层 unquote 还原为单个 '
        let stmts = parse_sql(
            "INSERT INTO t (id, note) VALUES (1, 'It''s ok')"
        ).unwrap();
        match &stmts[0] {
            SQLStatement::Insert { values, .. } => {
                assert_eq!(values[0][1], "'It''s ok'");
            }
            _ => panic!("Expected Insert"),
        }
    }

    #[test]
    fn test_update_value_containing_comma_not_split() {
        // 赋值值里的逗号不能当作赋值分隔符
        let stmts = parse_sql(
            "UPDATE t SET note = 'a, b, c', name = 'x' WHERE id = 1"
        ).unwrap();
        match &stmts[0] {
            SQLStatement::Update { assignments, where_clause, .. } => {
                assert_eq!(assignments.len(), 2, "应解析出 2 个赋值，实际: {:?}", assignments);
                assert_eq!(assignments[0].0, "note");
                assert_eq!(assignments[0].1, "'a, b, c'");
                assert_eq!(assignments[1].0, "name");
                assert_eq!(where_clause.as_deref(), Some("id = 1"));
            }
            _ => panic!("Expected Update"),
        }
    }

    #[test]
    fn test_update_where_keyword_inside_string() {
        // 字符串里的 "WHERE" 不是子句边界
        let stmts = parse_sql(
            "UPDATE t SET note = 'where are you' WHERE id = 2"
        ).unwrap();
        match &stmts[0] {
            SQLStatement::Update { assignments, where_clause, .. } => {
                assert_eq!(assignments.len(), 1);
                assert_eq!(assignments[0].1, "'where are you'");
                assert_eq!(where_clause.as_deref(), Some("id = 2"));
            }
            _ => panic!("Expected Update"),
        }
    }

    // ===== SelfMind 兼容：注释 / IF NOT EXISTS / 表级约束 / CREATE INDEX / 表别名 =====

    #[test]
    fn test_parse_sql_skips_line_comments() {
        let stmts = parse_sql(
            "-- 建表注释\nCREATE TABLE t (id INTEGER); -- 尾部注释\nSELECT id FROM t"
        ).unwrap();
        assert_eq!(stmts.len(), 2);
        assert!(matches!(stmts[0], SQLStatement::CreateTable { .. }));
        assert!(matches!(stmts[1], SQLStatement::Select { .. }));
    }

    #[test]
    fn test_parse_create_table_if_not_exists() {
        let stmts = parse_sql("CREATE TABLE IF NOT EXISTS t (id INTEGER)").unwrap();
        match &stmts[0] {
            SQLStatement::CreateTable { name, if_not_exists, .. } => {
                assert_eq!(name, "t");
                assert!(*if_not_exists);
            }
            _ => panic!("Expected CreateTable"),
        }
    }

    #[test]
    fn test_parse_create_table_skips_table_constraints() {
        let stmts = parse_sql(
            "CREATE TABLE t (id INTEGER, entry_id TEXT, FOREIGN KEY (entry_id) REFERENCES other(id))"
        ).unwrap();
        match &stmts[0] {
            SQLStatement::CreateTable { columns, .. } => {
                assert_eq!(columns.len(), 2, "表级 FOREIGN KEY 应被跳过: {:?}", columns);
                assert_eq!(columns[1].name, "entry_id");
            }
            _ => panic!("Expected CreateTable"),
        }
    }

    #[test]
    fn test_parse_create_index() {
        let stmts = parse_sql(
            "CREATE INDEX IF NOT EXISTS idx_entries_type ON entries(type)"
        ).unwrap();
        match &stmts[0] {
            SQLStatement::CreateIndex { name, table, columns, if_not_exists } => {
                assert_eq!(name, "idx_entries_type");
                assert_eq!(table, "entries");
                assert_eq!(columns, &vec!["type".to_string()]);
                assert!(*if_not_exists);
            }
            _ => panic!("Expected CreateIndex"),
        }
    }

    #[test]
    fn test_parse_join_with_table_aliases() {
        let stmts = parse_sql(
            "SELECT e.id FROM entries e INNER JOIN decay_history dh ON dh.entry_id = e.id"
        ).unwrap();
        match &stmts[0] {
            SQLStatement::Select { table_name, join, .. } => {
                assert_eq!(table_name, "entries");
                let jc = join.as_ref().expect("应有 JOIN");
                assert_eq!(jc.table, "decay_history");
                assert_eq!(jc.left_col, "dh.entry_id");
                assert_eq!(jc.right_col, "e.id");
            }
            _ => panic!("Expected Select"),
        }
    }
}