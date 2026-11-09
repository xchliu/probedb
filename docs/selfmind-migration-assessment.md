# SelfMind × ProbeDB 迁移评估（Phase 5.5）

> 2026-10-26 · 输入：Hermes（苏格拉底）的 `docs/selfmind-integration-requirements.md`
> 验收脚本：`python3 tools/selfmind_compat.py` → **PASS**

## 一、Hermes 给出的 4 条验收标准

| # | 标准 | 结果 | 证据 |
|:--:|---|:--:|---|
| 1 | `CREATE INDEX IF NOT EXISTS idx ON t(col)` 不报错 | ✅ | schema 初始化中的 14 个索引全部接受；重复执行幂等 |
| 2 | `SELECT e.id FROM entries e JOIN meta m ON e.id=m.eid` 可跑 | ✅ | 别名 JOIN 返回 `e1`（含 ON 右表在前的写法） |
| 3 | `execute(sql, params=("x",))` 或等价 API | ✅ | Python 绑定新增 `params=`；参数化 SELECT/LIKE/UPDATE 通过 |
| 4 | `unified_store.SCHEMA_SQL` 初始化通过 | ✅ | 20/20 语句；连续执行两次（幂等） |

## 二、本次补齐的能力

- **SQL 注释**：`--` 行注释（字符串内不处理）；`executescript` 整段 schema 可直接跑。
- **DDL**：`CREATE TABLE IF NOT EXISTS`；`CREATE [UNIQUE] INDEX IF NOT EXISTS`（校验表和列，当前不建物理索引）；表级 `FOREIGN KEY / PRIMARY KEY / UNIQUE / CHECK / CONSTRAINT` 子句跳过（不建模）。
- **表别名**：`FROM entries e INNER JOIN decay_history dh ON dh.entry_id = e.id`，并修正了 ON 两侧顺序与 FROM/JOIN 不一致的情况。
- **参数化 API**：`db.execute(sql, params=(...))` / `execute_all`；`?` 在客户端安全转成 SQL 字面量（跳过字符串/注释），参数数量与 `None` 有明确错误。
- **sqlite3 兼容辅助**：`executescript()`、`commit()`（no-op）、`last_row_id`、`Result.records`（按列名取行）。
- **正确性修复**：WHERE 操作符两侧无空格（`WHERE id='e1'`）现在可解析；不支持的谓词（如 `IN`）**显式报错**而不是静默返回空集。

测试：Rust **225 passed** / Python **20 passed**。

## 三、requirements 文档之外的新缺口（实测发现）

Hermes 的文档只列了 3 个阻断项；实际 `unified_store.py` 还需要（状态更新于 2026-11-09）：

| 缺口 | 实际用法 | 现状 | 影响 |
|---|---|---|---|
| **NULL** | 27 列 entries 中大量可空字段；32 处参数化查询可能传 `None` | ✅ 已支持（`None` → NULL，`IS NULL/IS NOT NULL`；结果以 `"NULL"` 返回） | 已解决 |
| **省略自增 id 的 INSERT** | `entry_history / snapshots / decay_history / operations_log / agent_recall_log` 都不传 `id` | ❌ 仍不支持部分列插入 | **剩余 P0** |
| **DEFAULT / AUTOINCREMENT 语义** | DDL 中大量 `DEFAULT ...`、`id INTEGER PRIMARY KEY AUTOINCREMENT` | ❌ DDL 解析忽略，不填默认值 | **剩余 P0**（与上一条一起做） |
| **表达式 UPDATE** | `UPDATE entries SET access_count=access_count+1 ...` | ✅ 已支持同列 ± 整数 | 已解决 |
| **聚合列别名** | `SELECT COUNT(*) as cnt`、`AVG(decay_score) as avg_decay` | ✅ 已支持（单列 & GROUP BY） | 已解决 |
| **IN 谓词** | `entry_id IN (?,?,...)`（recall scanner） | ✅ 已支持 `IN / NOT IN` 字面量列表 | 已解决 |
| **`COUNT(DISTINCT col)`** | recall scanner 统计 | ✅ 已支持 | 已解决 |
| **`last_insert_rowid()` 作为 SQL** | `SELECT last_insert_rowid()` | ⚠️ 绑定层有 `last_row_id`；SQL 函数仍缺 | 适配层改一行 |
| **`DATE(col)` / GROUP BY 表达式** | decay trend 的 `DATE(timestamp) as day` | ❌ 函数与分组表达式未支持 | P1（趋势接口） |
| **约束语义** | PRIMARY KEY / NOT NULL / FOREIGN KEY | 接受但**不强制**（去重/唯一性由业务代码保证） | 迁移前需确认可接受 |

