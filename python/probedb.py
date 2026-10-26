"""ProbeDB Python binding — ctypes over libprobedb (C ABI, 零外部依赖).

这是唯一的 Python 绑定入口（原 bridge/probedb.py 已删除，避免两份实现漂移）。

用法::

    from probedb import ProbeDB

    with ProbeDB.open("/tmp/memory.pdb") as db:
        db.execute("CREATE TABLE t (id INTEGER, name TEXT)")
        db.execute("INSERT INTO t (name, id) VALUES ('alice', 1)")  # 列顺序可任意
        result = db.execute("SELECT id, name FROM t")
        print(result.rows)          # [['1', 'alice']]
        print(result)               # 人类可读表格
        db.persist()

约定：
- 所有结果统一走 `probedb_execute_json`，返回结构化 `Result`，不再解析格式化文本。
- 行值当前是字符串（Rust 执行器投影层已格式化；VECTOR 形如 ``[1,2,3]``）。
- 同一个句柄的调用用进程内 Lock 串行化。ctypes 调 C 时会释放 GIL，
  无锁时多线程共用句柄会触发 Rust 侧未定义行为。
"""

from __future__ import annotations

import ctypes
import json
import os
import threading
from pathlib import Path
from typing import Any, Optional, Sequence

def sql_literal(value: str) -> str:
    """把字符串安全转成 SQL 字面量（SQL 标准 '' 转义）。"""
    return "'" + value.replace("'", "''") + "'"


def _param_literal(value: Any) -> str:
    """把 Python 参数转成 SQL 字面量。

    - None：转成 SQL `NULL`
    - bool / int / float：直接输出（bool → 1/0，与 SQLite 一致）
    - str：'' 转义
    """
    if value is None:
        return "NULL"
    if isinstance(value, bool):
        return "1" if value else "0"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        return repr(value)
    if isinstance(value, (bytes, bytearray)):
        raise ProbeDBError("ProbeDB 暂不支持 bytes 参数")
    return sql_literal(str(value))


def bind_params(sql: str, params: Optional[Sequence[Any]]) -> str:
    """把 `?` 位置参数安全替换进 SQL（跳过单引号字符串、'' 转义与 -- 注释）。"""
    if params is None:
        return sql
    params = list(params)
    out: list[str] = []
    used = 0
    i = 0
    n = len(sql)
    in_quote = False
    while i < n:
        c = sql[i]
        if in_quote:
            out.append(c)
            if c == "'":
                if i + 1 < n and sql[i + 1] == "'":
                    out.append("'")
                    i += 2
                    continue
                in_quote = False
            i += 1
            continue
        if c == "'":
            in_quote = True
            out.append(c)
            i += 1
            continue
        if c == "-" and i + 1 < n and sql[i + 1] == "-":
            # -- 行注释原样保留（Rust 解析层会丢弃）
            end = sql.find("\n", i)
            if end == -1:
                out.append(sql[i:])
                i = n
            else:
                out.append(sql[i : end + 1])
                i = end + 1
            continue
        if c == "?":
            if used >= len(params):
                raise ProbeDBError(
                    f"参数数量不匹配：SQL 占位符多于提供的参数（已提供 {len(params)} 个）"
                )
            out.append(_param_literal(params[used]))
            used += 1
            i += 1
            continue
        out.append(c)
        i += 1
    if used != len(params):
        raise ProbeDBError(
            f"参数数量不匹配：SQL 占位符 {used} 个，提供参数 {len(params)} 个"
        )
    return "".join(out)


class ProbeDBError(Exception):
    """ProbeDB 操作错误（对应 C 层 NULL 返回 + probedb_last_error）。"""


class Result:
    """一次 SQL 执行的结构化结果。

    对应 JSON: ``{"type": "select", "columns": [...], "rows": [[...]]}``
    """

    def __init__(self, payload: dict[str, Any]):
        self._payload = payload

    @property
    def type(self) -> str:
        return self._payload.get("type", "unknown")

    @property
    def columns(self) -> list[str]:
        return list(self._payload.get("columns", []))

    @property
    def rows(self) -> list[list[str]]:
        return [list(row) for row in self._payload.get("rows", [])]

    @property
    def records(self) -> list[dict[str, str]]:
        """按列名返回行（等价于 sqlite3.Row 的 dict 形式，便于迁移适配）。"""
        return [dict(zip(self.columns, row)) for row in self.rows]

    @property
    def row_id(self) -> Optional[int]:
        return self._payload.get("row_id")

    @property
    def count(self) -> Optional[int]:
        return self._payload.get("count")

    @property
    def name(self) -> Optional[str]:
        return self._payload.get("name")

    def __len__(self) -> int:
        return len(self._payload.get("rows", []))

    def __str__(self) -> str:
        t = self.type
        if t == "select":
            header = " | ".join(self.columns)
            sep = " | ".join("---" for _ in self.columns)
            lines = [f"查询结果 ({len(self.rows)} 行):", header, sep]
            lines += [" | ".join(row) for row in self.rows]
            return "\n".join(lines)
        if t == "table_created":
            return f"表 '{self.name}' 创建成功"
        if t == "table_dropped":
            return f"表 '{self.name}' 已删除"
        if t == "inserted":
            return f"插入成功，行ID: {self.row_id}"
        if t == "deleted":
            return f"删除成功，共 {self.count} 行"
        if t == "updated":
            return f"更新成功，共 {self.count} 行"
        if t == "message":
            return str(self._payload.get("text", ""))
        return str(self._payload)

    def __repr__(self) -> str:
        return f"Result(type={self.type!r}, payload={self._payload!r})"


