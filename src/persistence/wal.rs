// ProbeDB WAL（Write-Ahead Log）— 追加式操作日志（零外部依赖）
//
// 职责：
// - 记录每个 DML 操作（CREATE TABLE / INSERT / DELETE / UPDATE），先写 WAL 再改内存
// - 崩溃恢复：启动时 load 全量快照 + replay WAL → 恢复到崩溃点状态
// - 快照+WAL 组合：全量快照定期落盘（persist），成功后 truncate WAL
//
// 格式 v1（文本行协议，与快照格式一致）：
//   # ProbeDB WAL v1
//   T|<table>|<col>:<type>|...              CREATE TABLE
//   I|<table>|<row_id>|<value>|...          INSERT（显式 row_id，重放幂等）
//   D|<table>|<row_id>                      DELETE（重放幂等：删不存在的行无害）
//   U|<table>|<row_id>|<col_index>|<value>  UPDATE（重放幂等：重复赋值无害）
//
// 幂等设计：快照已包含 WAL 记录时重放不产生副作用——
//   - CREATE TABLE：表已存在则跳过
//   - INSERT：行已存在则跳过
//   - DELETE/UPDATE：对不存在的行无操作

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use crate::storage::{self, ColumnInfo, StorageEngine, TableSchema};
use crate::types::Value;

/// WAL 日志文件句柄
pub struct WalLog {
    path: String,
    file: fs::File,
}

impl WalLog {
    /// 打开（或创建）WAL 文件
    ///
    /// 追加模式打开：进程崩溃重启后从上次位置继续写，不覆盖已有记录。
    pub fn open(path: &str) -> Result<Self, String> {
        let exists = Path::new(path).exists();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("打开WAL文件失败 ({}): {}", path, e))?;
        let mut log = WalLog {
            path: path.to_string(),
            file,
        };
        if !exists {
            log.write_line("# ProbeDB WAL v1")?;
        }
        Ok(log)
    }

    /// 追加一条 WAL 记录（写盘并 flush）
    pub fn append(&mut self, entry: &str) -> Result<(), String> {
        self.write_line(entry)
    }

    /// 清空 WAL（快照落盘成功后调用）
    pub fn truncate(&mut self) -> Result<(), String> {
        let new_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.path)
            .map_err(|e| format!("截断WAL文件失败 ({}): {}", self.path, e))?;
        self.file = new_file;
        self.write_line("# ProbeDB WAL v1")
    }

    /// 当前 WAL 记录数（排除注释/空行）
    pub fn entry_count(&self) -> Result<usize, String> {
        let content = fs::read_to_string(&self.path)
            .map_err(|e| format!("读取WAL文件失败 ({}): {}", self.path, e))?;
        Ok(content
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .count())
    }

    /// 重放 WAL 到引擎（幂等）
    ///
    /// 返回重放的记录条数。损坏/无法解析的行跳过并计数，不整体失败
    /// ——恢复韧性：一条坏记录不能毁掉整个库。
    pub fn replay(&self, engine: &mut StorageEngine) -> Result<usize, String> {
        let content = fs::read_to_string(&self.path)
            .map_err(|e| format!("读取WAL文件失败 ({}): {}", self.path, e))?;
        let mut replayed = 0;
        let mut skipped = 0;
        for (lineno, line) in content.lines().enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts = storage::split_pipe_aware(line);
            if parts.is_empty() {
                continue;
            }
            let result = match parts[0] {
                "T" => replay_create(engine, &parts),
                "I" => replay_insert(engine, &parts),
                "D" => replay_delete(engine, &parts),
                "U" => replay_update(engine, &parts),
                other => Err(format!("未知WAL操作码: {}", other)),
            };
            match result {
                Ok(()) => replayed += 1,
                Err(e) => {
                    skipped += 1;
                    eprintln!(
                        "[wal] 跳过损坏记录 第{}行 ({}): {}",
                        lineno + 1,
                        line,
                        e
                    );
                }
            }
        }
        if skipped > 0 {
            eprintln!("[wal] 共跳过 {} 条损坏记录", skipped);
        }
        Ok(replayed)
    }

    fn write_line(&mut self, line: &str) -> Result<(), String> {
        writeln!(self.file, "{}", line)
            .and_then(|_| self.file.flush())
            .map_err(|e| format!("写入WAL失败 ({}): {}", self.path, e))
    }
}

