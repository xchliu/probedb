# ProbeDB

AI 原生嵌入式数据库。Rust 实现，**零外部依赖**（没有任何第三方 crate，可离线编译）。

第一版目标：作为 Hermes 的记忆/状态存储后端——先做独立工具验证，再谈替换内部存储。

## 当前状态

| 项目 | 状态 |
|---|---|
| Rust 核心 | 约 9k 行，零第三方依赖 |
| 测试 | **232 个 Rust 测试 + 31 个 Python 测试，全绿**（性能断言需 `PROBEDB_PERF_STRICT=1` 才严格失败） |
| Hermes 接入验证 | ✅ 已导入真实 `~/.hermes/memories`（46 条）并完成跨进程检索 |
| SelfMind 兼容验收 | ✅ [tools/selfmind_compat.py](tools/selfmind_compat.py) PASS（schema/约束/历史表写入）|
| SelfMind 双读比对 | ✅ [tools/selfmind_dualread.py](tools/selfmind_dualread.py) PASS（真实 612 条 entries，SQLite vs ProbeDB 逐行一致） |
| SelfMind 安装包 | ✅ [integrations/selfmind](integrations/selfmind/README.md) 在源码副本上安装并验证 PASS（612 条逐行一致） |
| 候选评估 | [Hermes](docs/hermes-integration.md) · [SelfMind 迁移评估](docs/selfmind-migration-assessment.md) |
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
python3 tools/probedb_mem.py get 4   --db ~/.probedb/hermes_memory.pdb
python3 tools/probedb_mem.py add "一条新记忆" --tag 测试 --db ~/.probedb/hermes_memory.pdb
python3 tools/probedb_mem.py update 47 --content "修正后的内容" --db ~/.probedb/hermes_memory.pdb
python3 tools/probedb_mem.py delete 47 --yes --db ~/.probedb/hermes_memory.pdb

# 5. SelfMind 兼容验收（schema 初始化 / 表别名 / 参数化）
python3 tools/selfmind_compat.py

# 6. 基准测试（100/1k/5k/10k 条）
python3 tools/bench_probedb.py --sizes 100,1000,5000,10000
```

`tools/probedb_mem.py` 用文件锁串行化多个 CLI 进程，导入/写入后自动 `persist()`，
因此每次调用都是"新进程 → 从磁盘恢复 → 检索"，天然验证持久化链路。

## 能力

- **SQL 子集**：`CREATE TABLE [IF NOT EXISTS]`、`INSERT`（列清单可任意顺序、可省略列清单，支持部分列 + `DEFAULT` / `AUTOINCREMENT` / `NOT NULL`）、`SELECT`（`WHERE`、`LIKE`、`IN / NOT IN`、`IS NULL / IS NOT NULL`、`ORDER BY` 多列、`LIMIT/OFFSET`、`DISTINCT`、`GROUP BY` + `HAVING`、`SUM/AVG/MIN/MAX`、`COUNT(DISTINCT col)`、`INNER JOIN` + **表别名**、**聚合列别名**）、`UPDATE`（支持 `col=col+1`）、`DELETE`、`DROP TABLE`、`CREATE [UNIQUE] INDEX [IF NOT EXISTS]`（兼容层：校验后接受，暂不建物理索引）
- **列约束**：`DEFAULT` / `AUTOINCREMENT` / `NOT NULL` 会随快照与 WAL 一起持久化、重启后继续生效；省略的自增列自动分配并与隐藏 row_id 对齐
- **NULL 语义**：支持写入/查询 `NULL`（`IS NULL` / `IS NOT NULL`），NULL 结果以 `"NULL"` 返回（行值协议尚未类型化）
- **聚合**：单列/多列聚合、`COUNT(DISTINCT col)`、`GROUP BY` 聚合、聚合列别名
- **SQL 兼容细节**：`--` 行注释、`''` 转义、字符串内分号/括号、`IF NOT EXISTS` 幂等、表级约束子句跳过；WHERE 操作符两侧可有/无空格
- **参数化 API**（Python）：`db.execute(sql, params=(...))`、`execute_all`、`executescript`、`commit`、`last_row_id`、`Result.records`；`None` → SQL NULL
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
  test_selfmind_conn.py # sqlite3 兼容层测试
  test_selfmind_install.py # 安装器测试
integrations/selfmind/
  README.md         # 安装/启用/回滚说明
  probedb_conn.py   # sqlite3.Connection 兼容层
  install.py        # 安装器（复制兼容层 + 最小补丁，幂等/可回滚）
tools/
  probedb_mem.py    # Hermes 记忆独立存储工具（导入/检索/增删改/统计）
  bench_probedb.py  # 记忆场景基准测试（导入/检索/CLI 延迟）
  selfmind_compat.py # SelfMind Phase 5.5 兼容验收（schema/别名/参数化/约束）
  selfmind_dualread.py # SelfMind SQLite vs ProbeDB 双读比对
  selfmind_landing_check.py # 安装包在源码副本上的端到端验证
docs/
  hermes-integration.md            # Hermes 候选评估：实测数据、接入方案、风险
  selfmind-migration-assessment.md # SelfMind 迁移评估：验收结果、缺口、758MB 规模分析
  selfmind-adapter-plan.md         # ProbeDBConnection 适配层设计 + 分阶段迁移
```