def _library_candidates() -> list[Path]:
    """按优先级返回候选动态库路径。"""
    repo_root = Path(__file__).resolve().parent.parent
    names = ["libprobedb.dylib", "libprobedb.so", "probedb.dll"]
    candidates: list[Path] = []

    env = os.environ.get("PROBEDB_LIB")
    if env:
        candidates.append(Path(env))

    for profile in ("release", "debug"):
        for name in names:
            candidates.append(repo_root / "target" / profile / name)

    for name in names:
        candidates.append(Path(__file__).resolve().parent / name)

    return candidates


class ProbeDB:
    """嵌入式 ProbeDB 句柄。"""

    _lib: Optional[ctypes.CDLL] = None
    _lib_path: Optional[str] = None

    def __init__(self, handle: int):
        self._handle = handle
        self._lock = threading.Lock()
        self._last_row_id: Optional[int] = None

    # ---------- 生命周期 ----------

    @classmethod
    def open(cls, path: Optional[str] = None) -> "ProbeDB":
        """打开（或创建）数据库。path=None 为纯内存模式。"""
        lib = cls._load_lib()
        c_path = path.encode("utf-8") if path is not None else None
        handle = lib.probedb_open(c_path)
        if not handle:
            raise ProbeDBError(cls._last_error())
        return cls(handle)

    def close(self) -> None:
        """关闭数据库并释放句柄（可重复调用）。"""
        handle = getattr(self, "_handle", 0)
        if handle:
            self._handle = 0
            self._lib.probedb_close(handle)

    def __enter__(self) -> "ProbeDB":
        return self

    def __exit__(self, *args) -> bool:
        self.close()
        return False

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass  # 解释器退出时动态库可能已卸载

    # ---------- 执行 ----------

    def execute(self, sql: str, params: Optional[Sequence[Any]] = None) -> Result:
        """执行单条 SQL，返回 Result；失败抛 ProbeDBError。

        params 提供时支持 SQLite 风格的 `?` 位置参数（在客户端安全拼成 SQL 字面量，
        因为 ProbeDB 的 C ABI 目前没有独立的参数绑定接口）。

        注意：`None` 会转成 SQL NULL；查询结果里的 NULL 目前以字符串 "NULL" 返回
        （行值协议尚未类型化）。
        """
        bound = bind_params(sql, params)
        results = self._execute_json(bound)
        if not results:
            return Result({"type": "message", "text": ""})
        if len(results) > 1:
            raise ValueError(
                f"SQL 包含 {len(results)} 条语句，请使用 execute_all() 获取全部结果"
            )
        result = Result(results[0])
        if result.type == "inserted" and result.row_id is not None:
            self._last_row_id = result.row_id
        return result

    def execute_all(self, sql: str, params: Optional[Sequence[Any]] = None) -> list[Result]:
        """执行一条或多条 SQL，返回全部 Result。"""
        bound = bind_params(sql, params)
        results = [Result(payload) for payload in self._execute_json(bound)]
        for result in results:
            if result.type == "inserted" and result.row_id is not None:
                self._last_row_id = result.row_id
        return results

    def executescript(self, script: str) -> list[Result]:
        """执行多语句脚本（对应 sqlite3 的 executescript，支持 -- 注释）。"""
        return self.execute_all(script)

    def commit(self) -> None:
        """兼容 sqlite3 的 commit()。ProbeDB 每条写入即时生效，这里是 no-op。"""

    @property
    def last_row_id(self) -> Optional[int]:
        """最近一次 INSERT 的行 ID（对应 sqlite3 的 lastrowid）。"""
        return self._last_row_id

    def _execute_json(self, sql: str) -> list[dict[str, Any]]:
        ptr = self._call(self._lib.probedb_execute_json, sql.encode("utf-8"))
        if not ptr:
            raise ProbeDBError(self._last_error())
        try:
            raw = ctypes.string_at(ptr).decode("utf-8")
        finally:
            self._lib.probedb_free_string(ptr)
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError as e:
            raise ProbeDBError(f"FFI 返回了非法 JSON: {raw!r} ({e})") from e
        return payload.get("results", [])

    # ---------- 持久化 ----------

    def persist(self) -> None:
        """原子落盘（先 flush WAL 再写快照）。"""
        ret = self._call(self._lib.probedb_persist)
        if ret != 0:
            raise ProbeDBError(self._last_error())

    def set_batch_mode(self, enabled: bool) -> None:
        """开启/关闭 WAL 批量模式（性能 vs 持久性权衡）。"""
        ret = self._call(self._lib.probedb_set_batch_mode, 1 if enabled else 0)
        if ret != 0:
            raise ProbeDBError(self._last_error())

    # ---------- 内部 ----------

    def _call(self, fn, *args):
        """串行化所有 FFI 调用（ctypes 调 C 会释放 GIL）。"""
        with self._lock:
            return fn(self._handle, *args)

    @classmethod
    def _load_lib(cls) -> ctypes.CDLL:
        if cls._lib is not None:
            return cls._lib

        candidates = _library_candidates()
        lib_path = next((p for p in candidates if p.exists()), None)
        if lib_path is None:
            raise ProbeDBError(
                "找不到 libprobedb 动态库。请先运行: cargo build --release --lib\n"
                f"已查找: {[str(p) for p in candidates]}"
            )

        lib = ctypes.CDLL(str(lib_path))
        lib.probedb_open.argtypes = [ctypes.c_char_p]
        lib.probedb_open.restype = ctypes.c_void_p

        lib.probedb_execute.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
        lib.probedb_execute.restype = ctypes.c_void_p

        lib.probedb_execute_json.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
        lib.probedb_execute_json.restype = ctypes.c_void_p

        lib.probedb_persist.argtypes = [ctypes.c_void_p]
        lib.probedb_persist.restype = ctypes.c_int32

        lib.probedb_set_batch_mode.argtypes = [ctypes.c_void_p, ctypes.c_int32]
        lib.probedb_set_batch_mode.restype = ctypes.c_int32

        lib.probedb_close.argtypes = [ctypes.c_void_p]
        lib.probedb_close.restype = None

        lib.probedb_last_error.argtypes = []
        lib.probedb_last_error.restype = ctypes.c_void_p

        lib.probedb_free_string.argtypes = [ctypes.c_void_p]
        lib.probedb_free_string.restype = None

        cls._lib = lib
        cls._lib_path = str(lib_path)
        return lib

    @classmethod
    def _last_error(cls) -> str:
        """取 C 层最后一条错误（Rust 分配，这里负责释放）。"""
        lib = cls._load_lib()
        ptr = lib.probedb_last_error()
        if not ptr:
            return "unknown error"
        try:
            return ctypes.cast(ptr, ctypes.c_char_p).value.decode("utf-8")
        finally:
            lib.probedb_free_string(ptr)


