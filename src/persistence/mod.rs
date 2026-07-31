// ProbeDB 持久化引擎 — 序列化到磁盘（零外部依赖）
//
// 职责划分：
// - StorageEngine::export_state / import_state — 内存状态 ↔ 文本行协议字符串
// - persistence::save / load — 字符串 ↔ 磁盘文件（原子写 + 加载）
//
// 原子写策略：先写临时文件，再 rename 替换目标文件。
// 即使进程在写入中途崩溃，也不会留下半截数据库文件。

use std::fs;
use std::path::Path;

use crate::storage::StorageEngine;

/// 将引擎状态原子保存到文件
///
/// 步骤：写 `<path>.tmp` → rename 到 `<path>`。
/// rename 在 POSIX 上是原子操作，保证数据库文件要么是旧的完整状态，
/// 要么是新的完整状态，绝不会是写入一半的残缺文件。
pub fn save(engine: &StorageEngine, path: &str) -> Result<(), String> {
    let state = engine.export_state();
    let tmp_path = format!("{}.tmp", path);
    fs::write(&tmp_path, state)
        .map_err(|e| format!("写入临时文件失败 ({}): {}", tmp_path, e))?;
    fs::rename(&tmp_path, path)
        .map_err(|e| format!("替换数据库文件失败 ({}): {}", path, e))?;
    Ok(())
}

/// 从文件加载引擎状态
///
/// 文件不存在时返回 Err（调用方决定是报错还是新建空库）。
pub fn load(path: &str) -> Result<StorageEngine, String> {
    if !Path::new(path).exists() {
        return Err(format!("数据库文件不存在: {}", path));
    }
    let state = fs::read_to_string(path)
        .map_err(|e| format!("读取数据库文件失败 ({}): {}", path, e))?;
    let mut engine = StorageEngine::new();
    engine.import_state(&state)?;
    Ok(engine)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_engine() -> StorageEngine {
        use crate::storage::TableSchema;
        use crate::storage::ColumnInfo;
        use crate::types::{DataType, Value};

        let mut engine = StorageEngine::new();
        engine.create_table(TableSchema {
            name: "users".to_string(),
            columns: vec![
                ColumnInfo { name: "id".to_string(), data_type: DataType::Integer, index: 0 },
                ColumnInfo { name: "name".to_string(), data_type: DataType::Text, index: 1 },
            ],
        }).unwrap();
        engine.insert("users", vec![
            Value::Integer(1),
            Value::Text("alice".to_string()),
        ]).unwrap();
        engine.insert("users", vec![
            Value::Integer(2),
            Value::Text("bob|smith".to_string()), // 含分隔符，考验转义
        ]).unwrap();
        engine
    }

    #[test]
    fn test_save_load_roundtrip() {
        let tmp = std::env::temp_dir().join("probedb_persist_test.pdb");
        let path = tmp.to_str().unwrap().to_string();
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(format!("{}.tmp", path));

        let engine = build_engine();
        save(&engine, &path).unwrap();

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.table_names(), vec!["users".to_string()]);
        let rows = loaded.scan_table("users").unwrap();
        assert_eq!(rows.len(), 2);
        // Text 含 | 无损还原
        assert_eq!(rows[1].values[1], crate::types::Value::Text("bob|smith".to_string()));
        // id 保留
        assert_eq!(rows[0].id, 1);
        assert_eq!(rows[1].id, 2);
        // next_id 恢复（原引擎插了2条，next_id 应为3）
        assert_eq!(loaded.next_id(), 3);

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(format!("{}.tmp", path));
    }

    #[test]
    fn test_save_overwrites_previous_state() {
        let tmp = std::env::temp_dir().join("probedb_persist_overwrite.pdb");
        let path = tmp.to_str().unwrap().to_string();
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(format!("{}.tmp", path));

        // 第一次保存：2行
        save(&build_engine(), &path).unwrap();

        // 第二次保存：1行（模拟新状态覆盖旧状态）
        let mut engine = StorageEngine::new();
        use crate::storage::{TableSchema, ColumnInfo};
        use crate::types::{DataType, Value};
        engine.create_table(TableSchema {
            name: "users".to_string(),
            columns: vec![ColumnInfo { name: "id".to_string(), data_type: DataType::Integer, index: 0 }],
        }).unwrap();
        engine.insert("users", vec![Value::Integer(99)]).unwrap();
        save(&engine, &path).unwrap();

        let loaded = load(&path).unwrap();
        let rows = loaded.scan_table("users").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].values[0], Value::Integer(99));

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(format!("{}.tmp", path));
    }

    #[test]
    fn test_load_missing_file_errors() {
        let r = load("/nonexistent/probedb_missing.pdb");
        assert!(r.is_err());
    }

    #[test]
    fn test_export_import_with_vector() {
        use crate::storage::{TableSchema, ColumnInfo};
        use crate::types::{DataType, Value};

        let mut engine = StorageEngine::new();
        engine.create_table(TableSchema {
            name: "items".to_string(),
            columns: vec![
                ColumnInfo { name: "id".to_string(), data_type: DataType::Integer, index: 0 },
                ColumnInfo { name: "emb".to_string(), data_type: DataType::Vector(3), index: 1 },
                ColumnInfo { name: "score".to_string(), data_type: DataType::Float, index: 2 },
            ],
        }).unwrap();
        engine.insert("items", vec![
            Value::Integer(1),
            Value::Vector(vec![0.1, 0.2, 0.3]),
            Value::Float(0.95),
        ]).unwrap();

        let state = engine.export_state();
        let mut engine2 = StorageEngine::new();
        engine2.import_state(&state).unwrap();

        let rows = engine2.scan_table("items").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].values[1], Value::Vector(vec![0.1, 0.2, 0.3]));
        assert_eq!(rows[0].values[2], Value::Float(0.95));
    }

    #[test]
    fn test_import_after_use_replaces_state() {
        // import_state 应清空旧状态，而不是叠加
        use crate::storage::{TableSchema, ColumnInfo};
        use crate::types::{DataType, Value};

        let mut engine = StorageEngine::new();
        engine.create_table(TableSchema {
            name: "old".to_string(),
            columns: vec![ColumnInfo { name: "id".to_string(), data_type: DataType::Integer, index: 0 }],
        }).unwrap();

        let state = build_engine().export_state();
        engine.import_state(&state).unwrap();

        assert_eq!(engine.table_names(), vec!["users".to_string()]);
        assert_eq!(engine.scan_table("users").unwrap().len(), 2);
    }
}
