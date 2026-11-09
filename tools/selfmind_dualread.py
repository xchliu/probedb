#!/usr/bin/env python3
"""SelfMind 双读比对：SQLite（现状） vs ProbeDB（适配层原型）。

做三件事：
1. 把 SelfMind 的 `UnifiedStore` 分别挂到 SQLite 和 ProbeDB 上（ProbeDB 用
   integrations/selfmind/probedb_conn.py 的 sqlite3 兼容层，业务代码零改动）；
2. 用真实数据库把 `entries` 表导入 ProbeDB（history/snapshots 仍留 SQLite）；
3. 对比 `get_all_entries / get_entry / get_entries_by_type / get_stats` 的结果与耗时。

源库只读；SQLite 副本与 ProbeDB 库都写在临时目录。

用法::

    python3 tools/selfmind_dualread.py
    python3 tools/selfmind_dualread.py --sqlite-db ~/Documents/selfmind/data/selfmind.db
"""

from __future__ import annotations

import argparse
import json
import shutil
import sqlite3
import statistics
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(_REPO_ROOT))
sys.path.insert(0, str(_REPO_ROOT / "python"))

from probedb import ProbeDB  # noqa: E402
from integrations.selfmind.probedb_conn import ProbeDBConnection  # noqa: E402

DEFAULT_SELFMIND_ROOT = Path("~/Documents/selfmind").expanduser()


def load_schema(selfmind_root: Path) -> str:
    text = (selfmind_root / "selfmind_app" / "unified_store.py").read_text(encoding="utf-8")
    start = text.find('SCHEMA_SQL = """') + len('SCHEMA_SQL = """')
    end = text.find('"""', start)
    if start <= 0 or end == -1:
        raise RuntimeError("unified_store.py 中未找到 SCHEMA_SQL")
    return text[start:end]


def import_entries(src_db: Path, probedb_path: Path, schema: str) -> dict:
    """把 SQLite entries（以及 agent_recall_log）导入 ProbeDB，返回统计。"""
    src = sqlite3.connect(f"file:{src_db}?mode=ro", uri=True)
    src.row_factory = sqlite3.Row
    entries = [dict(r) for r in src.execute("SELECT * FROM entries")]
    recalls = [dict(r) for r in src.execute("SELECT * FROM agent_recall_log")]
    src.close()

    db = ProbeDB.open(str(probedb_path))
    db.executescript(schema)

    def insert_rows(table: str, rows: list[dict]) -> None:
        if not rows:
            return
        columns = list(rows[0].keys())
        sql = (
            f"INSERT INTO {table} ({', '.join(columns)}) "
            f"VALUES ({', '.join('?' for _ in columns)})"
        )
        for row in rows:
            db.execute(sql, tuple(row[c] for c in columns))

    insert_rows("entries", entries)
    insert_rows("agent_recall_log", recalls)
    db.persist()
    db.close()
    return {"entries": len(entries), "agent_recall_log": len(recalls)}


def normalize(value: Any) -> Any:
    if isinstance(value, float):
        return round(value, 6)
    return value


def values_equal(a: Any, b: Any) -> bool:
    """浮点聚合结果允许 1e-3 容差（SQLite 与 Rust 的求和/舍入路径不同）。"""
    a, b = normalize(a), normalize(b)
    if isinstance(a, float) or isinstance(b, float):
        try:
            return abs(float(a) - float(b)) <= 1e-3
        except (TypeError, ValueError):
            return a == b
    return a == b