def _smoke() -> None:
    """快速自检：CRUD / 列名重排 / JSON 结果 / 持久化往返。"""
    import tempfile

    db = ProbeDB.open()
    db.execute("CREATE TABLE test (id INTEGER, name TEXT)")
    db.execute("INSERT INTO test (id, name) VALUES (1, 'hello')")
    # 列清单顺序与表结构不同 → 必须按列名映射
    db.execute("INSERT INTO test (name, id) VALUES ('world', 2)")

    r = db.execute("SELECT id, name FROM test ORDER BY id ASC")
    assert r.type == "select"
    assert r.columns == ["id", "name"]
    assert r.rows == [["1", "hello"], ["2", "world"]], r.rows

    db.execute("CREATE TABLE vec (id INTEGER, emb VECTOR(3))")
    db.execute("INSERT INTO vec (id, emb) VALUES (1, '[1.0,0.0,0.0]')")
    db.execute("INSERT INTO vec (id, emb) VALUES (2, '[0.0,1.0,0.0]')")
    r = db.execute(
        "SELECT id FROM vec WHERE vector_similarity(emb, '[1.0,0.0,0.0]') > 0.9"
    )
    assert r.rows == [["1"]], r.rows
    db.close()

    path = os.path.join(tempfile.gettempdir(), "probedb_py_smoke.pdb")
    try:
        with ProbeDB.open(path) as db2:
            db2.execute("CREATE TABLE kv (key TEXT, value TEXT)")
            db2.execute("INSERT INTO kv (key, value) VALUES ('x', 'y')")
            db2.persist()
        with ProbeDB.open(path) as db3:
            assert db3.execute("SELECT value FROM kv WHERE key = 'x'").rows == [["y"]]
    finally:
        for p in (path, path + ".wal", path + ".tmp"):
            try:
                os.remove(p)
            except OSError:
                pass

    print("✅ Smoke tests passed")


if __name__ == "__main__":
    _smoke()