/// 重放 CREATE TABLE（幂等：表已存在则跳过）
fn replay_create(engine: &mut StorageEngine, parts: &[&str]) -> Result<(), String> {
    if parts.len() < 3 {
        return Err("字段不足".to_string());
    }
    let name = parts[1].to_string();
    let mut columns = Vec::new();
    for (i, col) in parts[2..].iter().enumerate() {
        let (col_name, type_str) = col
            .split_once(':')
            .ok_or_else(|| format!("列格式错误: {}", col))?;
        columns.push(ColumnInfo {
            name: col_name.to_string(),
            data_type: storage::decode_type(type_str)?,
            index: i,
        });
    }
    if engine.get_schema(&name).is_ok() {
        return Ok(());
    }
    engine.create_table(TableSchema { name, columns })
}

/// 重放 INSERT（幂等：行已存在则跳过）
fn replay_insert(engine: &mut StorageEngine, parts: &[&str]) -> Result<(), String> {
    if parts.len() < 4 {
        return Err("字段不足".to_string());
    }
    let table = parts[1].to_string();
    let id: u64 = parts[2]
        .parse()
        .map_err(|_| format!("id解析失败: {}", parts[2]))?;
    let values: Result<Vec<Value>, String> =
        parts[3..].iter().map(|s| storage::decode_value(s)).collect();
    let values = values?;
    if let Ok(rows) = engine.scan_table(&table) {
        if rows.iter().any(|r| r.id == id) {
            return Ok(());
        }
    }
    engine.insert_row_with_id(&table, id, values)
}

/// 重放 DELETE（幂等：删不存在的行无操作）
fn replay_delete(engine: &mut StorageEngine, parts: &[&str]) -> Result<(), String> {
    if parts.len() < 3 {
        return Err("字段不足".to_string());
    }
    let table = parts[1].to_string();
    let id: u64 = parts[2]
        .parse()
        .map_err(|_| format!("id解析失败: {}", parts[2]))?;
    engine.delete_by_ids(&table, &[id])?;
    Ok(())
}

