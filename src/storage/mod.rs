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
        for (i, (col, val)) in schema.columns.iter().zip(values.iter()).enumerate() {
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