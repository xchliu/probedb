// ProbeDB 执行器 — 将解析后的 SQL 语句转换为存储操作

use crate::sql::SQLStatement;
use crate::storage::*;
use crate::types::Value;
use crate::types::cosine_similarity;

/// 执行结果
#[derive(Debug)]
pub enum ExecuteResult {
    TableCreated { name: String },
    Inserted { row_id: u64 },
    Deleted { count: usize },
    Updated { count: usize },
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

            SQLStatement::Delete { table_name, where_clause } => {
                let schema = self.engine.get_schema(&table_name)?;
                let rows = self.engine.scan_table(&table_name)?.clone();

                let matched = if let Some(ref condition) = where_clause {
                    filter_rows(&rows, condition, &schema)?
                } else {
                    rows
                };

                let ids: Vec<u64> = matched.iter().map(|r| r.id).collect();
                let count = self.engine.delete_by_ids(&table_name, &ids)?;
                Ok(ExecuteResult::Deleted { count })
            }

            SQLStatement::Update { table_name, assignments, where_clause } => {
                let schema = self.engine.get_schema(&table_name)?.clone();
                let rows = self.engine.scan_table(&table_name)?.clone();

                let matched = if let Some(ref condition) = where_clause {
                    filter_rows(&rows, condition, &schema)?
                } else {
                    rows
                };

                let ids: Vec<u64> = matched.iter().map(|r| r.id).collect();
                let mut total_updated = 0;

                for (col_name, val_str) in &assignments {
                    let col = schema.columns.iter()
                        .find(|c| c.name == *col_name)
                        .ok_or_else(|| format!("列 '{}' 不存在", col_name))?;
                    let value = parse_value(val_str, &col.data_type)
                        .map_err(|e| format!("解析更新值 '{}' 失败: {}", val_str, e))?;
                    let count = self.engine.update_by_ids(&table_name, &ids, col.index, value)?;
                    total_updated += count;
                }

                let col_count = assignments.len();
                Ok(ExecuteResult::Updated {
                    count: total_updated / col_count.max(1),
                })
            }

