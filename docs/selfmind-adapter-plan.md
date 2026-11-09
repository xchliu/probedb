# SelfMind ProbeDB 适配层方案（ProbeDBStore / 连接兼容层）

> 2026-11-09 · 对应 SelfMind Phase 5.5 / `docs/dsh-tasks.md` 任务二
> 状态：方案就绪，等待 SelfMind 仓库写权限后落地

## 一、目标

让 SelfMind 的 `selfmind_app/unified_store.py` 在不重写业务代码的前提下从 SQLite 切到 ProbeDB，
并保留可回滚路径。**第一版只接 `entries` 表（606 条 / 7 MB）；history/snapshots/operations 继续留在 SQLite。**

## 二、SelfMind 现有耦合点

`UnifiedStore` 直接持有 `sqlite3.Connection`，代码里用到：

| 能力 | 使用方式 | ProbeDB 现状 |
|---|---|---|
| `execute(sql, params)` | 全部 CRUD | ✅ Python 绑定已支持 `?` 参数 |
| 结果行按列名访问 | `row["id"]` / `dict(row)` | ⚠️ 需 shim 提供 `Row`；ProbeDB 返回 `records` |
| `executescript` | schema 初始化 | ✅ |
| `commit()` | 频繁提交 | ✅ no-op（写入即时生效） |
| `last_insert_rowid()` | snapshots 插入后取 id | ❌ SQL 函数未支持（shim 可拦截） |
| `check_same_thread=False` | 服务单线程使用 | ✅ 绑定内部加锁 |
| `DATE()` / `GROUP BY DATE()` | decay 趋势 | ❌ 未支持 |
| `IS NULL` / `IN` / 别名 / 聚合 | stats / recall | ✅ |
| 事务 | 没有显式 BEGIN | ✅ 无影响 |

结论：**用连接兼容层（shim）比分叉改造 UnifiedStore 成本低得多。**

## 三、推荐架构：`ProbeDBConnection` 兼容层

新增 `selfmind_app/probedb_conn.py`，暴露与 `sqlite3.Connection` 相同的最小接口；
把 `UnifiedStore.__init__` 中的 `sqlite3.connect(...)` 换成工厂函数即可（`backend="probedb"` 时）。

```python
class ProbeDBCursor:
    def __init__(self, result, lastrowid=0):
        self._rows = [Row(r) for r in result.records]  # 按列名
        self.lastrowid = lastrowid
        self.rowcount = result.count or len(self._rows)
    def fetchone(self): return self._rows[0] if self._rows else None
    def fetchall(self): return list(self._rows)
    def __iter__(self): return iter(self._rows)

class Row(dict):
    """sqlite3.Row 的最小兼容：row["col"] / row[0] / dict(row)"""
    def __init__(self, mapping):
        super().__init__(mapping)
        self._order = list(mapping.keys())
    def __getitem__(self, key):
        if isinstance(key, int):
            return dict.__getitem__(self, self._order[key])
        return dict.__getitem__(self, key)

class ProbeDBConnection:
    def __init__(self, path, check_same_thread=False):
        self._db = ProbeDB.open(path)
        self._lastrowid = 0
        self._closed = False

    def execute(self, sql, params=()):
        if sql.strip().lower() == "select last_insert_rowid()":
            return ProbeDBCursor(_FakeSelect([[str(self._lastrowid)]], ["last_insert_rowid()"]))
        result = self._db.execute(sql, params)
        if result.type == "inserted":
            self._lastrowid = result.row_id or 0
        return ProbeDBCursor(result, self._lastrowid)

    def executescript(self, script):
        self._db.executescript(script)

    def commit(self):
        self._db.persist()      # 保持"提交后落盘"的语义

    def close(self):
        self._db.close()

    @property
    def lastrowid(self):
        return self._lastrowid
```

关键语义决定：

1. **`commit()` → `persist()`**：SQLite 的 commit 是持久化边界；ProbeDB 每次写都进 WAL，commit 做快照，最接近原语义。
2. **`last_insert_rowid()` 拦截**：只精确匹配这一条 SQL；其他函数不假装支持。
3. **NULL 映射**：ProbeDB 行值协议尚未类型化，NULL 以 `"NULL"` 返回。shim 暂时保留字符串，
   由业务层按已知可空列转换成 `None`；下一步升级 JSON 行值为 typed null 后移除该特判。
4. **不实现 `rollback`/嵌套事务**：SelfMind 当前没有显式事务；若未来需要，再补。

## 四、迁移分阶段

| 阶段 | 内容 | 风险 | 回滚 |
|---|---|---|---|
| **0. 镜像 + 双读** | 把 entries 606 条导入 ProbeDB；两个后端同时读，逐条比对 | 极低 | 不切流量 |
| **1. 适配层单测** | `probedb_conn.py` + `UnifiedStore` 跑单元测试（entries CRUD / stats / recall 读） | 低 | 保留 sqlite 分支 |
| **2. 影子写** | 生产仍以 SQLite 为准；entries 写入后同步 ProbeDB，失败只记日志 | 中 | 关开关 |
| **3. 切换 entries 读** | entries 读走 ProbeDB；history/snapshots/ops 仍在 SQLite | 中 | 切回 sqlite |
| **4. 评估全量** | 只有磁盘存储/索引/事务完成后才考虑 history 等大表 | 高 | 不做 |

## 五、落地清单（需要 SelfMind 写权限）

1. `selfmind_app/probedb_conn.py`：连接/游标/Row 兼容层。
2. `selfmind_app/unified_store.py`：`__init__(db_path, backend="sqlite")`，按 backend 选择连接。
3. `scripts/migrate_entries_to_probedb.py`：SQLite → ProbeDB 导入 + 条数/内容 hash 校验。
4. `scripts/dual_read_check.py`：双读比对（id/content_hash/content/status/type）。
5. `tests/unit/test_probedb_backend.py`：适配层单测；`pytest` 全绿纳入验收。
6. 文档：README 增加 `SELFMIND_BACKEND=probedb` 开关；回滚说明。

## 六、已知风险 / 前置项

- **P1 SQL 缺口**：`DATE()` / `GROUP BY DATE()`（decay 趋势接口）、`last_insert_rowid()`（由 shim 拦截）、事务。
- **NULL 字符串歧义**：内容本身等于 `"NULL"` 的文本会被误判；需 typed row 协议（后续）。
- **数据规模**：entries 7 MB 可行；history 429 MB / snapshots 178 MB / operations 104 MB 不进 ProbeDB。
- **单写者**：服务进程内单连接；若多进程同时写同一 .pdb 需外部锁。
- **回滚**：SQLite 原库保持只读备份，切换期不删除。

## 七、验收（对应 `docs/dsh-tasks.md` 验收标准）

1. `ProbeDBConnection` 可让 `UnifiedStore` 通过现有单测；
2. entries 双读比对 100% 一致（606 条）；
3. `cargo test`（ProbeDB）与 `pytest`（SelfMind）全绿；
4. 关闭 backend 开关可即时回滚 SQLite。
