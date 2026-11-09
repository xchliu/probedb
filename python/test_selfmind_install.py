"""SelfMind 安装器（integrations/selfmind/install.py）单元测试。"""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(_REPO_ROOT))

from integrations.selfmind.install import install, patch_source  # noqa: E402

SAMPLE = '''import os
import sqlite3
from datetime import datetime, timedelta


class UnifiedStore:
    def __init__(self, db_path):
        self.db_path = db_path
        self.conn = sqlite3.connect(db_path, check_same_thread=False)
        self.conn.row_factory = sqlite3.Row

    def switch_db(self, new_db_path):
        self.conn.close()
        self.conn = sqlite3.connect(new_db_path, check_same_thread=False)
'''


class InstallerTests(unittest.TestCase):
    def test_patch_source_injects_factory_and_backends(self):
        patched, notes = patch_source(SAMPLE)
        self.assertIn("_make_connection", patched)
        self.assertIn("self.conn = _make_connection(db_path)", patched)
        self.assertIn("self.conn = _make_connection(new_db_path)", patched)
        self.assertNotIn("sqlite3.connect(new_db_path", patched)
        self.assertTrue(any("inserted" in n for n in notes))

    def test_patch_source_is_idempotent(self):
        once, _ = patch_source(SAMPLE)
        twice, notes = patch_source(once)
        self.assertEqual(once, twice)
        self.assertEqual(notes, ["already patched"])

    def test_patch_source_rejects_unexpected_structure(self):
        with self.assertRaises(RuntimeError):
            patch_source("def foo():\n    pass\n")

    def test_install_on_temp_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            app = root / "selfmind_app"
            app.mkdir()
            (app / "unified_store.py").write_text(SAMPLE, encoding="utf-8")
            self.assertEqual(install(root, dry_run=False), 0)
            patched = (app / "unified_store.py").read_text(encoding="utf-8")
            self.assertIn("_make_connection", patched)
            self.assertTrue((app / "probedb_conn.py").exists())
            self.assertTrue((app / "unified_store.py.bak").exists())
            # 第二次安装应幂等
            self.assertEqual(install(root, dry_run=False), 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
