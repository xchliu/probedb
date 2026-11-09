"""ProbeDBConnection（SelfMind sqlite3 兼容层）单元测试。

运行：`python3 -m unittest discover -s python -v`
"""

from __future__ import annotations

import os
import sys
import tempfile
import unittest
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(_REPO_ROOT))
sys.path.insert(0, str(_REPO_ROOT / "python"))

from integrations.selfmind.probedb_conn import ProbeDBConnection, Row  # noqa: E402

SCHEMA = """
CREATE TABLE t (
    id TEXT,
    n INTEGER,
    x REAL,
    pinned INTEGER,
    note TEXT
);
"""


class ProbeDBConnectionTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.path = os.path.join(self._tmp.name, "test.pdb")
        self.conn = ProbeDBConnection(self.path)
        self.conn.executescript(SCHEMA)

    def tearDown(self):
        try:
            self.conn.close()
        finally:
            self._tmp.cleanup()

    def test_row_by_name_index_and_dict(self):
        self.conn.execute(
            "INSERT INTO t (id, n, x, pinned, note) VALUES (?, ?, ?, ?, ?)",
            ("a", 1, 0.5, 0, None),
        )
        cursor = self.conn.execute("SELECT id, n, x, pinned, note FROM t")
        row = cursor.fetchone()
        self.assertIsInstance(row, Row)
        self.assertEqual(row["id"], "a")
        self.assertEqual(row[0], "a")
        self.assertEqual(row["n"], 1)
        self.assertEqual(row["x"], 0.5)
        self.assertEqual(row["pinned"], 0)
        self.assertIsNone(row["note"])
        self.assertEqual(dict(row)["n"], 1)

    def test_last_insert_rowid(self):
        cur = self.conn.execute("INSERT INTO t (id) VALUES ('a')")
        self.assertEqual(cur.lastrowid, 1)
        row = self.conn.execute("SELECT last_insert_rowid()").fetchone()
        self.assertEqual(row[0], 1)
        self.conn.execute("INSERT INTO t (id) VALUES ('b')")
        self.assertEqual(self.conn.execute("SELECT last_insert_rowid()").fetchone()[0], 2)

    def test_aggregate_column_types(self):
        for i in range(3):
            self.conn.execute(
                "INSERT INTO t (id, n, x) VALUES (?, ?, ?)", (f"e{i}", i, float(i))
            )
        row = self.conn.execute(
            "SELECT COUNT(*) as cnt, AVG(x) as avg_x FROM t"
        ).fetchone()
        self.assertEqual(row["cnt"], 3)
        self.assertIsInstance(row["avg_x"], float)

    def test_commit_persists_and_reopens(self):
        self.conn.execute("INSERT INTO t (id, note) VALUES ('keep', 'me')")
        self.conn.commit()
        self.conn.close()
        self.conn = ProbeDBConnection(self.path)
        row = self.conn.execute("SELECT note FROM t WHERE id=?", ("keep",)).fetchone()
        self.assertEqual(row["note"], "me")

    def test_executemany(self):
        self.conn.executemany(
            "INSERT INTO t (id, n) VALUES (?, ?)",
            [("a", 1), ("b", 2)],
        )
        self.assertEqual(self.conn.execute("SELECT COUNT(*) as cnt FROM t").fetchone()["cnt"], 2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
