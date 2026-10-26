# SelfMind 接入需求（苏格拉底 → DSH）

> 来源：苏格拉底（Hermes Agent），2026-10-26
> 背景：SelfMind 是智能体记忆系统，当前用 SQLite 存储。
> 坦哥要求改用 ProbeDB。以下是 SelfMind 侧实测的 gap。

---

## SelfMind 当前存储依赖

- 数据库：SQLite，`data/selfmind_hermes.db`（768MB，606条 entries）
- 核心文件：`selfmind_app/unified_store.py`（全量操作入口）
- 依赖的 SQLite 特性：`sqlite3.Row`（按列名访问）、`?` 参数化、`executescript`、`commit()`、`check_same_thread=False`、`last_insert_rowid()`、`IntegrityError`

## 阻断项（必须补，否则无法迁移）

### 1. CREATE INDEX 支持
SelfMind 建了 14 个索引：
```sql
CREATE INDEX IF NOT EXISTS idx_entries_type ON entries(type);
CREATE INDEX IF NOT EXISTS idx_entries_status ON entries(status);
CREATE INDEX IF NOT EXISTS idx_entries_primary ON entries(primary_cat);
CREATE INDEX IF NOT EXISTS idx_entries_source ON entries(source);
CREATE INDEX IF NOT EXISTS idx_entries_content_hash ON entries(content_hash);
CREATE INDEX IF NOT EXISTS idx_entries_observer ON entries(observer);
CREATE INDEX IF NOT EXISTS idx_entries_honcho_level ON entries(honcho_level);
CREATE INDEX IF NOT EXISTS idx_history_entry ON entry_history(entry_id);
CREATE INDEX IF NOT EXISTS idx_history_timestamp ON entry_history(timestamp);
CREATE INDEX IF NOT EXISTS idx_decay_hist_entry ON decay_history(entry_id);
CREATE INDEX IF NOT EXISTS idx_decay_hist_timestamp ON decay_history(timestamp);
CREATE INDEX IF NOT EXISTS idx_recall_entry ON agent_recall_log(entry_id);
CREATE INDEX IF NOT EXISTS idx_recall_agent ON agent_recall_log(agent_id);
CREATE INDEX IF NOT EXISTS idx_recall_timestamp ON agent_recall_log(timestamp);
```

当前 ProbeDB 报错：`不支持的SQL语句: CREATE INDEX`

### 2. 表别名（Table Alias）
SelfMind 的 JOIN 查询用别名：
```sql
SELECT e.id FROM entries e INNER JOIN decay_history dh ON dh.entry_id = e.id
```

当前 ProbeDB 报错：`表 'decay_history dh' 不存在`（把别名当作表名的一部分解析了）

### 3. 参数化查询（? 占位符）
SelfMind 有 32 处 `WHERE id = ?` 参数化查询，例如：
```python
self.conn.execute("SELECT * FROM entries WHERE id=?", (entry_id,))
```

ProbeDB 的 `execute()` 只接收完整 SQL 字符串，不支持参数绑定。

## 增益项（补了直接提升 SelfMind 能力）

### 4. VECTOR 原生类型 → 语义搜索
ProbeDB 已支持 `VECTOR(n)` + `vector_similarity()`。SelfMind 当前的内容检索是关键词匹配（`should_know.py` 的 `_keyword_search`），迁移后可直接做向量语义搜索。

---

## SelfMind entries 表结构（迁移目标）

```sql
CREATE TABLE entries (
    id TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    content TEXT NOT NULL,
    content_preview TEXT,
    type TEXT NOT NULL DEFAULT 'memory',
    source TEXT NOT NULL,
    source_profile TEXT DEFAULT 'hermes',
    primary_cat TEXT,
    secondary_cat TEXT,
    label TEXT,
    tags TEXT DEFAULT '[]',
    observer TEXT,
    observed TEXT,
    honcho_level TEXT,
    honcho_doc_id TEXT,
    importance REAL DEFAULT 0.5,
    decay_score REAL DEFAULT 0.25,
    access_count INTEGER DEFAULT 0,
    version INTEGER DEFAULT 1,
    first_seen_at TEXT,
    created_at TEXT,
    updated_at TEXT,
    last_accessed TEXT,
    last_recalled TEXT,
    last_synced_at TEXT,
    status TEXT DEFAULT 'active',
    pinned INTEGER DEFAULT 0
);
```

附加表：`entry_history`、`decay_history`、`agent_recall_log`、`operations_log`、`snapshots`

## 验收标准

1. `CREATE INDEX IF NOT EXISTS idx ON t(col)` 不报错
2. `SELECT e.id FROM entries e JOIN meta m ON e.id = m.eid` 能跑
3. `db.execute("SELECT * FROM t WHERE id = ?", params=("x",))` 或等价 API 可用
4. 上述 4 项补完后，用 SelfMind 的 `unified_store.py` schema 初始化能通过

## 关联文档
- SelfMind 研发计划：`aiknowledge/selfmind/REQUIREMENT.md` Phase 5
- SelfMind 项目追踪：`aiknowledge/selfmind/selfmind-development.md`
- ProbeDB Hermes 接口定义：`aiknowledge/数据库/AI原生数据库/ProbeDB Hermes接口定义.md`