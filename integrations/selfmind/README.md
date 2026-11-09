# SelfMind ProbeDB 后端安装包

把 ProbeDB 接进 SelfMind `UnifiedStore` 的"落地包"：连接兼容层 + 最小补丁 + 验证脚本。

> 当前状态（2026-11-09）：在 SelfMind 仓库的**临时副本**上验证通过——
> 真实 612 条 entries，`get_all_entries` 逐行一致、`get_stats` 一致。
> 源仓库尚未写入（当前 DSH 会话对 `~/Documents/selfmind` 只有读权限）。

## 文件

| 文件 | 说明 |
|---|---|
| `probedb_conn.py` | `sqlite3.Connection` 兼容层（Row / Cursor / 类型回填 / NULL / last_insert_rowid） |
| `install.py` | 安装器：复制兼容层 + 给 `unified_store.py` 注入连接工厂（自动 `.bak` 备份、幂等） |
| `../../tools/selfmind_landing_check.py` | 在 SelfMind 源码**副本**上安装 + 真实数据双后端验证 |
| `../../tools/selfmind_dualread.py` | 不安装，仅双读比对（更轻量） |

## 安装（需要有 SelfMind 写权限）

```bash
cd ~/Documents/aiworkspace/probedb

# 1. 先看补丁内容
python3 integrations/selfmind/install.py --selfmind-root ~/Documents/selfmind --dry-run

# 2. 实际安装
python3 integrations/selfmind/install.py --selfmind-root ~/Documents/selfmind
```

安装器只做两件事：
1. 复制 `selfmind_app/probedb_conn.py`；
2. 在 `selfmind_app/unified_store.py` 注入 `_make_connection()`，替换 `__init__` 与 `switch_db`
   两处 `sqlite3.connect(...)`。业务代码零改动。

## 启用 / 回滚

```bash
# 启用（同一条命令切换后端）
SELFMIND_BACKEND=probedb \
SELFMIND_PROBEDB_PATH=~/Documents/probedb_data/selfmind.pdb \
python3 server.py

# 默认不设环境变量 = 原 SQLite，行为不变

# 回滚
cp selfmind_app/unified_store.py.bak selfmind_app/unified_store.py
rm selfmind_app/probedb_conn.py
```

## 迁移 entries 数据

安装后先用现有工具把 SQLite entries 导入 ProbeDB（当前用仓库内脚本即可）：

```bash
python3 tools/selfmind_dualread.py --sqlite-db ~/Documents/selfmind/data/selfmind_hermes.db
```

（后续可在 SelfMind 仓库补 `scripts/migrate_entries_to_probedb.py`。）

## 验证结果

`python3 tools/selfmind_landing_check.py`：

```
✅ 安装脚本执行
✅ 连接工厂已注入
✅ probedb_conn.py 已复制
✅ entries 导入 ProbeDB — 612 条
✅ UnifiedStore 以 probedb 后端启动
✅ get_all_entries 逐行一致 — 612 vs 612 rows, diffs=0
✅ get_stats 一致 — diffs=[]
结果: PASS
```

## 已知边界

- 只迁移 `entries` 表；`entry_history`（429MB）/ `snapshots`（178MB）/ `operations_log`（104MB）继续留 SQLite。
- `DATE()` / `GROUP BY DATE()` 趋势接口、事务 `BEGIN/COMMIT` 尚未支持（P1）。
- ProbeDB 行值协议是字符串：兼容层按 SCHEMA_SQL 回填数字类型；正文恰好等于 `"NULL"` 的文本会被当成 NULL（待 typed row 协议修复）。
- `switch_db()` 支持切换走工厂，但每 Agent 独立库的完整流程尚未在真实多 Agent 场景验证。
