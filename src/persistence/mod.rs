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

pub mod wal;

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

/// 当前快照格式版本（export_state 头部魔数）
const STATE_HEADER_V1: &str = "# ProbeDB state v1";

/// 从文件加载引擎状态
///
/// 文件不存在时返回 Err（调用方决定是报错还是新建空库）。
/// 做完整性校验：魔数/版本不匹配、空文件、损坏内容均返回 Err。
pub fn load(path: &str) -> Result<StorageEngine, String> {
    if !Path::new(path).exists() {
        return Err(format!("数据库文件不存在: {}", path));
    }
    let state = fs::read_to_string(path)
        .map_err(|e| format!("读取数据库文件失败 ({}): {}", path, e))?;
    // 完整性校验：空文件 / 非ProbeDB格式 / 版本不匹配 → 明确报错
    validate_header(&state)?;
    let mut engine = StorageEngine::new();
    engine.import_state(&state)?;
    Ok(engine)
}

/// 校验快照头部（版本/魔数），返回错误信息或 Ok(())
///
/// 独立于 import_state 的格式检查，用于：
/// - 空文件 / 不是ProbeDB格式 → 明确报错（避免把任意文本当数据库）
/// - 未来版本升级：v2 格式由调用方决定迁移或拒绝
pub fn validate_header(state: &str) -> Result<(), String> {
    let first_line = state.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return Err("数据库文件为空".to_string());
    }
    if !first_line.starts_with("# ProbeDB state") {
        return Err(format!(
            "数据库文件头无效（不是ProbeDB快照或已损坏）: '{}'",
            first_line
        ));
    }
    // 已知版本：v1。未知更高版本 → 拒绝（防止新版本写的数据被旧版本误读）
    if first_line != STATE_HEADER_V1 {
        return Err(format!(
            "数据库版本不匹配: 期望 '{}'，实际 '{}'",
            STATE_HEADER_V1, first_line
        ));
    }
    Ok(())
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
        use crate::storage::{ColumnInfo, TableSchema};
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

    #[test]
    fn test_validate_header_ok() {
        // 正常快照头部应通过校验
        let state = build_engine().export_state();
        assert!(validate_header(&state).is_ok());
    }

    #[test]
    fn test_validate_header_empty_file() {
        // 空文件 → 明确报错
        let e = validate_header("").unwrap_err();
        assert!(e.contains("空"), "空文件应报'空'错误, got: {}", e);
    }

    #[test]
    fn test_validate_header_not_probedb_format() {
        // 任意文本（非ProbeDB格式）→ 明确报错
        let e = validate_header("hello world, this is not a database").unwrap_err();
        assert!(e.contains("无效"), "非ProbeDB格式应报'无效', got: {}", e);
    }

    #[test]
    fn test_validate_header_version_mismatch() {
        // 未来版本（v2）→ 版本不匹配报错，防止旧版误读新版数据
        let e = validate_header("# ProbeDB state v2\nSCHEMA|t|id:INTEGER").unwrap_err();
        assert!(e.contains("版本不匹配"), "v2应报版本不匹配, got: {}", e);
    }

    #[test]
    fn test_load_corrupted_file_errors() {
        // 损坏的数据库文件（非ProbeDB格式）→ load 明确报错
        let tmp = std::env::temp_dir().join("probedb_corrupt.pdb");
        let path = tmp.to_str().unwrap().to_string();
        fs::write(&path, "this is garbage data, not a probedb snapshot").unwrap();

        let r = load(&path);
        assert!(r.is_err(), "损坏文件应加载失败");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_empty_file_errors() {
        // 空数据库文件 → load 明确报错（而非静默当空库）
        let tmp = std::env::temp_dir().join("probedb_empty.pdb");
        let path = tmp.to_str().unwrap().to_string();
        fs::write(&path, "").unwrap();

        let r = load(&path);
        assert!(r.is_err(), "空文件应加载失败");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_save_load_empty_database() {
        // 空库（只有表结构，无数据）保存→加载→表结构完整
        let tmp = std::env::temp_dir().join("probedb_empty_db.pdb");
        let path = tmp.to_str().unwrap().to_string();
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(format!("{}.tmp", path));

        let mut engine = StorageEngine::new();
        use crate::storage::{ColumnInfo, TableSchema};
        use crate::types::DataType;
        engine.create_table(TableSchema {
            name: "empty".to_string(),
            columns: vec![
                ColumnInfo { name: "id".to_string(), data_type: DataType::Integer, index: 0 },
                ColumnInfo { name: "emb".to_string(), data_type: DataType::Vector(2), index: 1 },
            ],
        }).unwrap();
        save(&engine, &path).unwrap();

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.table_names(), vec!["empty".to_string()]);
        assert_eq!(loaded.scan_table("empty").unwrap().len(), 0, "空表加载后仍为空");

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(format!("{}.tmp", path));
    }
}
