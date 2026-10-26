"""ProbeDB Python 层测试（标准库 unittest，可重复执行）。

运行::

    python3 -m unittest discover -s python -v
"""

from __future__ import annotations

import os
import sys
import tempfile
import unittest
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(_REPO_ROOT / "python"))
sys.path.insert(0, str(_REPO_ROOT))

from probedb import ProbeDB, ProbeDBError  # noqa: E402

from tools.probedb_mem import (  # noqa: E402
    add_entry,
    ensure_schema,
    import_files,
    list_entries,
    parse_memory_text,
    search,
    stats,
)


class BindingTests(unittest.TestCase):
    def test_crud_and_column_reorder(self):
        with ProbeDB.open() as db:
            db.execute("CREATE TABLE t (id INTEGER, name TEXT)")
            db.execute("INSERT INTO t (id, name) VALUES (1, 'alice')")
            # 列清单顺序与表结构不同 → 必须按列名映射
            db.execute("INSERT INTO t (name, id) VALUES ('bob', 2)")

            r = db.execute("SELECT id, name FROM t ORDER BY id ASC")
            self.assertEqual(r.type, "select")
            self.assertEqual(r.columns, ["id", "name"])
            self.assertEqual(r.rows, [["1", "alice"], ["2", "bob"]])
            self.assertEqual(str(r).splitlines()[0], "查询结果 (2 行):")

    def test_error_raises(self):
        with ProbeDB.open() as db:
            with self.assertRaises(ProbeDBError) as ctx:
                db.execute("SELECT * FROM ghost")
            self.assertIn("ghost", str(ctx.exception))

            with self.assertRaises(ProbeDBError):
                db.execute("CREATE TABLE t (id INTEGER)")
                db.execute("INSERT INTO t (id) VALUES ('not-an-int')")

    def test_structured_result_types(self):
        with ProbeDB.open() as db:
            self.assertEqual(db.execute("CREATE TABLE t (id INTEGER)").type, "table_created")
            self.assertEqual(db.execute("INSERT INTO t (id) VALUES (7)").row_id, 1)
            self.assertEqual(db.execute("UPDATE t SET id = 8").count, 1)
            self.assertEqual(db.execute("DELETE FROM t").count, 1)

    def test_persistence_roundtrip(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "mem.pdb")
            with ProbeDB.open(path) as db:
                db.execute("CREATE TABLE t (id INTEGER, note TEXT)")
                db.execute("INSERT INTO t (id, note) VALUES (1, 'It''s a (test); ok')")
                db.persist()

            with ProbeDB.open(path) as db:
                rows = db.execute("SELECT note FROM t").rows
                self.assertEqual(rows, [["It's a (test); ok"]])


class MemoryToolTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.db_path = self.tmp / "hermes.pdb"
        self.db = ProbeDB.open(str(self.db_path))

    def tearDown(self):
        self.db.close()
        self._tmp.cleanup()

    def _write_memory(self, name: str, text: str) -> Path:
        path = self.tmp / name
        path.write_text(text, encoding="utf-8")
        return path

    def test_parse_memory_text(self):
        text = (
            "[ProbeDB] 周一坨哥定计划\n"
            "§\n"
            "坦哥=刘小成,北银金科AI部门负责人\n"
            "§\n"
            "无标签的一条记忆\n"
        )
        entries = parse_memory_text(text, "MEMORY")
        self.assertEqual(len(entries), 3)
        self.assertEqual(entries[0].tags, "ProbeDB")
        self.assertEqual(entries[1].content, "坦哥=刘小成,北银金科AI部门负责人")
        self.assertEqual(entries[2].tags, "")

    def test_import_search_stats(self):
        MEMORY = self._write_memory(
            "MEMORY.md",
            "[ProbeDB] 周一坨哥定计划wiki定任务\n§\n[模型偏好] 默认deepseek-v4-pro\n",
        )
        USER = self._write_memory("USER.md", "坦哥关注SelfMind进展\n")

        count = import_files(self.db, [MEMORY, USER], reset=True)
        self.assertEqual(count, 3)

        hits = search(self.db, "ProbeDB")
        self.assertEqual(len(hits), 1)
        self.assertEqual(hits[0]["tags"], "ProbeDB")
        self.assertEqual(hits[0]["source"], "MEMORY")

        # 标签命中优先
        hits = search(self.db, "模型偏好")
        self.assertEqual(hits[0]["tags"], "模型偏好")

        info = stats(self.db)
        self.assertEqual(info["total"], 3)
        self.assertEqual(info["by_source"], {"MEMORY": 2, "USER": 1})

    def test_import_is_resettable(self):
        MEMORY = self._write_memory("MEMORY.md", "a\n§\nb\n")
        import_files(self.db, [MEMORY], reset=True)
        import_files(self.db, [MEMORY], reset=True)  # 再次导入默认清空
        self.assertEqual(len(list_entries(self.db, 100)), 2)

    def test_add_entry_with_quotes_and_semicolons(self):
        ensure_schema(self.db)
        entry_id = add_entry(
            self.db,
            "It's a (test); with | pipe and \\ backslash",
            tags="测试",
            source="manual",
        )
        self.assertEqual(entry_id, 1)
        hits = search(self.db, "It's")
        self.assertEqual(len(hits), 1)
        self.assertEqual(hits[0]["content"], "It's a (test); with | pipe and \\ backslash")

    def test_add_entry_persists_across_reopen(self):
        ensure_schema(self.db)
        add_entry(self.db, "持久化测试", tags="t", source="manual")
        self.db.close()

        self.db = ProbeDB.open(str(self.db_path))
        hits = search(self.db, "持久化")
        self.assertEqual(len(hits), 1)

    def test_search_miss_returns_empty(self):
        ensure_schema(self.db)
        add_entry(self.db, "hello world")
        self.assertEqual(search(self.db, "nonexistent-xyz"), [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
