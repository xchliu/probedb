// ProbeDB 存储引擎 — 支持行+向量的统一存储

use std::collections::HashMap;

use crate::types::{DataType, Value};

/// 表结构定义
#[derive(Debug, Clone)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<ColumnInfo>,
}

/// 列信息
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: DataType,
    pub index: usize,
}

/// 数据行
#[derive(Debug, Clone)]
pub struct Row {
    pub id: u64,
    pub values: Vec<Value>,
}

/// 存储引擎 — 内存实现（MVP阶段）
/// 后续替换为持久化存储
#[derive(Debug)]
pub struct StorageEngine {
    /// 所有表 schema
    schemas: HashMap<String, TableSchema>,
    /// 所有表的数据（table_name -> rows）
    data: HashMap<String, Vec<Row>>,
    /// 自增 ID 计数器
    next_id: u64,
}

impl StorageEngine {
    pub fn new() -> Self {
        StorageEngine {
            schemas: HashMap::new(),
            data: HashMap::new(),
            next_id: 1,
        }
    }

    /// 创建表
    pub fn create_table(&mut self, schema: TableSchema) -> Result<(), String> {
        let name = schema.name.clone();
        if self.schemas.contains_key(&name) {
            return Err(format!("表 '{}' 已存在", name));
        }
        self.schemas.insert(name.clone(), schema);
        self.data.insert(name, Vec::new());
        Ok(())
    }

    /// 插入数据
    pub fn insert(&mut self, table_name: &str, values: Vec<Value>) -> Result<u64, String> {
        let schema = self.schemas.get(table_name)
            .ok_or_else(|| format!("表 '{}' 不存在", table_name))?;

        if values.len() != schema.columns.len() {
            return Err(format!(
                "列数不匹配: 需要 {} 列, 提供了 {} 列",
                schema.columns.len(),
                values.len()
            ));
        }

        // 类型校验
        for (_, (col, val)) in schema.columns.iter().zip(values.iter()).enumerate() {
            if !type_matches(&col.data_type, val) {
                return Err(format!(
                    "列 '{}' 类型不匹配: 期望 {:?}, 得到 {:?}",
                    col.name, col.data_type, val
                ));
            }
        }

        let id = self.next_id;
        self.next_id += 1;

        let row = Row { id, values };

        if let Some(rows) = self.data.get_mut(table_name) {
            rows.push(row);
        }

        Ok(id)
    }

    /// 查询表的所有数据
    pub fn scan_table(&self, table_name: &str) -> Result<&Vec<Row>, String> {
        self.data.get(table_name)
            .ok_or_else(|| format!("表 '{}' 不存在", table_name))
    }

    /// 获取表 schema
    pub fn get_schema(&self, table_name: &str) -> Result<&TableSchema, String> {
        self.schemas.get(table_name)
            .ok_or_else(|| format!("表 '{}' 不存在", table_name))
    }

    /// 获取所有表名
    pub fn table_names(&self) -> Vec<String> {
        self.schemas.keys().cloned().collect()
    }

    /// 按ID列表删除行
    pub fn delete_by_ids(&mut self, table_name: &str, ids: &[u64]) -> Result<usize, String> {
        let table_name_str = table_name.to_string();
        let rows = self.data.get_mut(&table_name_str)
            .ok_or_else(|| format!("表 '{}' 不存在", table_name))?;

        let original_len = rows.len();
        let id_set: std::collections::HashSet<u64> = ids.iter().cloned().collect();
        rows.retain(|r| !id_set.contains(&r.id));
        let deleted = original_len - rows.len();
        Ok(deleted)
    }

    /// 更新指定ID行的指定列
    pub fn update_by_ids(
        &mut self,
        table_name: &str,
        ids: &[u64],
        col_index: usize,
        new_value: Value,
    ) -> Result<usize, String> {
        let table_name_str = table_name.to_string();
        let rows = self.data.get_mut(&table_name_str)
            .ok_or_else(|| format!("表 '{}' 不存在", table_name))?;

        let id_set: std::collections::HashSet<u64> = ids.iter().cloned().collect();
        let mut updated = 0;
        for row in rows.iter_mut() {
            if id_set.contains(&row.id) {
                if col_index < row.values.len() {
                    row.values[col_index] = new_value.clone();
                    updated += 1;
                }
            }
        }
        Ok(updated)
    }