另外两处不是 `unified_store` 的问题、但迁移时要注意：
- `metadata_db.py` 是**另一套 SQLite 库**（memory_meta / snapshots）；
- `http_handler.py` 用 `PRAGMA table_info(tasks)` / `sqlite_master` 查任务库，`analytics.py` 用 `PRAGMA journal_mode=WAL`——都不在本次兼容范围内。

## 四、规模现实（最关键的风险）

`data/selfmind_hermes.db` 实测：

| 表 | 行数 | 体积 |
|---|---:|---:|
| entries | 606 | 7.0 MB（正文合计 4.8 MB，均 7.8 KB/条） |
| entry_history | 755,453 | 429.2 MB |
| snapshots | 23,249 | 177.7 MB |
| operations_log | 756,081 | 104.1 MB |
| 合计 | **~1.53M 行** | **758 MB** |

ProbeDB 当前存储模型是**全内存 + `persist()` 全量快照**：

- **只接 `entries`（606 条 / 7 MB）**：可行，内存与快照都在 MB 级；
- **把 history/snapshots/operations 一起接**：不可行——每次 persist 要重写几百 MB，内存占用数倍于文本体积；
- 这也意味着"把 `unified_store.py` 整体换成 ProbeDB"不是补 3 个语法就能完成的，需要先决定历史/日志数据的归属。

### 三种迁移路线

| 方案 | 内容 | 风险 | 建议 |
|---|---|:--:|---|
| **A. 只接 entries（推荐）** | entries 由 ProbeDB 承载；history/snapshots/ops 继续 SQLite（或归档） | 低 | 先做，能立刻获得向量/结构化检索 |
| **B. 双写** | SQLite 仍为权威，ProbeDB 做索引副本 | 最低 | 过渡期验证用 |
| **C. 全量替换** | 1.5M 行全部进 ProbeDB | 高 | 需要磁盘存储/索引/事务，属于下一阶段 |

## 五、建议的下一步

1. **剩余 P0**（下一轮）：部分列插入 + DEFAULT/AUTOINCREMENT —— 做完后 `unified_store` 的历史/日志表才可写入。
2. **P1**：`DATE()` 函数 / GROUP BY 表达式（decay trend 接口）、`last_insert_rowid()` SQL 兼容。
3. **写适配层**：`ProbeDBStore` 实现 UnifiedStore 被用到的子集；用真实 606 条 entries 做**双读比对**（SQLite vs ProbeDB）。
4. **确定历史数据归属**（建议保留 SQLite），再谈 entries 的切换。
5. 向量语义搜索作为增益项：需要 embedding 来源，当前 SelfMind 没有，先不做。

> 2026-11-09 更新：NULL / 表达式 UPDATE / 聚合别名 / IN / COUNT(DISTINCT) 已完成；
> 仅剩 DEFAULT/AUTOINCREMENT + 部分列插入 是 P0 阻断。

## 六、复现方式

```bash
cd ~/Documents/aiworkspace/probedb
cargo build --release --lib
python3 tools/selfmind_compat.py          # 验收（PASS + 缺口清单）
python3 tools/selfmind_compat.py --json   # 机器可读
```
