# ProbeDB

AI 原生嵌入式数据库。Rust 实现，**零外部依赖**（没有任何第三方 crate，可离线编译）。

第一版目标：作为 Hermes 的记忆/状态存储后端——先做独立工具验证，再谈替换内部存储。

## 当前状态

| 项目 | 状态 |
|---|---|
| Rust 核心 | 约 8.6k 行，零第三方依赖 |
| 测试 | **213 个 Rust 测试 + 10 个 Python 测试，全绿** |
| Hermes 接入验证 | ✅ 已导入真实 `~/.hermes/memories`（46 条）并完成跨进程检索 |
| 版本 | v0.1.0（嵌入式，单进程） |

## 快速开始

```bash
# 1. 构建 + 跑测试
cargo test --release
cargo build --release --lib        # 产出 target/release/libprobedb.dylib

# 2. Python 绑定自检（CRUD / 列名重排 / 向量 / 持久化）
python3 python/probedb.py

# 3. Python 单元测试
python3 -m unittest discover -s python -v

# 4. Hermes 记忆工具：导入 → 跨进程检索
python3 tools/probedb_mem.py import --db ~/.probedb/hermes_memory.pdb
python3 tools/probedb_mem.py stats  --db ~/.probedb/hermes_memory.pdb
python3 tools/probedb_mem.py search "ProbeDB" --db ~/.probedb/hermes_memory.pdb
python3 tools/probedb_mem.py add "一条新记忆" --tag 测试 --db ~/.probedb/hermes_memory.pdb
```

`tools/probedb_mem.py` 用文件锁串行化多个 CLI 进程，导入/写入后自动 `persist()`，
因此每次调用都是"新进程 → 从磁盘恢复 → 检索"，天然验证持久化链路。

## 能力

- **SQL 子集**：`CREATE TABLE` / `INSERT`（列清单可任意顺序，也支持省略列清单）/ `SELECT`（`WHERE`、`LIKE`、`ORDER BY` 多列、`LIMIT/OFFSET`、`DISTINCT`、`GROUP BY` + `HAVING`、`SUM/AVG/MIN/MAX`、`INNER JOIN`）/ `UPDATE` / `DELETE` / `DROP TABLE`
- **类型**：`INTEGER`、`FLOAT`、`TEXT`、`BOOLEAN`、`DATE`、`TIME`、`VECTOR(n)`（向量是一等公民）
- **向量检索**：`WHERE vector_similarity(emb, '[1,2,3]') > 0.8 ORDER BY ... DESC`（余弦 / 欧氏，暴力搜索）
- **持久化**：全量快照 + WAL 增量 + FNV-1a 校验和 + 崩溃恢复；`persist()` 先 flush WAL 再原子写快照
- **跨语言**：C ABI（`src/ffi.rs`，8 个导出函数）+ Python ctypes 绑定（`python/probedb.py`），结果走结构化 JSON
- **字符串安全**：支持 SQL 标准 `''` 转义；字符串内的分号、括号、`|`、反斜杠、换行均不影响解析与存储

## 项目结构

```
src/
  lib.rs            # 公共 API（ProbeDB::open / execute / execute_json / persist）
  sql/mod.rs        # 自研 SQL 解析器（零依赖）
  types/mod.rs      # Value / DataType / 日期时间校验 / 向量相似度
  storage/mod.rs    # 内存存储引擎 + 文本行协议 + 值编解码
  executor/mod.rs   # 执行器（过滤/排序/聚合/JOIN/向量）
  persistence/      # 快照原子写 + WAL
  ffi.rs            # C ABI（8 个导出函数）
python/
  probedb.py        # 唯一的 Python 绑定（ctypes，结构化 Result）
  test_probedb.py   # Python 单元测试
tools/
  probedb_mem.py    # Hermes 记忆独立存储工具（导入/检索/统计/追加）
```

## 当前限制（明确边界）

- **存储模型**：数据全在内存，`persist()` 全量重写快照。适合个人记忆/Agent 状态这类小数据（MB 级），不适合 GB 级数据集。
- **无索引、无事务**：所有查询全表扫描；多行写入中途失败会留下部分写入。
- **无并发写**：一个句柄同一时刻只应被一个线程使用（Python 绑定已用 Lock 串行化；C 侧调用方自行保证）。
- **无 NULL / DEFAULT**：部分列插入会明确报错，而不是用零值填补。
- **JSON 行值统一为字符串**（VECTOR 形如 `[1,2,3]`）；类型化协议与参数绑定是下一步。
- **向量为暴力搜索**，HNSW 尚未实现。

## 路线图

1. **Hermes 记忆接入**（进行中）：独立 CLI/工具已跑通，下一步在 Hermes 侧挂载为记忆读写通道。
2. **协议升级**：类型化行值 + 参数绑定（消除拼接 SQL）。
3. **存储升级**：索引、事务、增量快照；再评估替换 Hermes 内部存储。
4. **AI 原生能力**：NL2SQL、自动索引、向量索引、自治（远期）。

## 开发约定

- 保持零外部依赖；`cargo build --offline` 必须可完成。
- 每次改动跑 `cargo test --release` 与 `python3 -m unittest discover -s python -v`。
- 重要决策与每日进展记录在 [DEVELOPMENT_LOG.md](DEVELOPMENT_LOG.md)。
- 接入路径采用"先独立工具验证，再替换内部存储"（方案 A），避免过早侵入 Hermes。