## 当前限制（明确边界）

- **存储模型**：数据全在内存，`persist()` 全量重写快照。适合个人记忆/Agent 状态这类小数据（MB 级），不适合 GB 级数据集。
- **无索引、无事务**：`CREATE INDEX` 仅语法兼容（校验后接受），查询仍全表扫描；多行写入中途失败会留下部分写入。
- **无并发写**：一个句柄同一时刻只应被一个线程使用（Python 绑定已用 Lock 串行化；C 侧调用方自行保证）。
- **约束语义**：`DEFAULT` / `AUTOINCREMENT` / `NOT NULL` 生效；`PRIMARY KEY` / `UNIQUE` / `FOREIGN KEY` 仅语法接受、不强制。
- **SQL 兼容缺口（P1）**：`DATE()` 等函数、`last_insert_rowid()` SQL 函数、`GROUP BY 表达式`、事务 `BEGIN/COMMIT` 尚未支持。
- **JSON 行值统一为字符串**（NULL → `"NULL"`，VECTOR → `"[1,2,3]"`）；类型化协议是下一步。
- **向量为暴力搜索**，HNSW 尚未实现。

## 路线图

1. **SelfMind 迁移前置** ✅：4 条验收标准 PASS；SQL 兼容缺口全部补齐（NULL / IN / 别名 / 表达式 UPDATE / DEFAULT+AUTOINCREMENT / 部分列插入）。
2. **SelfMind 适配层**（待落地）：安装包 + 双读 + 安装包验证均已完成（612 条 entries 全部一致）；[方案](docs/selfmind-adapter-plan.md)；只差 SelfMind 写权限执行 `integrations/selfmind/install.py`。
3. **Hermes 记忆接入**：独立 CLI/工具已跑通，下一步在 Hermes 侧挂载为记忆读写通道。
4. **协议升级**：类型化行值 + C 层参数绑定（当前参数绑定在 Python 客户端完成）。
5. **存储升级**：索引、事务、增量快照；再评估替换内部存储。
6. **AI 原生能力**：NL2SQL、自动索引、向量索引、自治（远期）。

## 开发约定

- 保持零外部依赖；`cargo build --offline` 必须可完成。
- 每次改动跑 `cargo test --release` 与 `python3 -m unittest discover -s python -v`。
- 重要决策与每日进展记录在 [DEVELOPMENT_LOG.md](DEVELOPMENT_LOG.md)。
- 接入路径采用"先独立工具验证，再替换内部存储"（方案 A），避免过早侵入 Hermes。
