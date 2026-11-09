"""ProbeDBConnection — sqlite3.Connection 兼容层（SelfMind 适配用）。

用途：把 `selfmind_app/unified_store.py` 里的 `sqlite3.connect(...)` 换成
`ProbeDBConnection(...)`，业务代码无需重写。当前覆盖 UnifiedStore 用到的接口：

- execute(sql, params) / executescript / commit / rollback / close / lastrowid
- 结果行同时支持 `row["col"]`、`row[0]`、`dict(row)`
- `SELECT last_insert_rowid()` 拦截
- 依据 SCHEMA_SQL 做基础类型回填（INTEGER/FLOAT/BOOLEAN），
  以及 COUNT/AVG 聚合列的类型回填（ProbeDB 行值协议目前是字符串）

已知边界（迁移前必须知道）：
- NULL 以 `"NULL"` 字符串返回 → 这里转成 `None`；若正文本身恰好是 "NULL" 会被误判，
  需等 ProbeDB 行值协议类型化后移除该特例。
- 不实现事务语义：commit = persist（落盘），rollback 为 no-op。
- 只做单表列类型回填；JOIN/表达式列不做猜测。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from typing import Any, Optional, Sequence

_REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(_REPO_ROOT / "python"))

from probedb import ProbeDB, ProbeDBError  # noqa: E402


class Row(dict):
    """sqlite3.Row 的最小兼容：支持字符串键、整数下标和 dict(row)。"""

    def __init__(self, mapping: dict[str, Any]):
        super().__init__(mapping)
        self._order = list(mapping.keys())

    def __getitem__(self, key):
        if isinstance(key, int):
            return dict.__getitem__(self, self._order[key])
        return dict.__getitem__(self, key)


class ProbeDBCursor:
    """sqlite3.Cursor 的最小兼容。"""

    def __init__(self, rows: list[Row], columns: list[str], lastrowid: int = 0):
        self._rows = rows
        self.columns = columns
        self.lastrowid = lastrowid
        self.rowcount = len(rows)
        self.description = [(c,) for c in columns]

    def fetchone(self) -> Optional[Row]:
        return self._rows[0] if self._rows else None

    def fetchall(self) -> list[Row]:
        return list(self._rows)

    def __iter__(self):
        return iter(self._rows)

    def close(self) -> None:
        pass


def _parse_schema_types(script: str) -> dict[str, dict[str, str]]:
    """从 SCHEMA_SQL 提取 {table: {column: 归一化类型}}，用于结果类型回填。"""
    script = re.sub(r"--[^\n]*", "", script)
    types: dict[str, dict[str, str]] = {}
    for match in re.finditer(
        r"CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?([A-Za-z_]\w*)\s*\((.*?)\)\s*;",
        script,
        re.IGNORECASE | re.DOTALL,
    ):
        table = match.group(1).lower()
        cols: dict[str, str] = {}
        for raw in match.group(2).split(","):
            tokens = raw.strip().split()
            if len(tokens) < 2:
                continue
            if tokens[0].upper() in {"FOREIGN", "PRIMARY", "UNIQUE", "CHECK", "CONSTRAINT"}:
                continue
            col = tokens[0].lower().strip('"`')
            type_token = tokens[1].upper()
            if type_token.startswith("INT"):
                cols[col] = "int"
            elif type_token.startswith(("REAL", "FLOAT", "DOUBLE")):
                cols[col] = "float"
            elif type_token.startswith("BOOL"):
                cols[col] = "bool"
            else:
                cols[col] = "text"
        types[table] = cols
    return types


_COUNT_ALIASES = {"cnt", "count", "recall_count", "sample_count"}
_FLOAT_ALIASES = {"avg_decay", "avg", "avg_score"}


class ProbeDBConnection:
    """sqlite3.Connection 兼容层（ProbeDB 后端）。"""

    def __init__(self, path: str, check_same_thread: bool = False, **kwargs):
        self._db = ProbeDB.open(str(path))
        self._lastrowid = 0
        self._types: dict[str, dict[str, str]] = {}
        self.row_factory = None  # UnifiedStore 会赋值 sqlite3.Row，这里忽略

    # ---------- SQL 执行 ----------

    def execute(self, sql: str, params: Optional[Sequence[Any]] = ()) -> ProbeDBCursor:
        stripped = sql.strip()
        if stripped.lower().startswith("select last_insert_rowid()"):
            return ProbeDBCursor(
                [Row({"last_insert_rowid()": self._lastrowid})],
                ["last_insert_rowid()"],
                self._lastrowid,
            )

        if params is None:
            params = ()
        result = self._db.execute(stripped, tuple(params) if params else None)
        if result.type == "inserted" and result.row_id is not None:
            self._lastrowid = result.row_id
        if result.type != "select":
            return ProbeDBCursor([], [], self._lastrowid)

        columns = result.columns
        table = _first_table_in_sql(stripped)
        sql_upper = stripped.upper()
        rows: list[Row] = []
        for raw_values in result.rows:
            mapping = {}
            for col, value in zip(columns, raw_values):
                mapping[col] = self._convert(col, value, table, sql_upper)
            rows.append(Row(mapping))
        return ProbeDBCursor(rows, columns, self._lastrowid)

    def executescript(self, script: str) -> None:
        self._db.executescript(script)
        self._types = _parse_schema_types(script)

    def executemany(self, sql: str, seq_of_params) -> None:
        for params in seq_of_params:
            self.execute(sql, params)

    # ---------- 事务 / 生命周期 ----------

    def commit(self) -> None:
        # SQLite 的 commit 是持久化边界；ProbeDB 里对应 persist()
        self._db.persist()

    def rollback(self) -> None:
        # ProbeDB 当前无事务回滚；no-op 并保留接口
        pass

    def close(self) -> None:
        self._db.close()

    @property
    def lastrowid(self) -> int:
        return self._lastrowid

    # ---------- 内部 ----------

    def _convert(self, column: str, value: str, table: Optional[str], sql_upper: str) -> Any:
        if value == "NULL":
            return None
        if "." in column:  # JOIN 列名带 table. 前缀时不猜类型
            return value

        col = column.lower()
        col_type = None
        if table and table in self._types:
            col_type = self._types[table].get(col)

        if col_type == "int" or col_type == "bool":
            return int(value)
        if col_type == "float":
            return float(value)

        # 聚合列（无 schema 类型可依）：COUNT → int，AVG → float
        if col in _COUNT_ALIASES or ("COUNT(" in sql_upper and col.endswith("count")):
            try:
                return int(value)
            except ValueError:
                return value
        if col in _FLOAT_ALIASES or ("AVG(" in sql_upper and col.startswith("avg")):
            try:
                return float(value)
            except ValueError:
                return value
        return value


def _first_table_in_sql(sql: str) -> Optional[str]:
    match = re.search(r"\bFROM\s+([A-Za-z_]\w*)", sql, re.IGNORECASE)
    return match.group(1).lower() if match else None
