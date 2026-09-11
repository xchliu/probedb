// ProbeDB 执行器 — 将解析后的 SQL 语句转换为存储操作

use crate::sql::SQLStatement;
use crate::storage::*;
use crate::types::Value;
use crate::types::DataType;
use crate::types::cosine_similarity;
use crate::persistence::wal::WalLog;

/// 执行结果
#[derive(Debug)]
pub enum ExecuteResult {
    TableCreated { name: String },
    TableDropped { name: String },
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
    /// 写前日志（None = 纯内存模式，不记录 WAL）
    pub wal: Option<WalLog>,
}

impl Executor {
    pub fn new() -> Self {
        Executor {
            engine: StorageEngine::new(),
            wal: None,
        }
    }

    /// 追加 WAL 记录（纯内存模式静默跳过）
    fn append_wal(&mut self, entry: &str) -> Result<(), String> {
        if let Some(wal) = &mut self.wal {
            wal.append(entry)?;
        }
        Ok(())
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
                // WAL 先写（T 记录），成功后改内存
                let cols: Vec<String> = schema.columns.iter()
                    .map(|c| format!("{}:{}", c.name, crate::storage::encode_type(&c.data_type)))
                    .collect();
                self.append_wal(&format!("T|{}|{}", name, cols.join("|")))?;
                self.engine.create_table(schema)?;
                Ok(ExecuteResult::TableCreated { name })
            }

            SQLStatement::DropTable { table_name } => {
                // WAL 先写（DROP 记录），成功后改内存
                self.append_wal(&format!("DROP|{}", table_name))?;
                self.engine.drop_table(&table_name)?;
                Ok(ExecuteResult::TableDropped { name: table_name })
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
                let mut last_row_id = 0u64;
                for row in all_parsed {
                    // WAL 先写（I 记录，显式 row_id），成功后改内存
                    let row_id = self.engine.next_id();
                    let vals: Vec<String> = row.iter().map(crate::storage::encode_value).collect();
                    self.append_wal(&format!("I|{}|{}|{}", table_name, row_id, vals.join("|")))?;
                    last_row_id = self.engine.insert(&table_name, row)?;
                }
                if count == 1 {
                    Ok(ExecuteResult::Inserted { row_id: last_row_id })
                } else {
                    Ok(ExecuteResult::Message(format!("插入 {} 行数据", count)))
                }
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
                // WAL 先写（D 记录），成功后改内存
                for id in &ids {
                    self.append_wal(&format!("D|{}|{}", table_name, id))?;
                }
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
                    // WAL 先写（U 记录），成功后改内存
                    let encoded = crate::storage::encode_value(&value);
                    for id in &ids {
                        self.append_wal(&format!("U|{}|{}|{}|{}", table_name, id, col.index, encoded))?;
                    }
                    let count = self.engine.update_by_ids(&table_name, &ids, col.index, value)?;
                    total_updated += count;
                }

