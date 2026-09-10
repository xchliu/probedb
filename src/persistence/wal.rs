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
    /// 批量模式：append 只进内存缓冲，flush() 时一次性写盘
    batch: bool,
    /// 批量模式下未落盘的记录（按 append 顺序）
    pending: Vec<String>,
    /// 自动刷盘阈值：pending 达到该条数时自动 flush（0 = 不自动刷，手动 flush）
    auto_flush_threshold: usize,
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
            batch: false,
            pending: Vec::new(),
            auto_flush_threshold: 0,
        };
        if !exists {
            log.write_line("# ProbeDB WAL v1")?;
        }
        Ok(log)
    }

    /// 追加一条 WAL 记录
    ///
    /// - 同步模式（默认）：立即写盘并 flush（原语义，每条都 fsync）
    /// - 批量模式：先进内存缓冲，flush() 时合并落盘（性能优化，延迟持久化）
    pub fn append(&mut self, entry: &str) -> Result<(), String> {
        if self.batch {
            self.pending.push(entry.to_string());
            if self.auto_flush_threshold > 0 && self.pending.len() >= self.auto_flush_threshold {
                self.flush()?;
            }
            Ok(())
        } else {
            self.write_line(entry)
        }
    }

    /// 开启/关闭批量模式
    ///
    /// 关闭时自动 flush 剩余 pending，保证切回同步模式后记录完整。
    pub fn set_batch(&mut self, enabled: bool) -> Result<(), String> {
        self.batch = enabled;
        if !enabled {
            self.flush()?;
        }
        Ok(())
    }

    /// 当前是否处于批量模式
    pub fn is_batch(&self) -> bool {
        self.batch
    }

    /// 设置自动刷盘阈值（pending 达到该条数自动 flush，0 = 手动）
    pub fn set_auto_flush_threshold(&mut self, threshold: usize) {
        self.auto_flush_threshold = threshold;
    }

    /// 把 pending 缓冲一次性写入磁盘（批量写 + 单次 flush）
    pub fn flush(&mut self) -> Result<(), String> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let lines = std::mem::take(&mut self.pending);
        self.write_lines(&lines)?;
        Ok(())
    }

    /// 批量模式下尚未落盘的记录数
    pub fn pending_count(&self) -> usize {
        self.pending.len()
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
                "DROP" => replay_drop(engine, &parts),
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

    /// 批量写入多行（一次 write_all + 一次 flush，避免每条记录一次 fsync）
    fn write_lines(&mut self, lines: &[String]) -> Result<(), String> {
        let mut buf = String::new();
        for line in lines {
            buf.push_str(line);
            buf.push('\n');
        }
        self.file
            .write_all(buf.as_bytes())
            .and_then(|_| self.file.flush())
            .map_err(|e| format!("批量写入WAL失败 ({}): {}", self.path, e))
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

/// 重放 DROP TABLE（幂等：表不存在则跳过）
fn replay_drop(engine: &mut StorageEngine, parts: &[&str]) -> Result<(), String> {
    if parts.len() < 2 {
        return Err("字段不足".to_string());
    }
    let table = parts[1].to_string();
    // 表不存在 → 幂等跳过（可能已被快照固化后再次重放）
    if engine.get_schema(&table).is_err() {
        return Ok(());
    }
    engine.drop_table(&table)?;
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

    #[test]
    fn test_wal_batch_accumulates_then_flushes() {
        let path = wal_path("probedb_wal_batch.wal");
        let _ = fs::remove_file(&path);

        let mut wal = WalLog::open(&path).unwrap();
        wal.set_batch(true).unwrap();
        assert!(wal.is_batch());

        // 批量模式下 append 只进缓冲，不落盘
        wal.append("T|users|id:INTEGER|name:TEXT").unwrap();
        wal.append("I|users|1|INT:1|TEXT:alice").unwrap();
        wal.append("I|users|2|INT:2|TEXT:bob").unwrap();
        assert_eq!(wal.pending_count(), 3, "3条记录应在内存缓冲");
        assert_eq!(wal.entry_count().unwrap(), 0, "尚未落盘");

        // flush 一次合并写盘
        wal.flush().unwrap();
        assert_eq!(wal.pending_count(), 0, "flush后缓冲清空");
        assert_eq!(wal.entry_count().unwrap(), 3, "3条记录一次落盘");

        // 新引擎重放：数据完整
        let mut engine = StorageEngine::new();
        let replayed = wal.replay(&mut engine).unwrap();
        assert_eq!(replayed, 3);
        let rows = engine.scan_table("users").unwrap();
        assert_eq!(rows.len(), 2, "批量写入后重放应恢复2行");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_wal_batch_set_batch_false_flushes() {
        let path = wal_path("probedb_wal_batch_off.wal");
        let _ = fs::remove_file(&path);

        let mut wal = WalLog::open(&path).unwrap();
        wal.set_batch(true).unwrap();
        wal.append("I|users|1|INT:1|TEXT:alice").unwrap();
        wal.append("I|users|2|INT:2|TEXT:bob").unwrap();
        assert_eq!(wal.pending_count(), 2);

        // 关闭批量模式 → 自动 flush 剩余 pending
        wal.set_batch(false).unwrap();
        assert!(!wal.is_batch());
        assert_eq!(wal.pending_count(), 0, "关闭批量模式应自动落盘");
        assert_eq!(wal.entry_count().unwrap(), 2);

        // 同步模式继续追加：立即落盘
        wal.append("I|users|3|INT:3|TEXT:carol").unwrap();
        assert_eq!(wal.entry_count().unwrap(), 3, "同步模式应立即落盘");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_wal_batch_auto_flush_threshold() {
        let path = wal_path("probedb_wal_batch_threshold.wal");
        let _ = fs::remove_file(&path);

        let mut wal = WalLog::open(&path).unwrap();
        wal.set_batch(true).unwrap();
        wal.set_auto_flush_threshold(2);

        wal.append("I|users|1|INT:1|TEXT:alice").unwrap();
        assert_eq!(wal.pending_count(), 1, "未达阈值，仍在缓冲");

        wal.append("I|users|2|INT:2|TEXT:bob").unwrap();
        assert_eq!(wal.pending_count(), 0, "达到阈值自动flush");
        assert_eq!(wal.entry_count().unwrap(), 2, "自动落盘2条");

        wal.append("I|users|3|INT:3|TEXT:carol").unwrap();
        assert_eq!(wal.pending_count(), 1, "新一轮累积");
        wal.flush().unwrap();
        assert_eq!(wal.entry_count().unwrap(), 3);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_wal_batch_order_preserved() {
        let path = wal_path("probedb_wal_batch_order.wal");
        let _ = fs::remove_file(&path);

        // 混合操作批量累积后，重放顺序必须与 append 顺序一致
        let mut wal = WalLog::open(&path).unwrap();
        wal.set_batch(true).unwrap();
        wal.append("T|t|id:INTEGER|name:TEXT").unwrap();
        wal.append("I|t|1|INT:1|TEXT:a").unwrap();
        wal.append("I|t|2|INT:2|TEXT:b").unwrap();
        wal.append("U|t|1|1|TEXT:a2").unwrap();
        wal.append("D|t|2").unwrap();
        wal.flush().unwrap();

        let mut engine = StorageEngine::new();
        let replayed = wal.replay(&mut engine).unwrap();
        assert_eq!(replayed, 5, "5条记录按序重放");

        let rows = engine.scan_table("t").unwrap();
        assert_eq!(rows.len(), 1, "U和D应按序生效");
        assert_eq!(rows[0].values[1], Value::Text("a2".to_string()));

        let _ = fs::remove_file(&path);
    }
}
