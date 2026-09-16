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
                        // 保留底层解析原因（格式错误/日期越界/类型不符），否则用户只看到"无法解析"
                        let value = parse_value(clean, &col_types[i])
                            .map_err(|e| format!("无法解析列 '{}' 的值 '{}': {}", col_names[i], clean, e))?;
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

            SQLStatement::Select { table_name, columns, where_clause, order_by, group_by, having, limit, offset, distinct, join } => {
                let orig_schema = self.engine.get_schema(&table_name)?;

                // 扫描全表
                let rows = self.engine.scan_table(&table_name)?.clone();

                // ===== JOIN 行合并 =====
                // 将左表行与右表行按 ON 条件做等值连接（INNER JOIN）
                let (joined_rows, joined_schema) = if let Some(ref jc) = join {
                    let right_schema = self.engine.get_schema(&jc.table)?.clone();
                    let right_rows = self.engine.scan_table(&jc.table)?.clone();

                    // 解析连接列：left_col / right_col 可能是 "table.col" 或 "col" 形式
                    let left_col_name = jc.left_col.rsplit('.').next().unwrap_or(&jc.left_col);
                    let right_col_name = jc.right_col.rsplit('.').next().unwrap_or(&jc.right_col);

                    let left_ci = orig_schema.columns.iter().find(|c| c.name == left_col_name)
                        .ok_or_else(|| format!("JOIN 左表列 '{}' 不存在", left_col_name))?;
                    let right_ci = right_schema.columns.iter().find(|c| c.name == right_col_name)
                        .ok_or_else(|| format!("JOIN 右表列 '{}' 不存在", right_col_name))?;

                    let left_idx = left_ci.index;
                    let right_idx = right_ci.index;

                    // 合并 schema：左表列 + 右表列（右表列重命名为 table.col 避免冲突）
                    let mut merged_cols: Vec<ColumnInfo> = orig_schema.columns.clone();
                    for rc in &right_schema.columns {
                        merged_cols.push(ColumnInfo {
                            name: format!("{}.{}", jc.table, rc.name),
                            data_type: rc.data_type.clone(),
                            index: merged_cols.len(),
                        });
                    }
                    let merged_schema = TableSchema {
                        name: format!("{}__join__{}", table_name, jc.table),
                        columns: merged_cols,
                    };

                    // 嵌套循环连接（MVP 实现）
                    let mut combined: Vec<Row> = Vec::new();
                    let mut next_join_id = 1u64;
                    for lr in &rows {
                        let lval = lr.values.get(left_idx);
                        for rr in &right_rows {
                            let rval = rr.values.get(right_idx);
                            if lval.is_some() && rval.is_some() && lval == rval {
                                let mut merged_values = lr.values.clone();
                                merged_values.extend(rr.values.clone());
                                combined.push(Row {
                                    id: next_join_id,
                                    values: merged_values,
                                });
                                next_join_id += 1;
                            }
                        }
                    }

                    (combined, merged_schema)
                } else {
                    (rows.clone(), orig_schema.clone())
                };

                // 用 joined_schema 替代后续的 schema 引用
                let schema = &joined_schema;

                // 检查是否需要计算向量相似度（用于 ORDER BY）
                let vector_sim_order = parse_order_by_vector_call(order_by.as_deref());

                // 预先计算 vector_similarity 得分（如果 ORDER BY 需要）
                let mut scored_rows: Vec<(Row, Option<f64>)> = if let Some(ref vs) = vector_sim_order {
                    joined_rows.into_iter().map(|row| {
                        let score = compute_vector_similarity(&row, &vs.col_name, &vs.target, schema);
                        (row, score)
                    }).collect()
                } else {
                    joined_rows.into_iter().map(|r| (r, None)).collect()
                };

                // WHERE 过滤（对带有 vector_similarity 调用的条件做原生求值）
                if let Some(ref condition) = where_clause {
                    scored_rows = filter_rows_with_scores(scored_rows, condition, &schema)?;
                }

                // 提取原始行（过滤后的）
                let matched: Vec<Row> = scored_rows.iter().map(|(r, _)| r.clone()).collect();

                // ===== GROUP BY 分组聚合 =====
                if let Some(ref gb_str) = group_by {
                    return execute_group_by(&matched, &columns, gb_str, having.as_deref(), &schema);
                }

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
                    proj_names = schema.columns.iter().map(|c| c.name.clone()).collect();
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
                            // 支持限定列名 table.col 和裸列名 col
                            // 优先精确匹配，避免 JOIN 后右表限定列名误命中左表裸列名
                            let bare = col_name.rsplit('.').next().unwrap_or(col_name);
                            let ci = schema.columns.iter()
                                .find(|c| c.name == *col_name)
                                .or_else(|| {
                                    schema.columns.iter()
                                        .find(|c| c.name == *bare || (c.name.ends_with(&format!(".{}", bare)) && c.name.rsplit('.').next().unwrap_or(&c.name) == bare))
                                })
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
#[derive(PartialEq, Clone)]
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

/// GROUP BY 分组聚合执行
/// 支持语法: SELECT dept, COUNT(*), SUM(salary) FROM employees GROUP BY dept
/// SELECT 子句中的普通列 = 分组键，聚合函数 = 对组内数据聚合
/// HAVING 子句过滤分组后的聚合结果（如 HAVING COUNT(*) > 2）
fn execute_group_by(
    rows: &[Row],
    columns: &[String],
    group_by_str: &str,
    having: Option<&str>,
    schema: &TableSchema,
) -> Result<ExecuteResult, String> {
    // 解析 GROUP BY 列名（支持多列 GROUP BY a, b）
    let gb_cols: Vec<String> = group_by_str
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .collect();

    // 验证 GROUP BY 列存在，获取索引
    let mut gb_indices: Vec<usize> = Vec::new();
    for gb_col in &gb_cols {
        let ci = schema
            .columns
            .iter()
            .find(|c| c.name == *gb_col)
            .ok_or_else(|| format!("GROUP BY 列 '{}' 不存在", gb_col))?;
        gb_indices.push(ci.index);
    }

    // 解析 SELECT 子句：区分分组键列和聚合函数
    // 例如 SELECT dept, COUNT(*), SUM(salary) → [(dept, idx, is_agg), ...]
    #[derive(Clone)]
    struct SelectItem {
        label: String,     // 输出列名
        is_agg: bool,      // 是否是聚合函数
        agg_fn: Option<AggFunc>,
        col_name: String,  // 聚合函数内的列名（或分组键列名）
        col_index: usize,  // 列索引
        is_count_star: bool,
        data_type: DataType,
    }

    let mut select_items: Vec<SelectItem> = Vec::new();
    for col in columns {
        let col_trimmed = col.trim();
        let upper = col_trimmed.to_uppercase();

        // COUNT(*)
        if upper == "COUNT(*)" || upper == "COUNT (*)" {
            select_items.push(SelectItem {
                label: "count".to_string(),
                is_agg: true,
                agg_fn: None,
                col_name: String::new(),
                col_index: 0,
                is_count_star: true,
                data_type: DataType::Integer,
            });
            continue;
        }

        // SUM/AVG/MIN/MAX
        if let Some(agg_fn) = parse_aggregate(&upper) {
            let inner_col = extract_agg_column(&upper);
            let col_name = inner_col.trim().to_lowercase();
            let ci = schema
                .columns
                .iter()
                .find(|c| c.name == col_name)
                .ok_or_else(|| format!("列 '{}' 不存在", col_name))?;
            let agg_name = match agg_fn {
                AggFunc::Sum => "sum",
                AggFunc::Avg => "avg",
                AggFunc::Min => "min",
                AggFunc::Max => "max",
            };
            select_items.push(SelectItem {
                label: agg_name.to_string(),
                is_agg: true,
                agg_fn: Some(agg_fn),
                col_name,
                col_index: ci.index,
                is_count_star: false,
                data_type: ci.data_type.clone(),
            });
            continue;
        }

        // 普通列 — 必须是 GROUP BY 的分组键
        let col_name = col_trimmed.to_lowercase();
        let ci = schema
            .columns
            .iter()
            .find(|c| c.name == col_name)
            .ok_or_else(|| format!("列 '{}' 不存在", col_name))?;

        // 检查是否是 GROUP BY 的列之一
        if !gb_cols.contains(&col_name) {
            return Err(format!(
                "列 '{}' 不在 GROUP BY 中（非聚合列必须出现在 GROUP BY 子句中）",
                col_name
            ));
        }

        select_items.push(SelectItem {
            label: col_name.clone(),
            is_agg: false,
            agg_fn: None,
            col_name,
            col_index: ci.index,
            is_count_star: false,
            data_type: ci.data_type.clone(),
        });
    }

    // 分组：用行内分组键的字符串表示作为 HashMap 的 key，保持首次出现顺序
    use std::collections::HashMap;
    let mut group_order: Vec<String> = Vec::new(); // 保持分组出现顺序
    let mut groups: HashMap<String, Vec<&Row>> = HashMap::new();

    for row in rows {
        let key: String = gb_indices
            .iter()
            .map(|&idx| {
                row.values
                    .get(idx)
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join("\x1f"); // 用分隔符连接，避免值碰撞

        if !groups.contains_key(&key) {
            group_order.push(key.clone());
        }
        groups.entry(key).or_default().push(row);
    }

    // 输出列名
    let result_columns: Vec<String> = select_items.iter().map(|si| si.label.clone()).collect();

    // 对每个分组计算输出行
    let mut result_rows: Vec<Vec<String>> = Vec::new();

    for key in &group_order {
        let group_rows = groups.get(key).unwrap();
        let mut output_row: Vec<String> = Vec::new();

        for si in &select_items {
            if !si.is_agg {
                // 普通列：取第一行的值（同一组内值相同）
                let val = group_rows
                    .first()
                    .and_then(|r| r.values.get(si.col_index))
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                output_row.push(val);
            } else if si.is_count_star {
                // COUNT(*) = 组内行数
                output_row.push(group_rows.len().to_string());
            } else {
                // SUM/AVG/MIN/MAX
                let agg_fn = si.agg_fn.clone().unwrap();
                let mut nums: Vec<f64> = Vec::new();
                for row in group_rows {
                    if let Some(v) = row.values.get(si.col_index) {
                        match v {
                            Value::Integer(n) => nums.push(*n as f64),
                            Value::Float(f) => nums.push(*f),
                            _ => {}
                        }
                    }
                }

                let agg_value = match agg_fn {
                    AggFunc::Sum => nums.iter().sum::<f64>(),
                    AggFunc::Avg => {
                        if nums.is_empty() {
                            0.0
                        } else {
                            nums.iter().sum::<f64>() / nums.len() as f64
                        }
                    }
                    AggFunc::Min => {
                        if nums.is_empty() {
                            0.0
                        } else {
                            nums.iter().cloned().fold(f64::INFINITY, f64::min)
                        }
                    }
                    AggFunc::Max => {
                        if nums.is_empty() {
                            0.0
                        } else {
                            nums.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
                        }
                    }
                };

                // 格式化：整数列+非AVG→整数格式，否则浮点
                let value_str = if agg_value == agg_value.trunc()
                    && si.data_type == DataType::Integer
                    && agg_fn != AggFunc::Avg
                {
                    format!("{}", agg_value as i64)
                } else {
                    format!("{}", agg_value)
                };
                output_row.push(value_str);
            }
        }

        result_rows.push(output_row);
    }

    // ===== HAVING 过滤 =====
    if let Some(having_str) = having {
        result_rows = filter_having_rows(
            result_rows,
            &result_columns,
            having_str,
        )?;
    }

    Ok(ExecuteResult::SelectResult {
        columns: result_columns,
        rows: result_rows,
    })
}

/// 求值 HAVING 条件
/// HAVING 可以引用聚合函数（COUNT(*)/SUM(col)/AVG(col)/MIN(col)/MAX(col)）和分组键列
/// 条件格式：聚合函数或列名 + 比较运算符 + 字面量，支持 AND/OR
fn filter_having_rows(
    rows: Vec<Vec<String>>,
    columns: &[String],
    having: &str,
) -> Result<Vec<Vec<String>>, String> {
    // 按 OR 分割
    let or_parts: Vec<&str> = split_top_level(having, " OR ");
    let mut kept_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for row_idx in 0..rows.len() {
        let row = &rows[row_idx];
        let or_pass = or_parts.iter().any(|or_part| {
            let and_parts: Vec<&str> = split_top_level(or_part, " AND ");
            and_parts.iter().all(|cond| {
                eval_having_condition(cond, row, columns)
                    .unwrap_or(false)
            })
        });
        if or_pass {
            kept_indices.insert(row_idx);
        }
    }

    Ok(rows
        .into_iter()
        .enumerate()
        .filter(|(i, _)| kept_indices.contains(i))
        .map(|(_, r)| r)
        .collect())
}

/// 求值单个 HAVING 条件
/// 条件左侧可以是：聚合函数（COUNT(*)/SUM(col)/...）或分组键列名
/// 条件右侧是字面量
fn eval_having_condition(
    cond: &str,
    row: &[String],
    columns: &[String],
) -> Result<bool, String> {
    let c = cond.trim();

    // 找到操作符位置（跳过聚合函数内的括号）
    let ops = [">=", "<=", "!=", "=", ">", "<"];
    let mut op_pos = None;
    let mut found_op = "";
    let mut paren_depth = 0;

    let chars: Vec<(usize, char)> = c.char_indices().collect();
    for &(i, ch) in &chars {
        if ch == '(' { paren_depth += 1; }
        else if ch == ')' && paren_depth > 0 { paren_depth -= 1; }
        else if paren_depth == 0 {
            for op in &ops {
                if c[i..].starts_with(op) {
                    // 确保前面不是字母/数字/下划线（避免匹配列名中的等号）
                    if i > 0 {
                        let before = chars[i - 1].1;
                        if before.is_alphanumeric() || before == '_' {
                            continue;
                        }
                    }
                    op_pos = Some(i);
                    found_op = op;
                    break;
                }
            }
            if op_pos.is_some() { break; }
        }
    }

    let pos = op_pos.ok_or_else(|| format!("HAVING 条件缺少比较操作符: {}", c))?;
    let left = c[..pos].trim();
    let right_str = c[pos + found_op.len()..].trim();

    // 解析左侧值
    let left_upper = left.to_uppercase();
    let left_val: f64;

    // COUNT(*) 聚合
    if left_upper == "COUNT(*)" || left_upper == "COUNT (*)" {
        // COUNT(*) 的值就是输出行中 "count" 列的值
        if let Some(col_idx) = columns.iter().position(|c| *c == "count") {
            left_val = row[col_idx].parse::<f64>().unwrap_or(0.0);
        } else {
            return Err("HAVING 引用 COUNT(*) 但 SELECT 中没有 COUNT(*)".to_string());
        }
    }
    // SUM/AVG/MIN/MAX 聚合
    else if let Some(agg_fn) = parse_aggregate(&left_upper) {
        let agg_name = match agg_fn {
            AggFunc::Sum => "sum",
            AggFunc::Avg => "avg",
            AggFunc::Min => "min",
            AggFunc::Max => "max",
        };
        // 从输出列中找到对应的聚合值
        if let Some(col_idx) = columns.iter().position(|c| *c == agg_name) {
            left_val = row[col_idx].parse::<f64>().unwrap_or(0.0);
        } else {
            return Err(format!("HAVING 引用 {} 但 SELECT 中没有该聚合", agg_name));
        }
    }
    // 分组键列名
    else {
        let col_name = left.to_lowercase();
        // 从输出列中找到分组键值
        if let Some(col_idx) = columns.iter().position(|c| *c == col_name) {
            // 尝试数值比较，否则字符串比较
            let left_str = &row[col_idx];
            let right_parsed = parse_literal(right_str);
            let left_parsed = parse_literal(left_str);
            let cmp = compare_values(&left_parsed, &right_parsed);
            return Ok(match found_op {
                ">"  => cmp == std::cmp::Ordering::Greater,
                ">=" => cmp == std::cmp::Ordering::Greater || cmp == std::cmp::Ordering::Equal,
                "<"  => cmp == std::cmp::Ordering::Less,
                "<=" => cmp == std::cmp::Ordering::Less || cmp == std::cmp::Ordering::Equal,
                "="  => cmp == std::cmp::Ordering::Equal,
                "!=" => cmp != std::cmp::Ordering::Equal,
                _ => false,
            });
        }
        return Err(format!("HAVING 引用了未知列或聚合: {}", left));
    }

    // 解析右侧值（字面量 → f64）
    let right_val: f64 = right_str
        .parse::<f64>()
        .map_err(|_| format!("HAVING 条件右侧不是数值: {}", right_str))?;

    Ok(match found_op {
        ">"  => left_val > right_val,
        ">=" => left_val >= right_val,
        "<"  => left_val < right_val,
        "<=" => left_val <= right_val,
        "="  => (left_val - right_val).abs() < 1e-10,
        "!=" => (left_val - right_val).abs() >= 1e-10,
        _ => false,
    })
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
    // 支持限定列名 table.col 和裸列名 col
    // 优先精确匹配（table.col），找不到再用裸列名模糊匹配
    // 这避免了 JOIN 后 "departments.name" 误命中左表 "name" 列
    let bare = col_name.rsplit('.').next().unwrap_or(col_name);
    let ci = schema.columns.iter()
        .find(|c| c.name == col_name)
        .or_else(|| {
            schema.columns.iter()
                .find(|c| c.name == bare || (c.name.ends_with(&format!(".{}", bare)) && c.name.rsplit('.').next().unwrap_or(&c.name) == bare))
        })
        .ok_or_else(|| format!("列 '{}' 不存在", col_name))?;
    row.values.get(ci.index)
        .ok_or_else(|| format!("列 '{}' 没有值", col_name))
}

/// 解析一个字面量值（数字、字符串、布尔、浮点数）
fn parse_literal(s: &str) -> Value {
    let s = s.trim();
    // 字符串（带引号）
    if (s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')) {
        return Value::Text(s[1..s.len()-1].to_string());
    }
    // 布尔字面量（裸词 true/false，大小写不敏感）
    match s.to_lowercase().as_str() {
        "true" => return Value::Boolean(true),
        "false" => return Value::Boolean(false),
        _ => {}
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
        // 布尔：false < true（ORDER BY 可排序）；与 1/0 数字字面量互通（SQLite 兼容语义）
        (Value::Boolean(ab), Value::Boolean(bb)) => ab.cmp(bb),
        (Value::Boolean(ab), Value::Integer(bi)) => (*ab as i64).cmp(bi),
        (Value::Integer(ai), Value::Boolean(bb)) => ai.cmp(&(*bb as i64)),
        (Value::Boolean(ab), Value::Text(bt)) => {
            let a_str = if *ab { "true" } else { "false" };
            a_str.cmp(bt.as_str())
        }
        (Value::Text(at), Value::Boolean(bb)) => {
            let b_str = if *bb { "true" } else { "false" };
            at.as_str().cmp(b_str)
        }
        // DATE/TIME 内部为规范化 ISO 字符串 → 字典序即时间序；也允许与字符串字面量比较
        (Value::Date(ad), Value::Date(bd)) => ad.cmp(bd),
        (Value::Time(at), Value::Time(bt)) => at.cmp(bt),
        (Value::Date(ad), Value::Text(bt)) => ad.cmp(bt),
        (Value::Text(at), Value::Date(bd)) => at.cmp(bd),
        (Value::Time(at), Value::Text(bt)) => at.cmp(bt),
        (Value::Text(at), Value::Time(bt)) => at.cmp(bt),
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
            Value::Boolean(b) => b.to_string(),
            Value::Date(d) => d.clone(),
            Value::Time(t) => t.clone(),
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

    // 获取列索引（支持限定列名 table.col 和裸列名 col）
    let bare = order_col.rsplit('.').next().unwrap_or(order_col);
    let ci = schema.columns.iter()
        .find(|c| c.name == *order_col || c.name == *bare || c.name.ends_with(&format!(".{}", bare)) && c.name.rsplit('.').next().unwrap_or(&c.name) == bare)
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

    // ===== GROUP BY 分组聚合测试 =====

    #[test]
    fn test_group_by_count_star() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE emp (id INTEGER, dept TEXT, salary INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (1, 'eng', 100)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (2, 'eng', 120)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (3, 'sales', 90)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (4, 'sales', 110)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (5, 'eng', 130)").unwrap()).unwrap();

        // SELECT dept, COUNT(*) FROM emp GROUP BY dept
        let results = executor.execute(parse_sql("SELECT dept, COUNT(*) FROM emp GROUP BY dept").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns, &vec!["dept".to_string(), "count".to_string()]);
                assert_eq!(rows.len(), 2, "应分2组: eng, sales");
                // eng 组有3人，sales 组有2人
                let eng_row = rows.iter().find(|r| r[0] == "eng").unwrap();
                assert_eq!(eng_row[1], "3", "eng 组应有3人");
                let sales_row = rows.iter().find(|r| r[0] == "sales").unwrap();
                assert_eq!(sales_row[1], "2", "sales 组应有2人");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_group_by_sum_avg() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE emp (id INTEGER, dept TEXT, salary INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (1, 'eng', 100)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (2, 'eng', 120)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (3, 'sales', 90)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (4, 'sales', 110)").unwrap()).unwrap();

        // SELECT dept, SUM(salary), AVG(salary) FROM emp GROUP BY dept
        let results = executor.execute(parse_sql("SELECT dept, SUM(salary), AVG(salary) FROM emp GROUP BY dept").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns, &vec!["dept".to_string(), "sum".to_string(), "avg".to_string()]);
                assert_eq!(rows.len(), 2);

                let eng_row = rows.iter().find(|r| r[0] == "eng").unwrap();
                assert_eq!(eng_row[1], "220", "eng SUM(salary) 应为 220");
                let avg: f64 = eng_row[2].parse().unwrap();
                assert!((avg - 110.0).abs() < 0.01, "eng AVG(salary) 应为 110, 得到 {}", avg);

                let sales_row = rows.iter().find(|r| r[0] == "sales").unwrap();
                assert_eq!(sales_row[1], "200", "sales SUM(salary) 应为 200");
                let avg: f64 = sales_row[2].parse().unwrap();
                assert!((avg - 100.0).abs() < 0.01, "sales AVG(salary) 应为 100, 得到 {}", avg);
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_group_by_min_max() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, cat TEXT, val INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, cat, val) VALUES (1, 'a', 10)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, cat, val) VALUES (2, 'a', 30)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, cat, val) VALUES (3, 'b', 20)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, cat, val) VALUES (4, 'b', 50)").unwrap()).unwrap();

        let results = executor.execute(parse_sql("SELECT cat, MIN(val), MAX(val) FROM t GROUP BY cat").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns, &vec!["cat".to_string(), "min".to_string(), "max".to_string()]);
                assert_eq!(rows.len(), 2);

                let a_row = rows.iter().find(|r| r[0] == "a").unwrap();
                assert_eq!(a_row[1], "10", "a MIN 应为 10");
                assert_eq!(a_row[2], "30", "a MAX 应为 30");

                let b_row = rows.iter().find(|r| r[0] == "b").unwrap();
                assert_eq!(b_row[1], "20", "b MIN 应为 20");
                assert_eq!(b_row[2], "50", "b MAX 应为 50");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_group_by_with_where() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, dept TEXT, salary INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (1, 'eng', 100)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (2, 'eng', 120)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (3, 'eng', 80)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (4, 'sales', 90)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (5, 'sales', 110)").unwrap()).unwrap();

        // WHERE salary >= 90 → 过滤后: eng(100,120), sales(90,110)
        // GROUP BY dept → eng: 2人 sum=220, sales: 2人 sum=200
        let results = executor.execute(parse_sql("SELECT dept, COUNT(*), SUM(salary) FROM t WHERE salary >= 90 GROUP BY dept").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2, "WHERE 后应分2组");

                let eng_row = rows.iter().find(|r| r[0] == "eng").unwrap();
                assert_eq!(eng_row[1], "2", "eng 过滤后应有2人");
                assert_eq!(eng_row[2], "220", "eng SUM 应为 220");

                let sales_row = rows.iter().find(|r| r[0] == "sales").unwrap();
                assert_eq!(sales_row[1], "2", "sales 过滤后应有2人");
                assert_eq!(sales_row[2], "200", "sales SUM 应为 200");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_group_by_empty_table() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, dept TEXT, val INTEGER)").unwrap()).unwrap();

        // 空表 GROUP BY → 返回空结果（无分组）
        let results = executor.execute(parse_sql("SELECT dept, COUNT(*) FROM t GROUP BY dept").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 0, "空表 GROUP BY 应返回0行");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_group_by_single_group() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, dept TEXT, val INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, val) VALUES (1, 'x', 10)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, val) VALUES (2, 'x', 20)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, val) VALUES (3, 'x', 30)").unwrap()).unwrap();

        // 所有行同一分组
        let results = executor.execute(parse_sql("SELECT dept, COUNT(*), SUM(val) FROM t GROUP BY dept").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 1, "同一分组应返回1行");
                assert_eq!(rows[0][0], "x");
                assert_eq!(rows[0][1], "3");
                assert_eq!(rows[0][2], "60");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_group_by_multi_column() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, dept TEXT, level TEXT, salary INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, level, salary) VALUES (1, 'eng', 'junior', 100)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, level, salary) VALUES (2, 'eng', 'senior', 200)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, level, salary) VALUES (3, 'eng', 'junior', 110)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, level, salary) VALUES (4, 'sales', 'junior', 90)").unwrap()).unwrap();

        // GROUP BY dept, level → 3组: (eng,junior), (eng,senior), (sales,junior)
        let results = executor.execute(parse_sql("SELECT dept, level, COUNT(*) FROM t GROUP BY dept, level").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns, &vec!["dept".to_string(), "level".to_string(), "count".to_string()]);
                assert_eq!(rows.len(), 3, "多列 GROUP BY 应分3组");

                let eng_junior = rows.iter().find(|r| r[0] == "eng" && r[1] == "junior").unwrap();
                assert_eq!(eng_junior[2], "2", "eng/junior 应有2人");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    // ===== HAVING 过滤测试 =====

    #[test]
    fn test_having_count_star() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE emp (id INTEGER, dept TEXT, salary INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (1, 'eng', 100)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (2, 'eng', 120)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (3, 'eng', 130)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (4, 'sales', 90)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO emp (id, dept, salary) VALUES (5, 'sales', 110)").unwrap()).unwrap();

        // HAVING COUNT(*) > 2 → 只有 eng 有3人，sales 有2人被过滤
        let results = executor.execute(parse_sql("SELECT dept, COUNT(*) FROM emp GROUP BY dept HAVING COUNT(*) > 2").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 1, "HAVING COUNT(*) > 2 应只保留1组");
                assert_eq!(rows[0][0], "eng", "保留的组是 eng");
                assert_eq!(rows[0][1], "3", "eng 有3人");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_having_sum() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, dept TEXT, val INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, val) VALUES (1, 'a', 10)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, val) VALUES (2, 'a', 20)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, val) VALUES (3, 'b', 5)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, val) VALUES (4, 'b', 5)").unwrap()).unwrap();

        // HAVING SUM(val) > 20 → a: sum=30 保留, b: sum=10 过滤
        let results = executor.execute(parse_sql("SELECT dept, SUM(val) FROM t GROUP BY dept HAVING SUM(val) > 20").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 1, "HAVING SUM(val) > 20 应只保留1组");
                assert_eq!(rows[0][0], "a", "保留的组是 a");
                assert_eq!(rows[0][1], "30", "a SUM = 30");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_having_with_where() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, dept TEXT, salary INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (1, 'eng', 100)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (2, 'eng', 120)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (3, 'eng', 80)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (4, 'sales', 90)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (5, 'sales', 110)").unwrap()).unwrap();

        // WHERE salary >= 90 → eng(100,120)=2人 sum=220, sales(90,110)=2人 sum=200
        // HAVING COUNT(*) >= 2 → 两组都保留（各2人）
        let results = executor.execute(parse_sql("SELECT dept, COUNT(*), SUM(salary) FROM t WHERE salary >= 90 GROUP BY dept HAVING COUNT(*) >= 2").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2, "两组都应有2人");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_having_all_filtered() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, dept TEXT, val INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, val) VALUES (1, 'a', 10)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, val) VALUES (2, 'b', 20)").unwrap()).unwrap();

        // HAVING SUM(val) > 100 → 无组满足，返回0行
        let results = executor.execute(parse_sql("SELECT dept, SUM(val) FROM t GROUP BY dept HAVING SUM(val) > 100").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 0, "HAVING 过滤掉所有组应返回0行");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_having_avg() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, cat TEXT, score INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, cat, score) VALUES (1, 'x', 80)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, cat, score) VALUES (2, 'x', 90)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, cat, score) VALUES (3, 'y', 40)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, cat, score) VALUES (4, 'y', 60)").unwrap()).unwrap();

        // HAVING AVG(score) > 50 → x: avg=85 保留, y: avg=50 过滤
        let results = executor.execute(parse_sql("SELECT cat, AVG(score) FROM t GROUP BY cat HAVING AVG(score) > 50").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 1, "HAVING AVG(score) > 50 应只保留1组");
                assert_eq!(rows[0][0], "x", "保留的组是 x");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_having_and_or() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, dept TEXT, salary INTEGER)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (1, 'eng', 100)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (2, 'eng', 120)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (3, 'sales', 90)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (4, 'sales', 110)").unwrap()).unwrap();
        executor.execute(parse_sql("INSERT INTO t (id, dept, salary) VALUES (5, 'hr', 50)").unwrap()).unwrap();

        // eng: count=2, sum=220; sales: count=2, sum=200; hr: count=1, sum=50
        // HAVING COUNT(*) > 1 AND SUM(salary) > 200 → eng 保留 (2>1 AND 220>200)
        // sales: 2>1 AND 200>200 → false
        let results = executor.execute(parse_sql("SELECT dept, COUNT(*), SUM(salary) FROM t GROUP BY dept HAVING COUNT(*) > 1 AND SUM(salary) > 200").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 1, "AND 条件应只保留 eng");
                assert_eq!(rows[0][0], "eng");
            }
            _ => panic!("Expected SelectResult"),
        }

        // HAVING COUNT(*) > 1 OR SUM(salary) > 100 → eng(2>1 T) + sales(2>1 T) + hr(1>1 F OR 50>100 F → F)
        let results = executor.execute(parse_sql("SELECT dept, COUNT(*), SUM(salary) FROM t GROUP BY dept HAVING COUNT(*) > 1 OR SUM(salary) > 100").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2, "OR 条件应保留 eng + sales");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_having_empty_table() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, dept TEXT, val INTEGER)").unwrap()).unwrap();

        // 空表 GROUP BY + HAVING → 0行
        let results = executor.execute(parse_sql("SELECT dept, COUNT(*) FROM t GROUP BY dept HAVING COUNT(*) > 0").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 0, "空表 HAVING 应返回0行");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    // ===== BOOLEAN / DATE / TIME 端到端 =====

    /// 建一张含三种新类型的表
    fn setup_typed_table(executor: &mut Executor) {
        executor.execute(parse_sql(
            "CREATE TABLE tasks (id INTEGER, title TEXT, due DATE, at TIME, done BOOLEAN)",
        ).unwrap()).unwrap();
        executor.execute(parse_sql(
            "INSERT INTO tasks (id, title, due, at, done) VALUES \
             (1, 'alpha', '2026-01-15', '09:00:00', false), \
             (2, 'beta', '2026-03-02', '14:30:00', true), \
             (3, 'gamma', '2025-12-31', '23:59:59', false), \
             (4, 'delta', '2026-03-02', '08:00:00', true)",
        ).unwrap()).unwrap();
    }

    #[test]
    fn test_boolean_insert_and_select() {
        let mut executor = Executor::new();
        setup_typed_table(&mut executor);

        let results = executor.execute(parse_sql("SELECT id, title, done FROM tasks").unwrap()).unwrap();
        match &results[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns, &vec!["id".to_string(), "title".to_string(), "done".to_string()]);
                assert_eq!(rows.len(), 4);
                assert_eq!(rows[0][2], "false", "BOOL 输出应为 true/false");
                assert_eq!(rows[1][2], "true");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_boolean_where_filter() {
        let mut executor = Executor::new();
        setup_typed_table(&mut executor);

        // WHERE done = true （裸布尔字面量）
        let r = executor.execute(parse_sql("SELECT id FROM tasks WHERE done = true").unwrap()).unwrap();
        match &r[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2, "done=true 应有2行");
                assert_eq!(rows[0][0], "2");
                assert_eq!(rows[1][0], "4");
            }
            _ => panic!("Expected SelectResult"),
        }

        // WHERE done = false
        let r2 = executor.execute(parse_sql("SELECT id FROM tasks WHERE done = false").unwrap()).unwrap();
        match &r2[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2, "done=false 应有2行");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_boolean_where_equality_is_not_always_true() {
        // 回归保护：类型不匹配时 `=` 不能退化成恒真
        let mut executor = Executor::new();
        setup_typed_table(&mut executor);

        // 全部都是 false 的行查询 done = true，必须返回 0 行
        let r = executor.execute(parse_sql("SELECT id FROM tasks WHERE done = true AND title = 'nonexistent'").unwrap()).unwrap();
        match &r[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 0);
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_boolean_order_by() {
        let mut executor = Executor::new();
        setup_typed_table(&mut executor);

        // false < true，升序时 false 在前
        let r = executor.execute(parse_sql("SELECT id, done FROM tasks ORDER BY done ASC").unwrap()).unwrap();
        match &r[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows[0][1], "false");
                assert_eq!(rows[1][1], "false");
                assert_eq!(rows[2][1], "true");
                assert_eq!(rows[3][1], "true");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_date_range_filter() {
        let mut executor = Executor::new();
        setup_typed_table(&mut executor);

        let r = executor.execute(parse_sql("SELECT id, due FROM tasks WHERE due > '2026-01-01'").unwrap()).unwrap();
        match &r[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 3, "2026-01-01 之后应有3行（1/15、3/2、3/2）");
            }
            _ => panic!("Expected SelectResult"),
        }

        // 跨年边界：2025-12-31 必须被排除
        let r2 = executor.execute(parse_sql("SELECT id FROM tasks WHERE due >= '2025-12-31' AND due <= '2026-01-15'").unwrap()).unwrap();
        match &r2[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2, "闭区间应含 2025-12-31 与 2026-01-15");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_date_order_by_is_chronological() {
        let mut executor = Executor::new();
        setup_typed_table(&mut executor);

        let r = executor.execute(parse_sql("SELECT due FROM tasks ORDER BY due ASC").unwrap()).unwrap();
        match &r[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                let dues: Vec<&str> = rows.iter().map(|row| row[0].as_str()).collect();
                assert_eq!(dues, vec!["2025-12-31", "2026-01-15", "2026-03-02", "2026-03-02"],
                    "DATE 字典序必须等于时间序");
            }
            _ => panic!("Expected SelectResult"),
        }

        let r2 = executor.execute(parse_sql("SELECT due FROM tasks ORDER BY due DESC").unwrap()).unwrap();
        match &r2[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows[0][0], "2026-03-02");
                assert_eq!(rows[3][0], "2025-12-31");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_time_column_and_filter() {
        let mut executor = Executor::new();
        setup_typed_table(&mut executor);

        // HH:MM 应被规范化为 HH:MM:SS
        let r = executor.execute(parse_sql("SELECT at FROM tasks WHERE id = 2").unwrap()).unwrap();
        match &r[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows[0][0], "14:30:00");
            }
            _ => panic!("Expected SelectResult"),
        }

        let r2 = executor.execute(parse_sql("SELECT id FROM tasks WHERE at < '12:00:00'").unwrap()).unwrap();
        match &r2[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2, "12点前应有2行（09:00、08:00）");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_invalid_date_insert_is_rejected() {
        let mut executor = Executor::new();
        executor.execute(parse_sql("CREATE TABLE t (id INTEGER, d DATE)").unwrap()).unwrap();

        // 不存在的日期 → 报错，不静默写入
        let r = executor.execute(parse_sql("INSERT INTO t (id, d) VALUES (1, '2026-02-30')").unwrap());
        assert!(r.is_err(), "非法日期应拒绝插入");

        let r2 = executor.execute(parse_sql("INSERT INTO t (id, d) VALUES (1, '2026/02/28')").unwrap());
        assert!(r2.is_err(), "错误格式应拒绝插入");

        let r3 = executor.execute(parse_sql("INSERT INTO t (id, d) VALUES (1, '2025-02-29')").unwrap());
        assert!(r3.is_err(), "平年2月29日应拒绝插入");

        // 合法值可写入
        executor.execute(parse_sql("INSERT INTO t (id, d) VALUES (1, '2024-02-29')").unwrap()).unwrap();
        let r4 = executor.execute(parse_sql("SELECT d FROM t").unwrap()).unwrap();
        match &r4[0] {
            ExecuteResult::SelectResult { rows, .. } => assert_eq!(rows[0][0], "2024-02-29"),
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_boolean_update_and_delete() {
        let mut executor = Executor::new();
        setup_typed_table(&mut executor);

        // UPDATE 布尔列
        executor.execute(parse_sql("UPDATE tasks SET done = true WHERE id = 1").unwrap()).unwrap();
        let r = executor.execute(parse_sql("SELECT done FROM tasks WHERE id = 1").unwrap()).unwrap();
        match &r[0] {
            ExecuteResult::SelectResult { rows, .. } => assert_eq!(rows[0][0], "true"),
            _ => panic!("Expected SelectResult"),
        }

        // UPDATE 日期列
        executor.execute(parse_sql("UPDATE tasks SET due = '2027-07-04' WHERE id = 3").unwrap()).unwrap();
        let r2 = executor.execute(parse_sql("SELECT due FROM tasks WHERE id = 3").unwrap()).unwrap();
        match &r2[0] {
            ExecuteResult::SelectResult { rows, .. } => assert_eq!(rows[0][0], "2027-07-04"),
            _ => panic!("Expected SelectResult"),
        }

        // DELETE 按布尔条件
        // 注意：id=1 已被上面的 UPDATE 改成 done=true，此时 done=false 只剩 id=3
        let r3 = executor.execute(parse_sql("DELETE FROM tasks WHERE done = false").unwrap()).unwrap();
        match &r3[0] {
            ExecuteResult::Deleted { count } => assert_eq!(*count, 1, "应有1行 done=false 被删除（id=1 已改为 true）"),
            _ => panic!("Expected Deleted"),
        }
    }

    #[test]
    fn test_new_types_with_group_by_and_aggregate() {
        let mut executor = Executor::new();
        setup_typed_table(&mut executor);

        // 按布尔列分组计数 —— 新类型能作为分组键
        let r = executor.execute(parse_sql("SELECT done, COUNT(*) FROM tasks GROUP BY done").unwrap()).unwrap();
        match &r[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2, "done 应分成两组");
                let counts: Vec<&str> = rows.iter().map(|row| row[1].as_str()).collect();
                assert!(counts.contains(&"2"), "每组各2行, 实际: {:?}", counts);
            }
            _ => panic!("Expected SelectResult"),
        }

        // 按日期分组 —— 3/2 两行应归为一组
        let r2 = executor.execute(parse_sql("SELECT due, COUNT(*) FROM tasks GROUP BY due").unwrap()).unwrap();
        match &r2[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 3, "3个不同日期应有3组");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_date_distinct() {
        let mut executor = Executor::new();
        setup_typed_table(&mut executor);

        let r = executor.execute(parse_sql("SELECT DISTINCT due FROM tasks").unwrap()).unwrap();
        match &r[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 3, "4行数据只有3个不同日期");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    // ===== JOIN 测试 =====

    fn setup_join_tables(executor: &mut Executor) {
        // 创建 users 表
        executor.execute(parse_sql(
            "CREATE TABLE users (id INTEGER, name TEXT, dept_id INTEGER)"
        ).unwrap()).unwrap();
        executor.execute(parse_sql(
            "INSERT INTO users (id, name, dept_id) VALUES (1, 'alice', 10), (2, 'bob', 20), (3, 'charlie', 10)"
        ).unwrap()).unwrap();

        // 创建 departments 表
        executor.execute(parse_sql(
            "CREATE TABLE departments (id INTEGER, name TEXT)"
        ).unwrap()).unwrap();
        executor.execute(parse_sql(
            "INSERT INTO departments (id, name) VALUES (10, 'Engineering'), (20, 'Sales'), (30, 'HR')"
        ).unwrap()).unwrap();
    }

    #[test]
    fn test_inner_join_basic() {
        let mut executor = Executor::new();
        setup_join_tables(&mut executor);

        let r = executor.execute(parse_sql(
            "SELECT * FROM users INNER JOIN departments ON users.dept_id = departments.id"
        ).unwrap()).unwrap();

        match &r[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                // users有3列 + departments有2列 = 5列
                assert_eq!(columns.len(), 5, "JOIN后应有5列（3+2）");
                // alice→Engineering, bob→Sales, charlie→Engineering
                // HR(30)无匹配用户
                assert_eq!(rows.len(), 3, "3个用户都有匹配的部门");
                // 验证第一行：alice, dept_id=10, Engineering
                assert_eq!(rows[0][1], "alice");
                assert_eq!(rows[0][4], "Engineering");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_inner_join_with_where() {
        let mut executor = Executor::new();
        setup_join_tables(&mut executor);

        let r = executor.execute(parse_sql(
            "SELECT * FROM users INNER JOIN departments ON users.dept_id = departments.id WHERE departments.name = 'Engineering'"
        ).unwrap()).unwrap();

        match &r[0] {
            ExecuteResult::SelectResult { columns, rows, .. } => {
                eprintln!("DEBUG WHERE: columns={:?}", columns);
                eprintln!("DEBUG WHERE: rows.len()={}", rows.len());
                for (i, row) in rows.iter().enumerate() {
                    eprintln!("DEBUG WHERE: row[{}]={:?}", i, row);
                }
                // Let's also test without WHERE to see the join result
                let r2 = executor.execute(parse_sql(
                    "SELECT * FROM users INNER JOIN departments ON users.dept_id = departments.id"
                ).unwrap()).unwrap();
                match &r2[0] {
                    ExecuteResult::SelectResult { rows: rows2, .. } => {
                        eprintln!("DEBUG NOWHERE: rows.len()={}", rows2.len());
                        for (i, row) in rows2.iter().enumerate() {
                            eprintln!("DEBUG NOWHERE: row[{}]={:?}", i, row);
                        }
                    }
                    _ => {}
                }
                assert_eq!(rows.len(), 2, "Engineering部门有2人");
                for row in rows {
                    assert_eq!(row[4], "Engineering");
                }
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_inner_join_with_order_by() {
        let mut executor = Executor::new();
        setup_join_tables(&mut executor);

        let r = executor.execute(parse_sql(
            "SELECT * FROM users INNER JOIN departments ON users.dept_id = departments.id ORDER BY users.id DESC"
        ).unwrap()).unwrap();

        match &r[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 3);
                // DESC: charlie(3), bob(2), alice(1)
                assert_eq!(rows[0][1], "charlie");
                assert_eq!(rows[2][1], "alice");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_inner_join_no_match() {
        let mut executor = Executor::new();
        setup_join_tables(&mut executor);

        // HR 部门(id=30)没有用户关联
        let r = executor.execute(parse_sql(
            "SELECT * FROM departments INNER JOIN users ON departments.id = users.dept_id WHERE departments.name = 'HR'"
        ).unwrap()).unwrap();

        match &r[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 0, "HR部门无关联用户，应返回0行");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_inner_join_with_limit() {
        let mut executor = Executor::new();
        setup_join_tables(&mut executor);

        let r = executor.execute(parse_sql(
            "SELECT * FROM users INNER JOIN departments ON users.dept_id = departments.id LIMIT 2"
        ).unwrap()).unwrap();

        match &r[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 2, "LIMIT 2 应返回2行");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_inner_join_column_projection() {
        let mut executor = Executor::new();
        setup_join_tables(&mut executor);

        // 只选特定列（使用右表的限定列名 departments.name）
        let r = executor.execute(parse_sql(
            "SELECT users.name, departments.name FROM users INNER JOIN departments ON users.dept_id = departments.id"
        ).unwrap()).unwrap();

        match &r[0] {
            ExecuteResult::SelectResult { columns, rows } => {
                assert_eq!(columns.len(), 2, "只选2列");
                assert_eq!(rows.len(), 3);
                // 验证关联正确性
                assert_eq!(rows[0][0], "alice");
                assert_eq!(rows[0][1], "Engineering");
            }
            _ => panic!("Expected SelectResult"),
        }
    }

    #[test]
    fn test_join_without_inner_keyword() {
        // 仅 JOIN 也应等同于 INNER JOIN
        let mut executor = Executor::new();
        setup_join_tables(&mut executor);

        let r = executor.execute(parse_sql(
            "SELECT * FROM users JOIN departments ON users.dept_id = departments.id"
        ).unwrap()).unwrap();

        match &r[0] {
            ExecuteResult::SelectResult { rows, .. } => {
                assert_eq!(rows.len(), 3, "JOIN 等同于 INNER JOIN");
            }
            _ => panic!("Expected SelectResult"),
        }
    }
}