    /// 获取某个表的可变引用数据（用于事务性替换）
    pub fn table_data_mut(&mut self, table_name: &str) -> Result<&mut Vec<Row>, String> {
        self.data.get_mut(table_name)
            .ok_or_else(|| format!("表 '{}' 不存在", table_name))
    }

    /// 获取自增计数器当前值（持久化用）
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// 设置自增计数器（持久化恢复用）
    pub fn set_next_id(&mut self, id: u64) {
        self.next_id = id;
    }

    /// 按指定ID插入行（持久化恢复用，跳过自增）
    pub fn insert_row_with_id(
        &mut self,
        table_name: &str,
        id: u64,
        values: Vec<Value>,
    ) -> Result<(), String> {
        let schema = self.schemas.get(table_name)
            .ok_or_else(|| format!("表 '{}' 不存在", table_name))?;

        if values.len() != schema.columns.len() {
            return Err(format!(
                "列数不匹配: 需要 {} 列, 提供了 {} 列",
                schema.columns.len(),
                values.len()
            ));
        }
        for (col, val) in schema.columns.iter().zip(values.iter()) {
            if !type_matches(&col.data_type, val) {
                return Err(format!(
                    "列 '{}' 类型不匹配: 期望 {:?}, 得到 {:?}",
                    col.name, col.data_type, val
                ));
            }
        }
        let row = Row { id, values };
        if let Some(rows) = self.data.get_mut(table_name) {
            rows.push(row);
        }
        Ok(())
    }

    /// 序列化整个引擎状态为文本行协议（持久化用）
    ///
    /// 格式 v1:
    /// ```
    /// # ProbeDB state v1
    /// SCHEMA|<table>|<col>:<type>|<col>:<type>...
    /// ROW|<table>|<row_id>|<value>|<value>...
    /// NEXTID|<next_id>
    /// ```
    /// 显式类型标记 + 转义，保证 Text 含分隔符也能无损还原。
    pub fn export_state(&self) -> String {
        let mut out = String::new();
        out.push_str("# ProbeDB state v1\n");
        for name in self.table_names() {
            let schema = &self.schemas[&name];
            let cols: Vec<String> = schema.columns.iter()
                .map(|c| format!("{}:{}", c.name, encode_type(&c.data_type)))
                .collect();
            out.push_str(&format!("SCHEMA|{}|{}\n", name, cols.join("|")));
        }
        for name in self.table_names() {
            if let Some(rows) = self.data.get(&name) {
                for row in rows {
                    let vals: Vec<String> = row.values.iter().map(encode_value).collect();
                    out.push_str(&format!("ROW|{}|{}|{}\n", name, row.id, vals.join("|")));
                }
            }
        }
        out.push_str(&format!("NEXTID|{}\n", self.next_id));
        out
    }

    /// 从序列化状态恢复引擎（持久化用）
    pub fn import_state(&mut self, state: &str) -> Result<(), String> {
        // 清空现有状态
        self.schemas.clear();
        self.data.clear();
        self.next_id = 1;

        for (lineno, line) in state.lines().enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = split_pipe_aware(line);
            if parts.is_empty() {
                continue;
            }
            match parts[0] {
                "SCHEMA" => {
                    if parts.len() < 3 {
                        return Err(format!("第{}行 SCHEMA 字段不足", lineno + 1));
                    }
                    let name = parts[1].to_string();
                    let mut columns = Vec::new();
                    for (i, col) in parts[2..].iter().enumerate() {
                        let (col_name, type_str) = col.split_once(':')
                            .ok_or_else(|| format!("第{}行 SCHEMA 列格式错误: {}", lineno + 1, col))?;
                        columns.push(ColumnInfo {
                            name: col_name.to_string(),
                            data_type: decode_type(type_str)?,
                            index: i,
                        });
                    }
                    self.schemas.insert(name.clone(), TableSchema { name, columns });
                    self.data.insert(parts[1].to_string(), Vec::new());
                }
                "ROW" => {
                    if parts.len() < 4 {
                        return Err(format!("第{}行 ROW 字段不足", lineno + 1));
                    }
                    let table = parts[1];
                    let id: u64 = parts[2].parse()
                        .map_err(|_| format!("第{}行 ROW id 解析失败: {}", lineno + 1, parts[2]))?;
                    let vals: Result<Vec<Value>, String> =
                        parts[3..].iter().map(|s| decode_value(s)).collect();
                    self.insert_row_with_id(table, id, vals?)?;
                }
                "NEXTID" => {
                    if parts.len() < 2 {
                        return Err(format!("第{}行 NEXTID 字段不足", lineno + 1));
                    }
                    let id: u64 = parts[1].parse()
                        .map_err(|_| format!("第{}行 NEXTID 解析失败: {}", lineno + 1, parts[1]))?;
                    self.next_id = id;
                }
                other => return Err(format!("第{}行未知记录类型: {}", lineno + 1, other)),
            }
        }
        Ok(())
    }
}

