"""ProbeDB Python binding — ctypes wrapper over libprobedb.dylib"""

import ctypes
import platform
from pathlib import Path
from typing import Optional


class ProbeDBError(Exception):
    """ProbeDB operation error."""
    pass


class ProbeDB:
    """Embedded AI-native database — Pythonic interface.

    Usage:
        db = ProbeDB()              # in-memory mode
        db = ProbeDB.open("/tmp/mydb.pdb")  # persistent mode
        db.execute("CREATE TABLE t (id INTEGER, name TEXT)")
        db.execute("INSERT INTO t (id, name) VALUES (1, 'hello')")
        result = db.execute("SELECT * FROM t")
    """

    _lib = None

    @classmethod
    def _load_lib(cls):
        if cls._lib is not None:
            return cls._lib

        # Find the dylib
        candidates = [
            Path(__file__).parent / "libprobedb.dylib",
            Path(__file__).parent / "libprobedb.so",
            Path.home() / "Documents/aiworkspace/probedb/target/release/libprobedb.dylib",
        ]
        lib_path = None
        for p in candidates:
            if p.exists():
                lib_path = str(p)
                break

        if lib_path is None:
            raise ProbeDBError(
                "libprobedb.dylib not found. Build with: cargo build --release --lib\n"
                f"Searched: {[str(p) for p in candidates]}"
            )

        lib = ctypes.CDLL(lib_path)

        # probedb_open(path: *const c_char) -> *mut ProbeDBHandle
        lib.probedb_open.argtypes = [ctypes.c_char_p]
        lib.probedb_open.restype = ctypes.c_void_p

        # probedb_execute(handle, sql) -> *mut c_char
        lib.probedb_execute.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
        lib.probedb_execute.restype = ctypes.c_void_p

        # probedb_persist(handle) -> i32
        lib.probedb_persist.argtypes = [ctypes.c_void_p]
        lib.probedb_persist.restype = ctypes.c_int32

        # probedb_set_batch_mode(handle, enabled) -> i32
        lib.probedb_set_batch_mode.argtypes = [ctypes.c_void_p, ctypes.c_int32]
        lib.probedb_set_batch_mode.restype = ctypes.c_int32

        # probedb_close(handle)
        lib.probedb_close.argtypes = [ctypes.c_void_p]
        lib.probedb_close.restype = None

        # probedb_last_error() -> *mut c_char
        lib.probedb_last_error.argtypes = []
        lib.probedb_last_error.restype = ctypes.c_void_p

        # probedb_free_string(s)
        lib.probedb_free_string.argtypes = [ctypes.c_void_p]
        lib.probedb_free_string.restype = None

        cls._lib = lib
        return lib

    def __init__(self, handle: int):
        self._handle = handle

    @classmethod
    def open(cls, path: Optional[str] = None) -> "ProbeDB":
        """Open (or create) a database.

        Args:
            path: File path for persistent mode. None for in-memory mode.
        """
        lib = cls._load_lib()
        c_path = path.encode("utf-8") if path else None
        handle = lib.probedb_open(c_path)
        if not handle:
            raise ProbeDBError(cls._last_error())
        return cls(handle)

    def execute(self, sql: str) -> str:
        """Execute SQL and return formatted result string."""
        c_sql = sql.encode("utf-8")
        result_ptr = self._lib.probedb_execute(self._handle, c_sql)
        if not result_ptr:
            raise ProbeDBError(self._last_error())
        result = ctypes.cast(result_ptr, ctypes.c_char_p).value.decode("utf-8")
        self._lib.probedb_free_string(result_ptr)
        return result

    def persist(self) -> None:
        """Atomically save current state to disk."""
        ret = self._lib.probedb_persist(self._handle)
        if ret != 0:
            raise ProbeDBError(self._last_error())

    def set_batch_mode(self, enabled: bool) -> None:
        """Enable/disable WAL batch mode for faster bulk writes."""
        ret = self._lib.probedb_set_batch_mode(self._handle, 1 if enabled else 0)
        if ret != 0:
            raise ProbeDBError(self._last_error())

    @classmethod
    def _last_error(cls) -> str:
        """Get last error message from FFI layer."""
        lib = cls._load_lib()
        ptr = lib.probedb_last_error()
        if not ptr:
            return "unknown error"
        msg = ctypes.cast(ptr, ctypes.c_char_p).value.decode("utf-8")
        lib.probedb_free_string(ptr)
        return msg

    def close(self) -> None:
        """Close the database and free resources."""
        if self._handle:
            self._lib.probedb_close(self._handle)
            self._handle = 0

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()
        return False

    def __del__(self):
        if self._handle:
            try:
                self.close()
            except Exception:
                pass  # interpreter shutdown may have unloaded the lib


def _smoke():
    """Quick smoke test."""
    db = ProbeDB.open()
    db.execute("CREATE TABLE test (id INTEGER, name TEXT)")
    db.execute("INSERT INTO test (id, name) VALUES (1, 'hello')")
    db.execute("INSERT INTO test (id, name) VALUES (2, 'world')")
    result = db.execute("SELECT id, name FROM test ORDER BY id ASC")
    assert "hello" in result
    assert "world" in result

    # Vector test
    db.execute("CREATE TABLE vec (id INTEGER, emb VECTOR(3))")
    db.execute("INSERT INTO vec (id, emb) VALUES (1, '[1.0,0.0,0.0]')")
    db.execute("INSERT INTO vec (id, emb) VALUES (2, '[0.0,1.0,0.0]')")
    r = db.execute("SELECT id FROM vec WHERE vector_similarity(emb, '[1.0,0.0,0.0]') > 0.9")
    assert "1" in r

    # Persistence round-trip
    import tempfile, os
    path = os.path.join(tempfile.gettempdir(), "probedb_py_smoke.pdb")
    try:
        db2 = ProbeDB.open(path)
        db2.execute("CREATE TABLE kv (key TEXT, value TEXT)")
        db2.execute("INSERT INTO kv (key, value) VALUES ('x', 'y')")
        db2.persist()
        db2.close()

        db3 = ProbeDB.open(path)
        r = db3.execute("SELECT value FROM kv WHERE key = 'x'")
        assert "y" in r
        db3.close()
    finally:
        for p in [path, path + ".wal", path + ".tmp"]:
            try:
                os.remove(p)
            except OSError:
                pass

    db.close()
    print("✅ Smoke tests passed")


if __name__ == "__main__":
    _smoke()