def compare_dicts(a: dict, b: dict, keys: list[str]) -> list[str]:
    diffs = []
    for key in keys:
        va, vb = a.get(key), b.get(key)
        equal = True
        if isinstance(va, dict) and isinstance(vb, dict):
            equal = set(va.keys()) == set(vb.keys()) and all(
                values_equal(va[k], vb[k]) for k in va
            )
        else:
            equal = values_equal(va, vb)
        if not equal:
            diffs.append(f"{key}: sqlite={normalize(va)!r} probedb={normalize(vb)!r}")
    return diffs


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="SelfMind SQLite vs ProbeDB 双读比对")
    parser.add_argument("--selfmind-root", type=Path, default=DEFAULT_SELFMIND_ROOT)
    parser.add_argument(
        "--sqlite-db",
        type=Path,
        default=None,
        help="默认使用 <selfmind-root>/data/selfmind_hermes.db",
    )
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)

    root = args.selfmind_root.expanduser()
    src_db = (args.sqlite_db or (root / "data" / "selfmind_hermes.db")).expanduser()
    if not src_db.exists():
        print(f"❌ 数据库不存在: {src_db}", file=sys.stderr)
        return 2

    sys.path.insert(0, str(root))
    from selfmind_app.unified_store import UnifiedStore  # noqa: E402

    schema = load_schema(root)
    tmp = Path(tempfile.mkdtemp(prefix="selfmind_dualread_"))
    sqlite_copy = tmp / "selfmind_sqlite.db"
    probedb_path = tmp / "selfmind_probedb.pdb"
    report: dict[str, Any] = {"source": str(src_db), "checks": []}

    def add(name: str, ok: bool, detail: str = "") -> None:
        report["checks"].append({"name": name, "status": "pass" if ok else "fail", "detail": detail})

    try:
        # 1) 复制 SQLite（UnifiedStore 构造会写 schema，不能动源库）
        shutil.copy2(src_db, sqlite_copy)

        # 2) 导入 entries 到 ProbeDB
        t0 = time.perf_counter()
        imported = import_entries(src_db, probedb_path, schema)
        import_ms = (time.perf_counter() - t0) * 1000
        report["import"] = {**imported, "ms": round(import_ms, 1)}
        add("entries 导入", imported["entries"] > 0, f"{imported}，{import_ms:.1f}ms")

        # 3) 同一个 UnifiedStore 类，分别挂 SQLite / ProbeDB
        store_sqlite = UnifiedStore(str(sqlite_copy))

        original_connect = sqlite3.connect
        try:
            sqlite3.connect = lambda *a, **k: ProbeDBConnection(str(probedb_path))  # type: ignore
            store_probedb = UnifiedStore(str(probedb_path))
        finally:
            sqlite3.connect = original_connect

        # 4) get_all_entries 对比 + 耗时
        times_sqlite, times_probedb = [], []
        for _ in range(5):
            t = time.perf_counter(); entries_sqlite = store_sqlite.get_all_entries(); times_sqlite.append((time.perf_counter() - t) * 1000)
            t = time.perf_counter(); entries_probedb = store_probedb.get_all_entries(); times_probedb.append((time.perf_counter() - t) * 1000)

        report["get_all_entries"] = {
            "sqlite_rows": len(entries_sqlite),
            "probedb_rows": len(entries_probedb),
            "sqlite_avg_ms": round(statistics.mean(times_sqlite), 1),
            "probedb_avg_ms": round(statistics.mean(times_probedb), 1),
        }

        # 逐行对比（按 id 对齐，比较所有列）
        by_id_sqlite = {e["id"]: e for e in entries_sqlite}
        by_id_probedb = {e["id"]: e for e in entries_probedb}
        missing = set(by_id_sqlite) - set(by_id_probedb)
        extra = set(by_id_probedb) - set(by_id_sqlite)
        field_diffs = []
        for entry_id in sorted(set(by_id_sqlite) & set(by_id_probedb)):
            a, b = by_id_sqlite[entry_id], by_id_probedb[entry_id]
            for key in a.keys():
                if normalize(a[key]) != normalize(b.get(key)):
                    field_diffs.append(f"{entry_id}.{key}: {a[key]!r} != {b.get(key)!r}")
        add(
            "get_all_entries 逐行一致",
            not missing and not extra and not field_diffs,
            f"missing={len(missing)} extra={len(extra)} field_diffs={len(field_diffs)}"
            + ("; " + "; ".join(field_diffs[:3]) if field_diffs else ""),
        )

        # 5) get_entry 抽样
        sample_ids = sorted(by_id_sqlite)[:3] + sorted(by_id_sqlite)[-3:]
        entry_diffs = []
        for entry_id in sample_ids:
            a, b = store_sqlite.get_entry(entry_id), store_probedb.get_entry(entry_id)
            if a is None or b is None:
                entry_diffs.append(f"{entry_id}: sqlite={a is not None} probedb={b is not None}")
                continue
            for key in a.keys():
                if normalize(a[key]) != normalize(b.get(key)):
                    entry_diffs.append(f"{entry_id}.{key}: {a[key]!r} != {b.get(key)!r}")
        add("get_entry 抽样一致", not entry_diffs, "; ".join(entry_diffs[:3]) or f"samples={len(sample_ids)}")

        # 6) get_entries_by_type 对比
        type_stats = store_sqlite.get_stats().get("by_type", {})
        type_diffs = []
        for type_val in sorted(type_stats):
            a = store_sqlite.get_entries_by_type(type_val)
            b = store_probedb.get_entries_by_type(type_val)
            if len(a) != len(b):
                type_diffs.append(f"{type_val}: count {len(a)} != {len(b)}")
        add("get_entries_by_type 对比", not type_diffs, "; ".join(type_diffs[:3]) or f"types={len(type_stats)}")

        # 7) get_stats 对比（只比 entries 派生字段；history/snapshots 仍在 SQLite）
        stats_keys = [
            "by_type_status", "total_active", "total_inactive", "by_type",
            "avg_decay", "pinned", "fading_candidates",
        ]
        stats_sqlite = store_sqlite.get_stats()
        stats_probedb = store_probedb.get_stats()
        diffs = compare_dicts(stats_sqlite, stats_probedb, stats_keys)
        add(
            "get_stats（entries 派生字段）一致",
            not diffs,
            "; ".join(diffs[:3]) or f"keys={stats_keys}",
        )

        store_sqlite.close()
        store_probedb.close()
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    report["passed"] = all(c["status"] == "pass" for c in report["checks"])

    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    else:
        print("=== SelfMind 双读比对（SQLite vs ProbeDB）===")
        print(f"源库: {src_db}")
        if report.get("import"):
            print(f"导入: {report['import']}")
        for c in report["checks"]:
            print(f"{'✅' if c['status'] == 'pass' else '❌'} {c['name']}" + (f" — {c['detail']}" if c["detail"] else ""))
        if report.get("get_all_entries"):
            g = report["get_all_entries"]
            print(
                f"\nget_all_entries 耗时: SQLite {g['sqlite_avg_ms']}ms / "
                f"ProbeDB {g['probedb_avg_ms']}ms"
            )
        print("\n结果:", "PASS" if report["passed"] else "FAIL")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
