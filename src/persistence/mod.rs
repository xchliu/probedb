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
/// 任何一步失败都会清理残留的 tmp 文件，不留垃圾。
pub fn save(engine: &StorageEngine, path: &str) -> Result<(), String> {
    let state = engine.export_state();
    let tmp_path = format!("{}.tmp", path);
    if let Err(e) = fs::write(&tmp_path, &state) {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("写入临时文件失败 ({}): {}", tmp_path, e));
    }
    if let Err(e) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("替换数据库文件失败 ({}): {}", path, e));
    }
    Ok(())
}

/// 当前快照格式版本（export_state 头部魔数）
const STATE_HEADER_V1: &str = "# ProbeDB state v1";

/// 从文件加载引擎状态
///
/// 文件不存在时返回 Err（调用方决定是报错还是新建空库）。
/// 做完整性校验：魔数/版本不匹配、空文件、损坏内容均返回 Err。
/// 校验和（若存在）验证失败也返回 Err——静默篡改/损坏无处遁形。
pub fn load(path: &str) -> Result<StorageEngine, String> {
    if !Path::new(path).exists() {
        return Err(format!("数据库文件不存在: {}", path));
    }
    let state = fs::read_to_string(path)
        .map_err(|e| format!("读取数据库文件失败 ({}): {}", path, e))?;
    // 完整性校验：空文件 / 非ProbeDB格式 / 版本不匹配 → 明确报错
    validate_header(&state)?;
    // 校验和：检测静默篡改/损坏（向后兼容：v1早期无校验快照跳过）
    validate_checksum(&state)?;
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

/// 校验快照校验和（最后一行 `# checksum <fnv1a64-hex>`）
///
/// 校验和覆盖校验行之前的全部字节，用于检测：
/// - 静默损坏（磁盘位翻转、半截写入但头部恰好完整）
/// - 恶意篡改（改数据但保留合法头部——头部校验拦不住）
///
/// 向后兼容：v1 早期（校验和引入前）的快照没有校验行 → 跳过校验。
/// 这样旧文件仍可加载，新保存的文件自动获得校验保护。
pub fn validate_checksum(state: &str) -> Result<(), String> {
    use crate::storage::fnv1a64;

    // 找最后一行是否为校验行
    let checksum_line = state.lines().rev().next().unwrap_or("").trim();
    if !checksum_line.starts_with("# checksum ") {
        // 无校验行 = 早期 v1 快照 → 跳过（向后兼容）
        return Ok(());
    }
    let expected = checksum_line["# checksum ".len()..].trim();
    let expected: u64 = u64::from_str_radix(expected, 16)
        .map_err(|_| format!("校验和格式无效: '{}'", checksum_line))?;

    // 计算校验和覆盖的内容：去掉最后一行（含行尾换行）
    let body = state.trim_end();
    let body = match body.rfind('\n') {
        Some(pos) => &body[..=pos], // 保留到倒数第二行末尾的换行，与 export 时一致
        None => "",
    };
    let actual = fnv1a64(body.as_bytes());
    if actual != expected {
        return Err(format!(
            "数据库文件校验和不匹配 (期望 {:016x}, 实际 {:016x})——文件已损坏或被篡改",
            expected, actual
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

    #[test]
    fn test_export_state_has_checksum_line() {
        // 新格式导出必须带校验行（损坏检测的依据）
        let state = build_engine().export_state();
        let last = state.lines().rev().next().unwrap_or("");
        assert!(
            last.starts_with("# checksum "),
            "导出的快照应以校验行结尾, got last line: '{}'",
            last
        );
    }

    #[test]
    fn test_validate_checksum_ok() {
        // 自身导出的快照 → 校验通过
        let state = build_engine().export_state();
        assert!(validate_checksum(&state).is_ok());
    }

    #[test]
    fn test_validate_checksum_detects_tampering() {
        // 篡改数据行（改名字）但保留合法头部 → 校验和不匹配，必须报错
        let state = build_engine().export_state();
        let tampered = state.replace("TEXT:alice", "TEXT:evil");
        assert_ne!(state, tampered, "篡改必须改变内容");
        let e = validate_checksum(&tampered).unwrap_err();
        assert!(
            e.contains("校验和不匹配"),
            "篡改应报校验和不匹配, got: {}",
            e
        );
    }

    #[test]
    fn test_validate_checksum_legacy_no_checksum_ok() {
        // 向后兼容：v1 早期无校验行的快照 → 跳过校验，正常接受
        let legacy = "# ProbeDB state v1\nSCHEMA|users|id:INTEGER|name:TEXT\nNEXTID|1\n";
        assert!(validate_checksum(legacy).is_ok());
    }

    #[test]
    fn test_load_detects_tampered_file() {
        // 篡改落盘文件（改数据但保留头部+旧校验行）→ load 必须失败
        let tmp = std::env::temp_dir().join("probedb_tampered.pdb");
        let path = tmp.to_str().unwrap().to_string();
        let _ = fs::remove_file(&path);

        save(&build_engine(), &path).unwrap();
        // 读文件、篡改数据、写回
        let content = fs::read_to_string(&path).unwrap();
        let tampered = content.replace("TEXT:alice", "TEXT:hacked");
        fs::write(&path, &tampered).unwrap();

        let r = load(&path);
        assert!(r.is_err(), "篡改文件应加载失败");

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(format!("{}.tmp", path));
    }

    #[test]
    fn test_save_to_nonexistent_dir_errors_and_cleans_tmp() {
        // 目标目录不存在 → save 明确报错，且不残留 .tmp 垃圾文件
        let path = format!(
            "{}/probedb_no_such_dir/probedb_x.pdb",
            std::env::temp_dir().to_str().unwrap()
        );
        let tmp_path = format!("{}.tmp", path);
        // 确保目录不存在
        let dir = std::path::Path::new(&path).parent().unwrap();
        let _ = fs::remove_dir_all(dir);

        let r = save(&build_engine(), &path);
        assert!(r.is_err(), "不存在目录下 save 应失败");
        assert!(
            !std::path::Path::new(&tmp_path).exists(),
            "save 失败后不应残留 tmp 文件"
        );
    }

    #[test]
    fn test_save_cleanup_tmp_on_rename_failure() {
        // 目标已存在且是目录 → rename 失败 → save 报错且清理 tmp
        let dir = std::env::temp_dir().join("probedb_rename_dir_target");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // 目标"文件"实际上是个目录 → rename(tmp, dir) 失败
        let path = dir.to_str().unwrap().to_string();
        let tmp_path = format!("{}.tmp", path);
        // 先手动造一个 tmp 文件，模拟写成功后 rename 失败场景
        // save 会先覆盖 tmp（fs::write 成功），然后 rename 失败
        let r = save(&build_engine(), &path);
        assert!(r.is_err(), "rename 到目录应失败");
        assert!(
            !std::path::Path::new(&tmp_path).exists(),
            "rename 失败后应清理 tmp 文件"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_returns_clear_io_errors() {
        // IO 错误信息应包含路径和原因，便于定位（而非裸 panic）
        let path = format!(
            "{}/probedb_no_such_dir/probedb_missing.pdb",
            std::env::temp_dir().to_str().unwrap()
        );
        let e = load(&path).unwrap_err();
        assert!(e.contains("数据库文件不存在"), "缺失文件报错不清晰: {}", e);
    }
}