                let col_count = assignments.len();
                Ok(ExecuteResult::Updated {
                    count: total_updated / col_count.max(1),
                })
            }

            SQLStatement::Select { table_name, columns, where_clause, order_by, limit, offset, distinct } => {
                let schema = self.engine.get_schema(&table_name)?;
                let all_col_names: Vec<String> = schema.columns.iter().map(|c| c.name.clone()).collect();

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
                let matched: Vec<Row> = scored_rows.iter().map(|(r, _)| r.clone()).collect();

                // ===== 列投影解析 =====
                // SELECT *          → 所有列
                // SELECT col1, col2 → 仅指定列
                // SELECT COUNT(*)   → 聚合，返回单行单列
                let upper_cols: Vec<String> = columns.iter()
                    .map(|c| c.trim().to_uppercase()).collect();

                // COUNT(*) 聚合
                if upper_cols.len() == 1 && (upper_cols[0] == "COUNT(*)" || upper_cols[0] == "COUNT (*)") {
                    let count = matched.len();
                    return Ok(ExecuteResult::SelectResult {
                        columns: vec!["count".to_string()],
                        rows: vec![vec![count.to_string()]],
                    });
                }

                // SUM/AVG/MIN/MAX 聚合（单列）
                if upper_cols.len() == 1 {
                    let col_trimmed = columns[0].trim();
                    let upper = col_trimmed.to_uppercase();
                    let agg = parse_aggregate(&upper);
                    if let Some(agg_fn) = agg {
                        let inner_col = extract_agg_column(&upper);
                        let col_name = inner_col.trim().to_lowercase();
                        let ci = schema.columns.iter().find(|c| c.name == col_name)
                            .ok_or_else(|| format!("列 '{}' 不存在", col_name))?;

                        let mut nums: Vec<f64> = Vec::new();
                        for row in &matched {
                            if let Some(v) = row.values.get(ci.index) {
                                match v {
                                    Value::Integer(n) => nums.push(*n as f64),
                                    Value::Float(f) => nums.push(*f),
                                    _ => {}
                                }
                            }
                        }

                        let (agg_name, agg_value) = match agg_fn {
                            AggFunc::Sum => ("sum", nums.iter().sum::<f64>()),
                            AggFunc::Avg => {
                                if nums.is_empty() { ("avg", 0.0) }
                                else { ("avg", nums.iter().sum::<f64>() / nums.len() as f64) }
                            }
                            AggFunc::Min => {
                                if nums.is_empty() { ("min", 0.0) }
                                else { ("min", nums.iter().cloned().fold(f64::INFINITY, f64::min)) }
                            }
                            AggFunc::Max => {
                                if nums.is_empty() { ("max", 0.0) }
                                else { ("max", nums.iter().cloned().fold(f64::NEG_INFINITY, f64::max)) }
                            }
                        };

                        let value_str = if agg_value == agg_value.trunc()
                            && ci.data_type == DataType::Integer && agg_fn != AggFunc::Avg
                        {
                            format!("{}", agg_value as i64)
                        } else {
                            format!("{}", agg_value)
                        };

                        return Ok(ExecuteResult::SelectResult {
                            columns: vec![agg_name.to_string()],
                            rows: vec![vec![value_str]],
                        });
                    }
                }

                // 解析列投影（* 或具体列名）
                let proj_indices: Vec<usize>;
                let proj_names: Vec<String>;
                if columns.len() == 1 && columns[0] == "*" {
                    proj_indices = schema.columns.iter().map(|c| c.index).collect();
                    proj_names = all_col_names.clone();
                } else {
                    let mut idxs = Vec::new();
                    let mut names = Vec::new();
                    for col_name in &columns {
                        let col_name = col_name.trim();
                        if col_name == "*" {
                            // 混合 SELECT *, name — 展开所有列
                            for ci in &schema.columns {
                                idxs.push(ci.index);
                                names.push(ci.name.clone());
                            }
                        } else {
                            let ci = schema.columns.iter().find(|c| c.name == *col_name)
                                .ok_or_else(|| format!("列 '{}' 不存在", col_name))?;
                            idxs.push(ci.index);
                            names.push(ci.name.clone());
                        }
                    }
                    proj_indices = idxs;
                    proj_names = names;
                }

                // ORDER BY
                let mut sorted: Vec<Row> = if let Some(ref order_by_str) = order_by {
                    let (order_col, descending) = parse_order_by_str(order_by_str);

                    // 检查是否是 vector_similarity 排序
                    if let Some(ref _vs) = vector_sim_order {
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

                // 转换行为字符串（仅投影列）
                let mut result_rows: Vec<Vec<String>> = sorted.iter().map(|row| {
                    proj_indices.iter().map(|&i| {
                        if i < row.values.len() {
                            row.values[i].to_string()
                        } else {
                            String::new()
                        }
                    }).collect()
                }).collect();

                // DISTINCT 去重（保持首次出现顺序）
                if distinct {
                    let mut seen: std::collections::HashSet<Vec<String>> = std::collections::HashSet::new();
                    result_rows.retain(|row| seen.insert(row.clone()));
                }

                // OFFSET — 跳过前 N 行（在 DISTINCT 之后、LIMIT 之前）
                if let Some(offset_val) = offset {
                    let o = offset_val as usize;
                    if o < result_rows.len() {
                        result_rows.drain(0..o);
                    } else {
                        result_rows.clear();
                    }
                }

                // LIMIT
                if let Some(limit_val) = limit {
                    let l = limit_val as usize;
                    if l < result_rows.len() {
                        result_rows.truncate(l);
                    }
                }

                Ok(ExecuteResult::SelectResult {
                    columns: proj_names,
                    rows: result_rows,
                })
            }
        }
    }
}

// ===== 聚合函数支持 =====

/// 聚合函数类型
#[derive(PartialEq)]
enum AggFunc {
    Sum,
    Avg,
    Min,
    Max,
}