/// 编码数据类型为字符串
fn encode_type(t: &DataType) -> String {
    match t {
        DataType::Integer => "INTEGER".to_string(),
        DataType::Float => "FLOAT".to_string(),
        DataType::Text => "TEXT".to_string(),
        DataType::Vector(dim) => format!("VECTOR:{}", dim),
    }
}

/// 转义感知的字段分割：跳过 `\|` 和 `\\` 转义对，只按未转义的 `|` 分割
///
/// 保证 Text 值中的字面 `|`（编码为 `\|`）不会被误切成多个字段。
/// 字节索引安全：`|`(0x7C) 和 `\`(0x5C) 都是单字节 ASCII，
/// 多字节 UTF-8 字符的字节值 ≥ 0x80，不会干扰切片边界。
fn split_pipe_aware(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2; // 跳过转义对（\\ 或 \| 等）
            continue;
        }
        if bytes[i] == b'|' {
            parts.push(&s[start..i]);
            start = i + 1;
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts
}

/// 解码数据类型
fn decode_type(s: &str) -> Result<DataType, String> {
    match s {
        "INTEGER" => Ok(DataType::Integer),
        "FLOAT" => Ok(DataType::Float),
        "TEXT" => Ok(DataType::Text),
        other if other.starts_with("VECTOR:") => {
            let dim: usize = other[7..].parse()
                .map_err(|_| format!("VECTOR维度解析失败: {}", other))?;
            Ok(DataType::Vector(dim))
        }
        other => Err(format!("未知数据类型: {}", other)),
    }
}

/// 编码值为字符串（显式类型标记）
fn encode_value(v: &Value) -> String {
    match v {
        Value::Integer(n) => format!("INT:{}", n),
        Value::Float(f) => format!("FLOAT:{}", f),
        Value::Text(s) => format!("TEXT:{}", escape_text(s)),
        Value::Vector(vec) => {
            let dims: Vec<String> = vec.iter().map(|x| x.to_string()).collect();
            format!("VEC:{}:{}", vec.len(), dims.join(","))
        }
    }
}

/// 解码值
fn decode_value(s: &str) -> Result<Value, String> {
    if let Some(rest) = s.strip_prefix("INT:") {
        return rest.parse::<i64>().map(Value::Integer)
            .map_err(|_| format!("整数解析失败: {}", s));
    }
    if let Some(rest) = s.strip_prefix("FLOAT:") {
        return rest.parse::<f64>().map(Value::Float)
            .map_err(|_| format!("浮点解析失败: {}", s));
    }
    if let Some(rest) = s.strip_prefix("TEXT:") {
        return Ok(Value::Text(unescape_text(rest)));
    }
    if let Some(rest) = s.strip_prefix("VEC:") {
        let (len_str, nums_str) = rest.split_once(':')
            .ok_or_else(|| format!("向量编码格式错误: {}", s))?;
        let _len: usize = len_str.parse()
            .map_err(|_| format!("向量长度解析失败: {}", s))?;
        let nums: Result<Vec<f64>, _> = nums_str.split(',')
            .map(|x| x.trim().parse::<f64>())
            .collect();
        return nums.map(Value::Vector)
            .map_err(|_| format!("向量元素解析失败: {}", s));
    }
    Err(format!("未知值编码: {}", s))
}

