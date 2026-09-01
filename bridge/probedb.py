#!/usr/bin/env python3
"""ProbeDB Python 桥接层（Hermes 接入原型）

通过 ctypes 调用 libprobedb.dylib（C ABI，零外部依赖）。
这是 Hermes 接入的第一版接口层：Python 进程内直接调用 ProbeDB，
类似 sqlite3 模块的用法。

用法:
    python3 bridge/probedb.py            # 运行自测
    python3 bridge/probedb.py --demo     # 跑完整演示（含持久化+向量）

设计:
    - probedb_open(path)    打开/创建库（path=None 纯内存）
    - db.execute(sql)       执行 SQL，返回格式化文本；错误抛 ProbeDBError
    - db.persist()          原子落盘
    - db.close()            关闭
    - 所有 Rust 侧字符串由 probedb_free_string 释放（ctypes 自动处理）
"""

import ctypes
import os
import sys
from ctypes import c_char_p, c_int, c_void_p

_LIB_NAME = "libprobedb.dylib"
_LIB_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target", "release", _LIB_NAME)


class ProbeDBError(Exception):
    """ProbeDB 执行错误（对应 C 层 NULL 返回 + last_error）"""


class ProbeDB:
    """嵌入式 ProbeDB 句柄（Python 侧封装，API 风格仿 sqlite3）"""

    def __init__(self, path=None):
        self._lib = _load_lib()
        c_path = c_char_p(path.encode("utf-8")) if path else None
        self._handle = self._lib.probedb_open(c_path)
        if not self._handle:
            raise ProbeDBError(self._take_error() or "打开数据库失败")

    def execute(self, sql: str) -> str:
        """执行 SQL，返回格式化文本结果"""
        c_sql = c_char_p(sql.encode("utf-8"))
        result_ptr = self._lib.probedb_execute(self._handle, c_sql)
        if not result_ptr:
            raise ProbeDBError(self._take_error() or "执行失败")
        try:
            return ctypes.string_at(result_ptr).decode("utf-8")
        finally:
            self._lib.probedb_free_string(result_ptr)

    def persist(self) -> None:
        if self._lib.probedb_persist(self._handle) != 0:
            raise ProbeDBError(self._take_error() or "持久化失败")

    def set_batch_mode(self, enabled: bool) -> None:
        if self._lib.probedb_set_batch_mode(self._handle, 1 if enabled else 0) != 0:
            raise ProbeDBError(self._take_error() or "设置批量模式失败")

    def close(self) -> None:
        if self._handle:
            self._lib.probedb_close(self._handle)
            self._handle = None

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def _take_error(self) -> str:
        err_ptr = self._lib.probedb_last_error()
        if not err_ptr:
            return ""
        try:
            return ctypes.string_at(err_ptr).decode("utf-8")
        finally:
            self._lib.probedb_free_string(err_ptr)

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass


def _load_lib():
    if not os.path.exists(_LIB_PATH):
        raise RuntimeError(
            f"找不到 {_LIB_NAME}，请先构建: cd probedb && cargo build --release --offline"
        )
    lib = ctypes.CDLL(_LIB_PATH)
    lib.probedb_open.restype = c_void_p
    lib.probedb_open.argtypes = [c_char_p]
    # 注意: execute/last_error 返回 Rust 侧分配的原生指针（必须由
    # probedb_free_string 释放）。restype 不能用 c_char_p（ctypes 会转成
    # bytes 拷贝并丢失原指针，导致 free 错误指针 abort）。
    lib.probedb_execute.restype = c_void_p
    lib.probedb_execute.argtypes = [c_void_p, c_char_p]
    lib.probedb_persist.restype = c_int
    lib.probedb_persist.argtypes = [c_void_p]
    lib.probedb_set_batch_mode.restype = c_int
    lib.probedb_set_batch_mode.argtypes = [c_void_p, c_int]
    lib.probedb_last_error.restype = c_void_p
    lib.probedb_last_error.argtypes = []
    lib.probedb_free_string.restype = None
    lib.probedb_free_string.argtypes = [c_void_p]
    lib.probedb_close.restype = None
    lib.probedb_close.argtypes = [c_void_p]
    return lib


def _demo():
    """完整演示：CRUD + 向量混合查询 + 持久化重开"""
    print("=== ProbeDB Python 桥接层演示 ===")
    with ProbeDB() as db:
        print(db.execute("CREATE TABLE memories (id INTEGER, content TEXT, tag TEXT)"))
        print(db.execute(
            "INSERT INTO memories (id, content, tag) VALUES "
            "(1, '上周跟坦哥讨论ProbeDB定位', 'project'), "
            "(2, '秋秋喜欢科幻小说', 'family'), "
            "(3, '银行AI战略规划推进中', 'work')"
        ))
        print(db.execute("SELECT id, content FROM memories WHERE tag = 'project'"))

        print("\n-- 向量混合查询 --")
        print(db.execute(
            "CREATE TABLE emb (id INTEGER, content TEXT, vec VECTOR(3))"
        ))
        print(db.execute(
            "INSERT INTO emb (id, content, vec) VALUES "
            "(1, '会议记录', '[1.0,0.0,0.0]'), "
            "(2, '家庭琐事', '[0.0,1.0,0.0]'), "
            "(3, '战略文档', '[0.9,0.1,0.0]')"
        ))
        print(db.execute(
            "SELECT id, content FROM emb "
            "WHERE vector_similarity(vec, '[1.0,0.0,0.0]') > 0.7 "
            "ORDER BY vector_similarity(vec, '[1.0,0.0,0.0]') DESC"
        ))

    print("\n-- 持久化 + 重开 --")
    path = "/tmp/probedb_py_demo.pdb"
    for f in (path, path + ".wal"):
        if os.path.exists(f):
            os.remove(f)
    with ProbeDB(path) as db:
        print(db.execute("CREATE TABLE m (key TEXT, value TEXT)"))
        print(db.execute("INSERT INTO m (key, value) VALUES ('k1', 'v1'), ('k2', 'v2')"))
        db.persist()
    with ProbeDB(path) as db:
        print(db.execute("SELECT key, value FROM m ORDER BY key ASC"))
    for f in (path, path + ".wal"):
        if os.path.exists(f):
            os.remove(f)


def _self_test():
    """自测：基本 CRUD + 错误传播 + 持久化"""
    with ProbeDB() as db:
        assert "创建成功" in db.execute("CREATE TABLE t (id INTEGER, name TEXT)")
        assert "插入" in db.execute("INSERT INTO t (id, name) VALUES (1, 'alice')")
        out = db.execute("SELECT id, name FROM t")
        assert "alice" in out, out

        # 错误传播
        try:
            db.execute("SELECT * FROM ghost")
            raise AssertionError("应抛出 ProbeDBError")
        except ProbeDBError as e:
            assert "ghost" in str(e), str(e)

    print("✅ 自测通过（CRUD + 错误传播）")


if __name__ == "__main__":
    if "--demo" in sys.argv:
        _demo()
    else:
        _self_test()
        print("提示: 加 --demo 运行完整演示")