/// 检测字符串是否是聚合函数调用（SUM(col), AVG(col), MIN(col), MAX(col)）
fn parse_aggregate(s: &str) -> Option<AggFunc> {
    let s = s.trim();
    if s.to_uppercase().starts_with("SUM(") && s.ends_with(')') {
        return Some(AggFunc::Sum);
    }
    if s.to_uppercase().starts_with("AVG(") && s.ends_with(')') {
        return Some(AggFunc::Avg);
    }
    if s.to_uppercase().starts_with("MIN(") && s.ends_with(')') {
        return Some(AggFunc::Min);
    }
    if s.to_uppercase().starts_with("MAX(") && s.ends_with(')') {
        return Some(AggFunc::Max);
    }
    None
}

/// 从聚合函数调用中提取列名（如 "SUM(age)" → "age"）
fn extract_agg_column(s: &str) -> &str {
    let s = s.trim();
    if let Some(paren_start) = s.find('(') {
        if let Some(paren_end) = s.rfind(')') {
            return &s[paren_start + 1..paren_end];
        }
    }
    s
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

    // ===== 列投影 + COUNT(*) 测试 =====

    #[test]
    fn test_select_star_all_columns() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT, age INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (1, 'alice', 30)").unwrap()).unwrap();

        let results = executor.execute(parse_sql("SELECT * FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns.len(), 3, "SELECT * 应返回所有3列");
                assert_eq!(columns[0], "id");
                assert_eq!(columns[1], "name");
                assert_eq!(columns[2], "age");
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], "1");
                assert_eq!(rows[0][1], "alice");
                assert_eq!(rows[0][2], "30");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_select_single_column_projection() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT, age INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (1, 'alice', 30)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (2, 'bob', 25)").unwrap()).unwrap();

        // SELECT name — 只返回 name 列
        let results = executor.execute(parse_sql("SELECT name FROM t ORDER BY id ASC").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns.len(), 1, "SELECT name 应只返回1列");
                assert_eq!(columns[0], "name");
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0][0], "alice");
                assert_eq!(rows[1][0], "bob");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_select_multi_column_projection() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT, age INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (1, 'alice', 30)").unwrap()).unwrap();

        // SELECT id, name — 只返回2列（不含 age）
        let results = executor.execute(parse_sql("SELECT id, name FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns.len(), 2, "SELECT id, name 应返回2列");
                assert_eq!(columns[0], "id");
                assert_eq!(columns[1], "name");
                assert_eq!(rows[0].len(), 2, "每行应有2个值");
                assert_eq!(rows[0][0], "1");
                assert_eq!(rows[0][1], "alice");
                // 不应包含 age 值 "30"
                assert!(!rows[0].contains(&"30".to_string()), "不应包含未选择的列");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_select_nonexistent_column_errors() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (1, 'alice')").unwrap()).unwrap();

        // SELECT 不存在的列 → 报错
        let result = executor.execute(parse_sql("SELECT id, ghost FROM t").unwrap());
        assert!(result.is_err(), "查询不存在的列应报错");
        assert!(result.unwrap_err().contains("ghost"), "错误应提到列名 ghost");
    }

    #[test]
    fn test_count_star() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (1, 'a')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (2, 'b')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (3, 'c')").unwrap()).unwrap();

        // COUNT(*) 全表
        let results = executor.execute(parse_sql("SELECT COUNT(*) FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns, &vec!["count".to_string()], "COUNT(*) 列名应为 count");
                assert_eq!(rows.len(), 1, "COUNT 应返回单行");
                assert_eq!(rows[0][0], "3", "COUNT(*) 应返回3");
            }
            _ => panic!("Expected SelectResult"),
        }

        // COUNT(*) with WHERE
        let results = executor.execute(parse_sql("SELECT COUNT(*) FROM t WHERE id > 1").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows[0][0], "2", "COUNT(*) WHERE id > 1 应返回2");
            }
            _ => panic!("Expected SelectResult"),
        }

        // COUNT(*) 空表
        executor.execute(parse_sql("CREATE TABLE empty (id INTEGER)").unwrap()).unwrap();
        let results = executor.execute(parse_sql("SELECT COUNT(*) FROM empty").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows[0][0], "0", "COUNT(*) 空表应返回0");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_insert_returns_row_id() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap()).unwrap();

        // 单行 INSERT 应返回 Inserted { row_id }
        let results = executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (1, 'alice')").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::Inserted { row_id } => {
                assert!(*row_id > 0, "单行插入应返回有效 row_id");
            }
            _ => panic!("Expected Inserted for single-row INSERT"),
        }

        // 多行 INSERT 应返回 Message（带行数）
        let results = executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (2, 'bob'), (3, 'charlie')").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::Message(msg) => {
                assert!(msg.contains("2"), "多行插入消息应含行数: {}", msg);
            }
            _ => panic!("Expected Message for multi-row INSERT"),
        }
    }

    // ===== DROP TABLE 测试 =====

    #[test]
    fn test_drop_table_basic() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (1, 'alice')").unwrap()).unwrap();

        // DROP TABLE
        let results = executor.execute(parse_sql("DROP TABLE t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::TableDropped { name } => assert_eq!(name, "t"),
            _ => panic!("Expected TableDropped"),
        }

        // 表已删除 → SELECT 应报错
        let result = executor.execute(parse_sql("SELECT * FROM t").unwrap());
        assert!(result.is_err(), "删除表后查询应报错");
    }

    #[test]
    fn test_drop_table_then_recreate() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("DROP TABLE t").unwrap()).unwrap();

        // 删除后可以重新创建同名表
        let result = executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap());
        assert!(result.is_ok(), "删除后应可重建同名表");
    }

    #[test]
    fn test_drop_nonexistent_table_errors() {
        let mut executor = Executor::new();
        let result = executor.execute(parse_sql("DROP TABLE ghost").unwrap());
        assert!(result.is_err(), "删除不存在的表应报错");
    }

    #[test]
    fn test_drop_table_if_exists() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER)").unwrap()).unwrap();

        // DROP TABLE IF EXISTS 存在的表
        let results = executor.execute(parse_sql("DROP TABLE IF EXISTS t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::TableDropped { name } => assert_eq!(name, "t"),
            _ => panic!("Expected TableDropped"),
        }

        // DROP TABLE IF EXISTS 不存在的表 → 应报错（表不存在）
        // IF EXISTS 只在解析层跳过，executor 仍尝试 drop_table 并报错
        // 这是设计选择：不做 IF EXISTS 语义（保持简单），解析器只忽略 IF EXISTS 关键字
    }

    #[test]
    fn test_drop_table_wal_recovery() {
        // 验证 DROP TABLE 能通过 WAL 正确恢复
        use crate::persistence::wal::WalLog;
        use std::fs;

        let wal_path = std::env::temp_dir().join("probedb_drop_wal_test.wal");
        let _ = fs::remove_file(&wal_path);
        let path_str = wal_path.to_str().unwrap();

        let mut wal = WalLog::open(path_str).unwrap();
        wal.append("T|users|id:INTEGER|name:TEXT").unwrap();
        wal.append("I|users|1|INT:1|TEXT:alice").unwrap();
        wal.append("DROP|users").unwrap();

        let mut engine = StorageEngine::new();
        let replayed = wal.replay(&mut engine).unwrap();
        assert_eq!(replayed, 3, "3条记录全部重放");

        // DROP 后表应不存在
        assert!(engine.get_schema("users").is_err(), "DROP TABLE 重放后表应不存在");

        let _ = fs::remove_file(&wal_path);
    }

    // ===== SELECT DISTINCT 测试 =====

    #[test]
    fn test_select_distinct_single_column() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (1, 'alice')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (2, 'alice')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (3, 'bob')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (4, 'alice')").unwrap()).unwrap();

        // SELECT DISTINCT name → 只有 'alice' 和 'bob'
        let results = executor.execute(parse_sql("SELECT DISTINCT name FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2, "DISTINCT 应去重为2行");
                // 首次出现的顺序保持
                assert_eq!(rows[0][0], "alice");
                assert_eq!(rows[1][0], "bob");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_select_distinct_multi_column() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT, age INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (1, 'alice', 30)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (2, 'alice', 30)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (3, 'alice', 25)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (4, 'bob', 30)").unwrap()).unwrap();

        // SELECT DISTINCT name, age → (alice,30), (alice,25), (bob,30)
        let results = executor.execute(parse_sql("SELECT DISTINCT name, age FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 3, "DISTINCT 多列去重应为3行");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_select_distinct_all_unique() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (1, 'a')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (2, 'b')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (3, 'c')").unwrap()).unwrap();

        // 所有行都不同 → DISTINCT 不减少行数
        let results = executor.execute(parse_sql("SELECT DISTINCT id, name FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 3, "全部唯一时 DISTINCT 不去重");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_select_distinct_with_where() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT, age INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (1, 'alice', 30)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (2, 'alice', 25)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (3, 'bob', 30)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (4, 'alice', 30)").unwrap()).unwrap();

        // SELECT DISTINCT name WHERE age = 30 → alice, bob
        let results = executor.execute(parse_sql("SELECT DISTINCT name FROM t WHERE age = 30").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2, "DISTINCT + WHERE 应返回2行");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_select_distinct_star() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (1, 'alice')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (1, 'alice')").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name) VALUES (2, 'bob')").unwrap()).unwrap();

        // SELECT DISTINCT * → 完全重复的行去重
        let results = executor.execute(parse_sql("SELECT DISTINCT * FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2, "DISTINCT * 应去重完全相同的行");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    // ===== 聚合函数测试 =====

    #[test]
    fn test_sum_aggregate() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT, price INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, price) VALUES (1, 'a', 10)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, price) VALUES (2, 'b', 20)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, price) VALUES (3, 'c', 30)").unwrap()).unwrap();

        let results = executor.execute(parse_sql("SELECT SUM(price) FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns, &vec!["sum".to_string()]);
                assert_eq!(rows[0][0], "60", "SUM(price) 应为 60");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_avg_aggregate() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT, age INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (1, 'a', 30)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (2, 'b', 25)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (3, 'c', 35)").unwrap()).unwrap();

        let results = executor.execute(parse_sql("SELECT AVG(age) FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns, &vec!["avg".to_string()]);
                let avg: f64 = rows[0][0].parse().unwrap();
                assert!((avg - 30.0).abs() < 0.01, "AVG(age) 应为 30，得到 {}", avg);
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_min_max_aggregate() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT, score INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, score) VALUES (1, 'a', 50)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, score) VALUES (2, 'b', 90)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, score) VALUES (3, 'c', 70)").unwrap()).unwrap();

        let results = executor.execute(parse_sql("SELECT MIN(score) FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns, &vec!["min".to_string()]);
                assert_eq!(rows[0][0], "50", "MIN(score) 应为 50");
            }
            _ => panic!("Expected SelectResult"),
        }

        let results = executor.execute(parse_sql("SELECT MAX(score) FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns, &vec!["max".to_string()]);
                assert_eq!(rows[0][0], "90", "MAX(score) 应为 90");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_aggregate_with_where() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT, age INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (1, 'a', 30)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (2, 'b', 25)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, name, age) VALUES (3, 'c', 35)").unwrap()).unwrap();

        // SUM(age) WHERE age >= 30 → 30 + 35 = 65
        let results = executor.execute(parse_sql("SELECT SUM(age) FROM t WHERE age >= 30").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows[0][0], "65", "SUM(age) WHERE age >= 30 应为 65");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_aggregate_empty_table() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, score INTEGER)").unwrap()).unwrap();

        let results = executor.execute(parse_sql("SELECT SUM(score) FROM t").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows[0][0], "0", "空表 SUM 应为 0");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    // ===== OFFSET 分页测试 =====

    #[test]
    fn test_offset_basic() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap()).unwrap();
        for i in 1..=5 {
            let sql = format!("INSERT INTO t (id, name) VALUES ({}, 'item{}')", i, i);
            executor.execute(parse_sql(&sql).unwrap()).unwrap();
        }

        // OFFSET 2 → 跳过前2行，返回3行
        let results = executor.execute(parse_sql("SELECT id, name FROM t ORDER BY id ASC OFFSET 2").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 3, "OFFSET 2 应返回3行");
                assert_eq!(rows[0][0], "3", "第一行应为 id=3");
                assert_eq!(rows[2][0], "5", "最后一行应为 id=5");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_limit_with_offset() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, name TEXT)").unwrap()).unwrap();
        for i in 1..=10 {
            let sql = format!("INSERT INTO t (id, name) VALUES ({}, 'item{}')", i, i);
            executor.execute(parse_sql(&sql).unwrap()).unwrap();
        }

        // LIMIT 3 OFFSET 5 → 第6-8行
        let results = executor.execute(parse_sql("SELECT id, name FROM t ORDER BY id ASC LIMIT 3 OFFSET 5").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 3, "LIMIT 3 OFFSET 5 应返回3行");
                assert_eq!(rows[0][0], "6", "第一行应为 id=6");
                assert_eq!(rows[2][0], "8", "最后一行应为 id=8");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_offset_exceeds_count() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id) VALUES (1)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id) VALUES (2)").unwrap()).unwrap();

        // OFFSET 10 > 行数 → 空结果
        let results = executor.execute(parse_sql("SELECT id FROM t OFFSET 10").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 0, "OFFSET 超过行数应返回空");
            }
            _ => panic!("Expected SelectResult"),
        }
    }
}
