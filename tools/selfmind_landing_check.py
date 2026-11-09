#!/usr/bin/env python3
"""验证 SelfMind「ProbeDB 后端安装包」在真实数据上可用。

流程：
1. 把 SelfMind 的 `selfmind_app/` 复制到临时目录（不碰源仓库）；
2. 用 `integrations/selfmind/install.py` 打补丁（连接工厂 + probedb_conn）；
3. 从临时副本导入打过补丁的 `UnifiedStore`；
4. entries 导入 ProbeDB，分别用 sqlite/probedb 后端跑读接口并逐行比对。

用法::

    python3 tools/selfmind_landing_check.py
    python3 tools/selfmind_landing_check.py --sqlite-db ~/Documents/selfmind/data/selfmind.db
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sqlite3
import sys
import tempfile
from pathlib import Path
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(_REPO_ROOT))
sys.path.insert(0, str(_REPO_ROOT / "python"))

from probedb import ProbeDB  # noqa: E402
from integrations.selfmind.install import install  # noqa: E402

DEFAULT_SELFMIND_ROOT = Path("~/Documents/selfmind").expanduser()


def normalize(value: Any) -> Any:
    if isinstance(value, float):
        return round(value, 6)
    return value


def values_equal(a: Any, b: Any) -> bool:
    a, b = normalize(a), normalize(b)
    if isinstance(a, float) or isinstance(b, float):
        try:
            return abs(float(a) - float(b)) <= 1e-3
        except (TypeError, ValueError):
            return a == b
    return a == b


def import_entries(src_db: Path, probedb_path: Path, schema: str) -> int:
    src = sqlite3.connect(f"file:{src_db}?mode=ro", uri=True)
    src.row_factory = sqlite3.Row
    rows = [dict(r) for r in src.execute("SELECT * FROM entries")]
    recalls = [dict(r) for r in src.execute("SELECT * FROM agent_recall_log")]
    src.close()

    db = ProbeDB.open(str(probedb_path))
    db.executescript(schema)
    for table, items in (("entries", rows), ("agent_recall_log", recalls)):
        if not items:
            continue
        columns = list(items[0].keys())
        sql = f"INSERT INTO {table} ({', '.join(columns)}) VALUES ({', '.join('?' for _ in columns)})"
        for item in items:
            db.execute(sql, tuple(item[c] for c in columns))
    db.persist()
    db.close()
    return len(rows)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="SelfMind ProbeDB 安装包验证")
    parser.add_argument("--selfmind-root", type=Path, default=DEFAULT_SELFMIND_ROOT)
    parser.add_argument("--sqlite-db", type=Path, default=None)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)

    root = args.selfmind_root.expanduser()
    sqlite_db = (args.sqlite_db or (root / "data" / "selfmind_hermes.db")).expanduser()
    if not (root / "selfmind_app" / "unified_store.py").exists():
        print(f"❌ SelfMind 源码不存在: {root}", file=sys.stderr)
        return 2
    if not sqlite_db.exists():
        print(f"❌ 数据库不存在: {sqlite_db}", file=sys.stderr)
        return 2

    tmp = Path(tempfile.mkdtemp(prefix="selfmind_landing_"))
    copy_root = tmp / "selfmind"
    sqlite_copy = tmp / "selfmind_sqlite.db"
    probedb_path = tmp / "selfmind_probedb.pdb"
    report: dict[str, Any] = {"checks": []}

    def add(name: str, ok: bool, detail: str = "") -> None:
        report["checks"].append({"name": name, "status": "pass" if ok else "fail", "detail": detail})

    try:
        # 1) 复制 SelfMind 源码 + 数据库副本（源仓库只读）
        shutil.copytree(root / "selfmind_app", copy_root / "selfmind_app")
        shutil.copy2(sqlite_db, sqlite_copy)

        # 2) 在副本上安装 ProbeDB 后端
        rc = install(copy_root, dry_run=False)
        add("安装脚本执行", rc == 0, f"install() rc={rc}")
        patched = (copy_root / "selfmind_app" / "unified_store.py").read_text(encoding="utf-8")
        add("连接工厂已注入", "_make_connection" in patched)
        add("probedb_conn.py 已复制", (copy_root / "selfmind_app" / "probedb_conn.py").exists())

        # 3) 导入打过补丁的 UnifiedStore（临时副本优先）
        sys.path.insert(0, str(copy_root))
        for mod in list(sys.modules):
            if mod == "selfmind_app" or mod.startswith("selfmind_app."):
                del sys.modules[mod]
        from selfmind_app.unified_store import SCHEMA_SQL, UnifiedStore  # noqa: E402

        # 4) entries 导入 ProbeDB
        count = import_entries(sqlite_copy, probedb_path, SCHEMA_SQL)
        add("entries 导入 ProbeDB", count > 0, f"{count} 条")

        # 5) 两种后端分别实例化（同一份补丁代码）
        os.environ.pop("SELFMIND_BACKEND", None)
        store_sqlite = UnifiedStore(str(sqlite_copy))

        os.environ["SELFMIND_BACKEND"] = "probedb"
        os.environ["SELFMIND_PROBEDB_PATH"] = str(probedb_path)
        store_probedb = UnifiedStore(str(probedb_path))
        add("UnifiedStore 以 probedb 后端启动", True)

        entries_sqlite = store_sqlite.get_all_entries()
        entries_probedb = store_probedb.get_all_entries()
        diffs = []
        by_id_sqlite = {e["id"]: e for e in entries_sqlite}
        by_id_probedb = {e["id"]: e for e in entries_probedb}
        for entry_id in sorted(set(by_id_sqlite) & set(by_id_probedb)):
            for key, value in by_id_sqlite[entry_id].items():
                if not values_equal(value, by_id_probedb[entry_id].get(key)):
                    diffs.append(f"{entry_id}.{key}: {value!r} != {by_id_probedb[entry_id].get(key)!r}")
        add(
            "get_all_entries 逐行一致",
            len(entries_sqlite) == len(entries_probedb) and not diffs,
            f"{len(entries_sqlite)} vs {len(entries_probedb)} rows, diffs={len(diffs)}"
            + ("; " + "; ".join(diffs[:3]) if diffs else ""),
        )

        stats_sqlite = store_sqlite.get_stats()
        stats_probedb = store_probedb.get_stats()
        stats_keys = [
            "by_type_status", "total_active", "total_inactive", "by_type",
            "avg_decay", "pinned", "fading_candidates",
        ]
        stats_diffs = []
        for key in stats_keys:
            va, vb = stats_sqlite.get(key), stats_probedb.get(key)
            if isinstance(va, dict) and isinstance(vb, dict):
                if set(va) != set(vb) or any(not values_equal(va[k], vb[k]) for k in va):
                    stats_diffs.append(key)
            elif not values_equal(va, vb):
                stats_diffs.append(key)
        add("get_stats 一致", not stats_diffs, f"diffs={stats_diffs}")

        store_sqlite.close()
        store_probedb.close()
    finally:
        os.environ.pop("SELFMIND_BACKEND", None)
        os.environ.pop("SELFMIND_PROBEDB_PATH", None)
        shutil.rmtree(tmp, ignore_errors=True)

    report["passed"] = all(c["status"] == "pass" for c in report["checks"])
    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    else:
        print("=== SelfMind ProbeDB 安装包验证 ===")
        for c in report["checks"]:
            print(f"{'✅' if c['status'] == 'pass' else '❌'} {c['name']}" + (f" — {c['detail']}" if c["detail"] else ""))
        print("\n结果:", "PASS" if report["passed"] else "FAIL")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