/// 重放 UPDATE（幂等：对不存在的行无操作）
fn replay_update(engine: &mut StorageEngine, parts: &[&str]) -> Result<(), String> {
    if parts.len() < 5 {
        return Err("字段不足".to_string());
    }
    let table = parts[1].to_string();
    let id: u64 = parts[2]
        .parse()
        .map_err(|_| format!("id解析失败: {}", parts[2]))?;
    let col_index: usize = parts[3]
        .parse()
        .map_err(|_| format!("col_index解析失败: {}", parts[3]))?;
    let value = storage::decode_value(parts[4])?;
    engine.update_by_ids(&table, &[id], col_index, value)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ColumnInfo, TableSchema};
    use crate::types::{DataType, Value};

    fn wal_path(name: &str) -> String {
        std::env::temp_dir().join(name).to_str().unwrap().to_string()
    }

    fn build_engine() -> StorageEngine {
        let mut engine = StorageEngine::new();
        engine
            .create_table(TableSchema {
                name: "users".to_string(),
                columns: vec![
                    ColumnInfo {
                        name: "id".to_string(),
                        data_type: DataType::Integer,
                        index: 0,
                    },
                    ColumnInfo {
                        name: "name".to_string(),
                        data_type: DataType::Text,
                        index: 1,
                    },
                ],
            })
            .unwrap();
        engine
            .insert(
                "users",
                vec![Value::Integer(1), Value::Text("alice".to_string())],
            )
            .unwrap();
        engine
    }

    #[test]
    fn test_wal_append_and_count() {
        let path = wal_path("probedb_wal_count.wal");
        let _ = fs::remove_file(&path);

        let mut wal = WalLog::open(&path).unwrap();
        assert_eq!(wal.entry_count().unwrap(), 0);

        wal.append("I|users|1|INT:1|TEXT:alice").unwrap();
        wal.append("D|users|1").unwrap();
        wal.append("U|users|1|1|TEXT:bob").unwrap();
        assert_eq!(wal.entry_count().unwrap(), 3);

        // 追加模式：重新 open 不清空已有记录
        drop(wal);
        let wal2 = WalLog::open(&path).unwrap();
        assert_eq!(wal2.entry_count().unwrap(), 3);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_wal_replay_restores_operations() {
        let path = wal_path("probedb_wal_replay.wal");
        let _ = fs::remove_file(&path);

        let mut wal = WalLog::open(&path).unwrap();
        // CREATE TABLE + INSERT
        wal.append("T|users|id:INTEGER|name:TEXT").unwrap();
        wal.append("I|users|1|INT:1|TEXT:alice").unwrap();
        wal.append("I|users|2|INT:2|TEXT:bob\\|smith").unwrap(); // 含 | 转义
        // UPDATE id=1 name → alice2
        wal.append("U|users|1|1|TEXT:alice2").unwrap();
        // DELETE id=2
        wal.append("D|users|2").unwrap();

        let mut engine = StorageEngine::new();
        let replayed = wal.replay(&mut engine).unwrap();
        assert_eq!(replayed, 5, "5条记录应全部重放");

        let rows = engine.scan_table("users").unwrap();
        assert_eq!(rows.len(), 1, "DELETE后应剩1行");
        assert_eq!(rows[0].id, 1);
        assert_eq!(rows[0].values[1], Value::Text("alice2".to_string()), "UPDATE应生效");
        assert_eq!(engine.next_id(), 1, "重放不应消耗自增ID");
    }

    #[test]
    fn test_wal_replay_idempotent() {
        let path = wal_path("probedb_wal_idem.wal");
        let _ = fs::remove_file(&path);

        let mut wal = WalLog::open(&path).unwrap();
        wal.append("T|users|id:INTEGER|name:TEXT").unwrap();
        wal.append("I|users|1|INT:1|TEXT:alice").unwrap();

        // 场景：persist 成功后 WAL 未 truncate 就崩溃 → 下次 open 时
        // 快照里已有这些数据，WAL 重放必须幂等（不重复插行）
        let mut engine = build_engine(); // 快照恢复后的引擎：已含 users 表 + alice(id=1)
        let replayed = wal.replay(&mut engine).unwrap();
        assert_eq!(replayed, 2, "记录全部重放（幂等跳过）");

        let rows = engine.scan_table("users").unwrap();
        assert_eq!(rows.len(), 1, "重复重放不得产生重复行");
        assert_eq!(rows[0].values[1], Value::Text("alice".to_string()));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_wal_corrupt_line_skipped() {
        let path = wal_path("probedb_wal_corrupt.wal");
        let _ = fs::remove_file(&path);

        let mut wal = WalLog::open(&path).unwrap();
        wal.append("T|users|id:INTEGER|name:TEXT").unwrap();
        wal.append("GARBAGE|no|format").unwrap(); // 未知操作码
        wal.append("I|users|1|INT:1|TEXT:alice").unwrap();
        wal.append("I|users|2|NOT_A_VALUE").unwrap(); // 损坏值

        let mut engine = StorageEngine::new();
        let replayed = wal.replay(&mut engine).unwrap();
        assert_eq!(replayed, 2, "损坏行跳过，其余正常重放");

        let rows = engine.scan_table("users").unwrap();
        assert_eq!(rows.len(), 1, "alice 恢复，损坏行不影响");

        let _ = fs::remove_file(&path);
    }
}