/// 转义文本: \ → \\, | → \|, \n → \n, \r → \r
fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '|' => out.push_str("\\|"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

/// 反转义文本
fn unescape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('|') => out.push('|'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 检查值是否匹配列类型
fn type_matches(col_type: &DataType, value: &Value) -> bool {
    match (col_type, value) {
        (DataType::Integer, Value::Integer(_)) => true,
        (DataType::Float, Value::Float(_))
        | (DataType::Float, Value::Integer(_)) => true, // Integer 可以隐式转 Float
        (DataType::Text, Value::Text(_)) => true,
        (DataType::Vector(dim), Value::Vector(v)) => {
            if *dim == 0 {
                true // 不限制维度
            } else {
                v.len() == *dim
            }
        }
        _ => false,
    }
}

/// 将字符串值解析为对应的 Value
pub fn parse_value(value_str: &str, col_type: &DataType) -> Result<Value, String> {
    match col_type {
        DataType::Integer => {
            let v = value_str.parse::<i64>()
                .map_err(|_| format!("无法解析为整数: {}", value_str))?;
            Ok(Value::Integer(v))
        }
        DataType::Float => {
            let v = value_str.parse::<f64>()
                .map_err(|_| format!("无法解析为浮点数: {}", value_str))?;
            Ok(Value::Float(v))
        }
        DataType::Text => {
            // 去掉可能的引号
            let s = value_str.trim_matches('\'');
            Ok(Value::Text(s.to_string()))
        }
        DataType::Vector(_) => {
            // 解析向量格式: [1.0,2.0,3.0] 或 1.0,2.0,3.0
            let trimmed = value_str.trim_matches('[').trim_matches(']');
            let nums: Result<Vec<f64>, _> = trimmed.split(',')
                .map(|s| s.trim().parse::<f64>())
                .collect();
            let v = nums.map_err(|_| format!("无法解析为向量: {}", value_str))?;
            Ok(Value::Vector(v))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_schema() -> TableSchema {
        TableSchema {
            name: "users".to_string(),
            columns: vec![
                ColumnInfo { name: "id".to_string(), data_type: DataType::Integer, index: 0 },
                ColumnInfo { name: "name".to_string(), data_type: DataType::Text, index: 1 },
                ColumnInfo { name: "age".to_string(), data_type: DataType::Integer, index: 2 },
            ],
        }
    }

    #[test]
    fn test_create_and_insert() {
        let mut engine = StorageEngine::new();
        engine.create_table(create_test_schema()).unwrap();

        let id = engine.insert("users", vec![
            Value::Integer(1),
            Value::Text("alice".to_string()),
            Value::Integer(30),
        ]).unwrap();
        assert_eq!(id, 1);

        let rows = engine.scan_table("users").unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn test_type_mismatch() {
        let mut engine = StorageEngine::new();
        engine.create_table(create_test_schema()).unwrap();

        let result = engine.insert("users", vec![
            Value::Integer(1),
            Value::Integer(2), // name 列应该是 Text
            Value::Integer(30),
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn test_duplicate_table() {
        let mut engine = StorageEngine::new();
        engine.create_table(create_test_schema()).unwrap();
        let result = engine.create_table(create_test_schema());
        assert!(result.is_err());
    }

    #[test]
    fn test_vector_storage() {
        let mut engine = StorageEngine::new();
        let schema = TableSchema {
            name: "items".to_string(),
            columns: vec![
                ColumnInfo { name: "id".to_string(), data_type: DataType::Integer, index: 0 },
                ColumnInfo { name: "embedding".to_string(), data_type: DataType::Vector(3), index: 1 },
            ],
        };
        engine.create_table(schema).unwrap();

        engine.insert("items", vec![
            Value::Integer(1),
            Value::Vector(vec![0.1, 0.2, 0.3]),
        ]).unwrap();

        let rows = engine.scan_table("items").unwrap();
        assert_eq!(rows.len(), 1);
        if let Value::Vector(v) = &rows[0].values[1] {
            assert_eq!(v.len(), 3);
        } else {
            panic!("Expected Vector value");
        }
    }
}