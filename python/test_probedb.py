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
    delete_entry,
    ensure_schema,
    get_entry,
    import_files,
    list_entries,
    parse_memory_text,
    search,
    stats,
    update_entry,
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

    # ---------- SelfMind 迁移前置能力 ----------

    def test_param_binding(self):
        with ProbeDB.open() as db:
            db.execute("CREATE TABLE t (id INTEGER, name TEXT)")
            db.execute("INSERT INTO t (id, name) VALUES (?, ?)", (1, "alice"))
            db.execute("INSERT INTO t (id, name) VALUES (?, ?)", (2, "bob"))
            # 无空格 WHERE（参数绑定的典型产物）
            self.assertEqual(db.execute("SELECT id FROM t WHERE id=?", (2,)).rows, [["2"]])
            # LIKE 参数
            self.assertEqual(db.execute("SELECT id FROM t WHERE name LIKE ?", ("%li%",)).rows, [["1"]])
            # UPDATE ... WHERE ?（SelfMind 的 access_count 写法之外的基本形态）
            db.execute("UPDATE t SET name=? WHERE id=?", ("carol", 1))
            self.assertEqual(db.execute("SELECT name FROM t WHERE id=?", (1,)).rows, [["carol"]])
            # 参数顺序不固化在 SQL 里
            self.assertEqual(
                db.execute("SELECT id FROM t WHERE name=? AND id=?", ("bob", 2)).rows, [["2"]]
            )

    def test_param_binding_errors(self):
        with ProbeDB.open() as db:
            db.execute("CREATE TABLE t (id INTEGER)")
            db.execute("INSERT INTO t (id) VALUES (1)")
            with self.assertRaises(ProbeDBError):
                db.execute("SELECT * FROM t WHERE id=?", ())  # 参数不足
            with self.assertRaises(ProbeDBError):
                db.execute("SELECT * FROM t WHERE id=?", (1, 2))  # 参数过多

    def test_param_binding_null(self):
        with ProbeDB.open() as db:
            db.execute("CREATE TABLE t (id INTEGER, note TEXT)")
            db.execute("INSERT INTO t (id, note) VALUES (?, ?)", (1, None))
            db.execute("INSERT INTO t (id, note) VALUES (?, ?)", (2, "x"))
            self.assertEqual(db.execute("SELECT note FROM t WHERE id=?", (1,)).rows, [["NULL"]])
            self.assertEqual(db.execute("SELECT id FROM t WHERE note IS NULL").rows, [["1"]])
            self.assertEqual(db.execute("SELECT id FROM t WHERE note IS NOT NULL").rows, [["2"]])

    def test_executescript_with_comments_and_if_not_exists(self):
        with ProbeDB.open() as db:
            script = (
                "-- SelfMind schema 风格\n"
                "CREATE TABLE IF NOT EXISTS t (id TEXT, status TEXT);\n"
                "-- 索引\n"
                "CREATE INDEX IF NOT EXISTS idx_status ON t(status);\n"
            )
            results = db.executescript(script)
            self.assertEqual(len(results), 2)
            self.assertEqual(results[0].type, "table_created")
            self.assertEqual(results[1].type, "index_created")
            # 幂等：再跑一遍不报错
            self.assertEqual(len(db.executescript(script)), 2)

    def test_table_alias_join(self):
        with ProbeDB.open() as db:
            db.execute("CREATE TABLE entries (id TEXT, content TEXT)")
            db.execute("CREATE TABLE decay_history (entry_id TEXT, decay_score FLOAT)")
            db.execute("INSERT INTO entries (id, content) VALUES ('e1', 'hello')")
            db.execute("INSERT INTO decay_history (entry_id, decay_score) VALUES ('e1', 0.5)")
            # ON 两侧顺序与 FROM/JOIN 相反（SelfMind 的真实写法）
            r = db.execute(
                "SELECT e.id FROM entries e INNER JOIN decay_history dh "
                "ON dh.entry_id = e.id"
            )
            self.assertEqual(r.rows, [["e1"]])

    def test_unsupported_predicate_raises_not_silent(self):
        # 不支持的条件必须报错，不能静默返回空集
        with ProbeDB.open() as db:
            db.execute("CREATE TABLE t (id TEXT)")
            db.execute("INSERT INTO t (id) VALUES ('a')")
            with self.assertRaises(ProbeDBError) as ctx:
                db.execute("SELECT id FROM t WHERE id IN ('a')")
            self.assertIn("无法解析条件", str(ctx.exception))

    def test_records_by_column_name(self):
        with ProbeDB.open() as db:
            db.execute("CREATE TABLE t (id INTEGER, name TEXT)")
            db.execute("INSERT INTO t (id, name) VALUES (1, 'alice')")
            records = db.execute("SELECT id, name FROM t").records
            self.assertEqual(records, [{"id": "1", "name": "alice"}])


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

    def test_update_entry_with_commas_and_quotes(self):
        entry_id = add_entry(self.db, "old", tags="t1")
        ok = update_entry(self.db, entry_id, content="a, b; It's ok", tags="t2")
        self.assertTrue(ok)

        entry = get_entry(self.db, entry_id)
        self.assertEqual(entry["content"], "a, b; It's ok")
        self.assertEqual(entry["tags"], "t2")
        self.assertEqual(entry["source"], "manual")

    def test_update_missing_entry_returns_false(self):
        self.assertFalse(update_entry(self.db, 999, content="x"))
        self.assertIsNone(get_entry(self.db, 999))

    def test_delete_entry(self):
        entry_id = add_entry(self.db, "to delete")
        self.assertTrue(delete_entry(self.db, entry_id))
        self.assertIsNone(get_entry(self.db, entry_id))
        self.assertFalse(delete_entry(self.db, entry_id))

    def test_delete_persists_across_reopen(self):
        entry_id = add_entry(self.db, "delete me")
        delete_entry(self.db, entry_id)
        self.db.close()

        self.db = ProbeDB.open(str(self.db_path))
        self.assertIsNone(get_entry(self.db, entry_id))
        self.assertEqual(stats(self.db)["total"], 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