            SQLStatement::Select { table_name, columns: _, where_clause, order_by, limit } => {
                let schema = self.engine.get_schema(&table_name)?;
                let col_names: Vec<String> = schema.columns.iter().map(|c| c.name.clone()).collect();

                // 扫描全表
                let rows = self.engine.scan_table(&table_name)?.clone();

                // 检查是否需要计算向量相似度（用于 ORDER BY）
                let vector_sim_order = parse_order_by_vector_call(order_by.as_deref());

                // 预先计算 vector_similarity 得分（如果 ORDER BY 需要）
                let mut scored_rows: Vec<(Row, Option<f64>)> = if let Some(ref vs) = vector_sim_order {
                    rows.into_iter().map(|row| {
                        let score = compute_vector_similarity(&row, &vs.col_name, &vs.target, &schema);
                        (row, score)
                    }).collect()
                } else {
                    rows.into_iter().map(|r| (r, None)).collect()
                };

                // WHERE 过滤（对带有 vector_similarity 调用的条件做原生求值）
                if let Some(ref condition) = where_clause {
                    scored_rows = filter_rows_with_scores(scored_rows, condition, &schema)?;
                }

                // 提取原始行（过滤后的）
                let mut matched: Vec<Row> = scored_rows.iter().map(|(r, _)| r.clone()).collect();

                // 构建列名索引
                let col_indices: Vec<usize> = schema.columns.iter().map(|c| c.index).collect();

                // ORDER BY
                let mut sorted: Vec<Row> = if let Some(ref order_by_str) = order_by {
                    let (order_col, descending) = parse_order_by_str(order_by_str);

                    // 检查是否是 vector_similarity 排序
                    if let Some(ref vs) = vector_sim_order {
                        // 用预先计算的相似度排序
                        scored_rows.sort_by(|a, b| {
                            let sa = a.1.unwrap_or(0.0);
                            let sb = b.1.unwrap_or(0.0);
                            if descending { sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal) }
                            else { sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal) }
                        });
                        scored_rows.iter().map(|(r, _)| r.clone()).collect()
                    } else {
                        sort_rows(&matched, &order_col, descending, &schema)?
                    }
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

// ===== 向量相似度原生函数支持 =====

/// 解析 vector_similarity(col, target) 函数调用字符串
struct VectorSimilarityCall {
    col_name: String,
    target: Vec<f64>,
}

/// 尝试解析 "vector_similarity(embedding, '[0.1,0.2,0.3]')" 格式
fn parse_vector_similarity_call(text: &str) -> Option<VectorSimilarityCall> {
    let text = text.trim();
    let upper = text.to_uppercase();

    // 必须以 vector_similarity( 开头
    if !upper.starts_with("VECTOR_SIMILARITY(") {
        return None;
    }

    // 提取括号内的内容
    let args_start = text.find('(')? + 1;
    let rest = &text[args_start..];
    let mut depth = 0;
    let mut args_end = 0;
    for (i, c) in rest.char_indices() {
        if c == '(' { depth += 1; }
        else if c == ')' {
            if depth == 0 { args_end = i; break; }
            else { depth -= 1; }
        }
    }
    if args_end == 0 { return None; }

    let args_str = rest[..args_end].trim();

    // 按逗号分割参数（不在括号/方括号/引号内的逗号）
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth_paren = 0;
    let mut depth_bracket = 0;
    let mut in_quote = false;
    for c in args_str.chars() {
        match c {
            '(' => { depth_paren += 1; current.push(c); }
            ')' if depth_paren > 0 => { depth_paren -= 1; current.push(c); }
            '[' => { depth_bracket += 1; current.push(c); }
            ']' if depth_bracket > 0 => { depth_bracket -= 1; current.push(c); }
            '\'' => { in_quote = !in_quote; current.push(c); }
            ',' if depth_paren == 0 && depth_bracket == 0 && !in_quote => {
                args.push(current.trim().to_string());
                current = String::new();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        args.push(current.trim().to_string());
    }

    if args.len() != 2 {
        return None;
    }

    let col_name = args[0].trim().to_lowercase();
    let target_str = args[1].trim().trim_matches('\'');

    // 解析向量 [1.0,2.0,3.0]
    let trimmed = target_str.trim_matches('[').trim_matches(']');
    let nums: Result<Vec<f64>, _> = trimmed.split(',')
        .map(|s| s.trim().parse::<f64>())
        .collect();
    let target = nums.ok()?;

    if target.is_empty() {
        return None;
    }

    Some(VectorSimilarityCall { col_name, target })
}

/// 计算某行的向量相似度（如果该列存在且是向量类型）
fn compute_vector_similarity(row: &Row, col_name: &str, target: &[f64], schema: &TableSchema) -> Option<f64> {
    let ci = schema.columns.iter().find(|c| c.name == col_name)?;
    let val = row.values.get(ci.index)?;
    match val {
        Value::Vector(v) => Some(cosine_similarity(v, target)),
        _ => None,
    }
}

/// 解析 ORDER BY 中的 vector_similarity 调用
fn parse_order_by_vector_call(order_by: Option<&str>) -> Option<VectorSimilarityCall> {
    let text = order_by?;
    let text = text.trim();
    // 去掉末尾的 ASC/DESC
    let upper = text.to_uppercase();
    let func_text = if upper.ends_with(" DESC") {
        &text[..text.len() - 5]
    } else if upper.ends_with(" ASC") {
        &text[..text.len() - 4]
    } else {
        text
    };
    parse_vector_similarity_call(func_text)
}

/// 解析 ORDER BY 字符串为 (列名, 是否降序)
fn parse_order_by_str(order_by: &str) -> (String, bool) {
    let order_by = order_by.trim();
    let upper = order_by.to_uppercase();
    if let Some(pos) = upper.rfind(" DESC") {
        (order_by[..pos].trim().to_string(), true)
    } else if let Some(pos) = upper.rfind(" ASC") {
        (order_by[..pos].trim().to_string(), false)
    } else {
        (order_by.to_string(), false)
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

/// 求值单个条件（支持普通列比较 + vector_similarity 原生函数）
fn eval_condition(condition: &str, row: &Row, schema: &TableSchema) -> Result<bool, String> {
    let c = condition.trim();

    // 检查是否是 vector_similarity() 条件
    let upper_c = c.to_uppercase();
    if upper_c.starts_with("VECTOR_SIMILARITY(") {
        return eval_vector_similarity_condition(c, row, schema);
    }

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
            // 确保不是单词中间（比如"name"里的"= "不会有）
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

/// 求值 vector_similarity() 条件（原生函数调用）
/// 语法: vector_similarity(col_name, '[1.0,2.0,3.0]') > 0.8
fn eval_vector_similarity_condition(condition: &str, row: &Row, schema: &TableSchema) -> Result<bool, String> {
    let c = condition.trim();

    // 找到操作符位置（在函数的右括号之后）
    let ops = [">=", "<=", "!=", "=", ">", "<"];
    // 先找到右括号的位置
    let func_end = {
        let mut depth = 0;
        let mut end = 0;
        for (i, ch) in c.char_indices() {
            if ch == '(' { depth += 1; }
            else if ch == ')' {
                depth -= 1;
                if depth == 0 { end = i + 1; break; }
            }
        }
        if end == 0 { return Err(format!("函数调用括号不匹配: {}", c)); }
        end
    };

    let func_call = &c[..func_end];
    let rest = c[func_end..].trim();

    let vs = parse_vector_similarity_call(func_call)
        .ok_or_else(|| format!("无法解析 vector_similarity 调用: {}", func_call))?;

    // 计算相似度
    let sim = compute_vector_similarity(row, &vs.col_name, &vs.target, schema)
        .ok_or_else(|| format!("列 '{}' 不是 VECTOR 类型或不存在", vs.col_name))?;

    // 解析操作符和阈值
    let mut found_op = "";
    let mut op_start = None;
    for op in &ops {
        if let Some(pos) = rest.find(op) {
            op_start = Some(pos);
            found_op = op;
            break;
        }
    }

    let pos = op_start.ok_or_else(|| format!("vector_similarity 条件缺少比较操作符: {}", c))?;
    let threshold_str = rest[pos + found_op.len()..].trim();
    let threshold: f64 = threshold_str.parse::<f64>()
        .map_err(|_| format!("无法解析相似度阈值: {}", threshold_str))?;

    let result = match found_op {
        ">"  => sim > threshold,
        ">=" => sim >= threshold,
        "<"  => sim < threshold,
        "<=" => sim <= threshold,
        "="  => (sim - threshold).abs() < 1e-10,
        "!=" => (sim - threshold).abs() >= 1e-10,
        _ => return Err(format!("不支持的操作符: {}", found_op)),
    };

    Ok(result)
}

/// 带得分的行过滤（支持 vector_similarity 原生函数）
fn filter_rows_with_scores(
    rows: Vec<(Row, Option<f64>)>,
    where_clause: &str,
    schema: &TableSchema,
) -> Result<Vec<(Row, Option<f64>)>, String> {
    // 按 OR 分割
    let or_parts: Vec<&str> = split_top_level(where_clause, " OR ");

    let mut all_matched = Vec::new();
    for or_part in &or_parts {
        // 按 AND 分割
        let and_parts: Vec<&str> = split_top_level(or_part, " AND ");

        let mut or_matched = Vec::new();
        for (row, score) in &rows {
            let all_true = and_parts.iter().all(|cond| {
                eval_condition(cond, row, schema).unwrap_or(false)
            });
            if all_true {
                or_matched.push((row.clone(), *score));
            }
        }
        all_matched.extend(or_matched);
    }

    // 去重
    all_matched.sort_by_key(|(r, _)| r.id);
    all_matched.dedup_by_key(|(r, _)| r.id);

    Ok(all_matched)
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
    order_col: &str,
    descending: bool,
    schema: &TableSchema,
) -> Result<Vec<Row>, String> {
    let mut sorted = rows.to_vec();

    // 获取列索引
    let ci = schema.columns.iter().find(|c| c.name == *order_col)
        .ok_or_else(|| format!("排序列 '{}' 不存在", order_col))?;
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

// ===== 测试 =====

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

    #[test]
    fn test_delete_with_where() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (1, 'alice')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (2, 'bob')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (3, 'charlie')").unwrap()).unwrap();

        // DELETE WHERE
        let results = executor.execute(parse_sql("DELETE FROM t WHERE id = 1").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::Deleted { count } => assert_eq!(*count, 1),
            _ => panic!("Expected Deleted"),
        }

        // 验证剩余行
        let results = executor.execute(parse_sql("SELECT id FROM t ORDER BY id ASC").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0][0], "2");
                assert_eq!(rows[1][0], "3");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_delete_all() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id) VALUES (1)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id) VALUES (2)").unwrap()).unwrap();

        let results = executor.execute(parse_sql("DELETE FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::Deleted { count } => assert_eq!(*count, 2),
            _ => panic!("Expected Deleted"),
        }

        let results = executor.execute(parse_sql("SELECT id FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => assert_eq!(rows.len(), 0),
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_update_with_where() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT, age INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (1, 'alice', 30)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (2, 'bob', 25)").unwrap()).unwrap();

        // UPDATE WHERE
        let results = executor.execute(parse_sql("UPDATE t SET name = 'alice_updated', age = 31 WHERE id = 1").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::Updated { count } => assert_eq!(*count, 1),
            _ => panic!("Expected Updated"),
        }

        // 验证更新
        let results = executor.execute(parse_sql("SELECT id, name, age FROM t WHERE id = 1").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][1], "alice_updated");
                assert_eq!(rows[0][2], "31");
            }
            _ => panic!("Expected SelectResult"),
        }

        // 验证未被影响的行不变
        let results = executor.execute(parse_sql("SELECT id, name FROM t WHERE id = 2").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows[0][1], "bob");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_update_all() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (1, 'alice')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (2, 'bob')").unwrap()).unwrap();

        let results = executor.execute(parse_sql("UPDATE t SET name = 'updated'").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::Updated { count } => assert_eq!(*count, 2),
            _ => panic!("Expected Updated"),
        }

        // SELECT * FROM t 返回所有列，name 是第2列(index=1)
        let results = executor.execute(parse_sql("SELECT * FROM t ORDER BY id ASC").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows[0][1], "updated");
                assert_eq!(rows[1][1], "updated");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    // ===== 向量相似度测试 =====

    fn setup_vector_table(executor: &mut Executor) {
        executor.execute(parse_sql(
            "CREATE TABLE items (id INTEGER, name TEXT, embedding VECTOR(3))"
        ).unwrap()).unwrap();
        executor.execute(parse_sql(
            "INSERT INTO items (id, name, embedding) VALUES (1, 'apple', '[1.0,0.0,0.0]')"
        ).unwrap()).unwrap();
        executor.execute(parse_sql(
            "INSERT INTO items (id, name, embedding) VALUES (2, 'banana', '[0.0,1.0,0.0]')"
        ).unwrap()).unwrap();
        executor.execute(parse_sql(
            "INSERT INTO items (id, name, embedding) VALUES (3, 'cherry', '[0.0,0.0,1.0]')"
        ).unwrap()).unwrap();
        executor.execute(parse_sql(
            "INSERT INTO items (id, name, embedding) VALUES (4, 'date', '[0.9,0.1,0.0]')"
        ).unwrap()).unwrap();
        executor.execute(parse_sql(
            "INSERT INTO items (id, name, embedding) VALUES (5, 'elderberry', '[0.5,0.5,0.0]')"
        ).unwrap()).unwrap();
    }

    #[test]
    fn test_vector_similarity_where_filter() {
        let mut executor = Executor::new();
        setup_vector_table(&mut executor);

        // 查询与 [1.0, 0.0, 0.0] 相似度 > 0.9 的行
        let results = executor.execute(parse_sql(
            "SELECT id, name FROM items WHERE vector_similarity(embedding, '[1.0,0.0,0.0]') > 0.9"
        ).unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                // apple (相似度1.0) 和 date (相似度0.993) 应匹配
                assert_eq!(rows.len(), 2, "应与 [1,0,0] 相似度 > 0.9 的有 2 条（apple, date）");
                assert_eq!(rows[0][1], "apple");
                assert_eq!(rows[1][1], "date");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_vector_similarity_order_by() {
        let mut executor = Executor::new();
        setup_vector_table(&mut executor);

        // 按与 [1.0, 0.0, 0.0] 的相似度降序排列
        let results = executor.execute(parse_sql(
            "SELECT id, name FROM items ORDER BY vector_similarity(embedding, '[1.0,0.0,0.0]') DESC"
        ).unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 5);
                // 第一个应该是 apple（最相似于 [1,0,0]）
                assert_eq!(rows[0][1], "apple");
                // 第二个应该是 date（0.99+）
                assert_eq!(rows[1][1], "date");
                // 第三个应该是 elderberry（0.707）
                assert_eq!(rows[2][1], "elderberry");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_vector_similarity_mixed_query() {
        let mut executor = Executor::new();
        setup_vector_table(&mut executor);

        // 混合查询：WHERE 标量条件 + vector_similarity 阈值 + ORDER BY vector_similarity
        let results = executor.execute(parse_sql(
            "SELECT id, name FROM items WHERE id >= 3 AND vector_similarity(embedding, '[1.0,0.0,0.0]') > 0.5 ORDER BY vector_similarity(embedding, '[1.0,0.0,0.0]') DESC"
        ).unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                // id >= 3: cherry, date, elderberry
                // vector_similarity > 0.5: cherry(0.0不匹配), date(0.993匹配), elderberry(0.707匹配)
                // 按相似度降序: date, elderberry
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0][1], "date");
                assert_eq!(rows[1][1], "elderberry");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    // ===== 性能基线 =====

    fn setup_big_vector_table(executor: &mut Executor) {
        use std::time::Instant;
        use crate::types::cosine_similarity;

        executor.execute(parse_sql(
            "CREATE TABLE vectors (id INTEGER, label TEXT, embedding VECTOR(128))"
        ).unwrap()).unwrap();

        let start = Instant::now();
        for i in 0..1000 {
            let vals: Vec<String> = (0..128).map(|j| {
                format!("{:.4}", ((i as f64).sin() * (j as f64).cos() * 10.0).sin())
            }).collect();
            let vec_str = vals.join(",");
            let sql = format!(
                "INSERT INTO vectors (id, label, embedding) VALUES ({}, 'item_{}', '[{}]')",
                i, i, vec_str
            );
            executor.execute(parse_sql(&sql).unwrap()).unwrap();
        }
        let elapsed = start.elapsed();
        println!("[PERF] 插入 1000 行(128维): {:?} ({:.0} 行/秒)", elapsed, 1000.0 / elapsed.as_secs_f64());
    }

    #[test]
    fn test_vector_similarity_perf_baseline() {
        let mut executor = Executor::new();
        setup_big_vector_table(&mut executor);

        // 构建目标向量
        let target_values: Vec<String> = (0..128).map(|j| format!("{:.4}", (j as f64 * 0.1).sin())).collect();
        let target_str = target_values.join(",");

        // 1. 标量查询基线
        let start = std::time::Instant::now();
        for _ in 0..100 {
            executor.execute(parse_sql("SELECT id, label FROM vectors WHERE id > 500").unwrap()).unwrap();
        }
        let scalar_elapsed = start.elapsed();
        println!(
            "[PERF] 标量查询 (100次, WHERE id > 500): {:?} (平均 {:.2}µs/次)",
            scalar_elapsed,
            scalar_elapsed.as_micros() as f64 / 100.0
        );

        // 2. 向量相似度查询
        let query = format!(
            "SELECT id, label FROM vectors WHERE vector_similarity(embedding, '[{}]') > 0.0 ORDER BY vector_similarity(embedding, '[{}]') DESC LIMIT 10",
            target_str, target_str
        );
        let start = std::time::Instant::now();
        for _ in 0..10 {
            executor.execute(parse_sql(&query).unwrap()).unwrap();
        }
        let vec_elapsed = start.elapsed();
        println!(
            "[PERF] 向量相似度查询 (10次, 128维, 1000行, WHERE+ORDER BY): {:?} (平均 {:.2}ms/次)",
            vec_elapsed,
            vec_elapsed.as_micros() as f64 / 10000.0
        );

        // 3. 混合查询
        let mixed_query = format!(
            "SELECT id, label FROM vectors WHERE id >= 0 AND vector_similarity(embedding, '[{}]') > 0.5 ORDER BY vector_similarity(embedding, '[{}]') DESC LIMIT 5",
            target_str, target_str
        );
        let start = std::time::Instant::now();
        for _ in 0..10 {
            executor.execute(parse_sql(&mixed_query).unwrap()).unwrap();
        }
        let mixed_elapsed = start.elapsed();
        println!(
            "[PERF] 混合查询 (10次, 标量+向量+ORDER BY): {:?} (平均 {:.2}ms/次)",
            mixed_elapsed,
            mixed_elapsed.as_micros() as f64 / 10000.0
        );
    }
}
