#!/usr/bin/env python3
"""SelfMind × ProbeDB 兼容性验收（Phase 5.5 acceptance harness）。

对应 Hermes 给出的验收标准：
  1. CREATE INDEX IF NOT EXISTS 不报错
  2. `SELECT e.id FROM entries e JOIN ...` 表别名可跑
  3. `execute(sql, params=(...))` 参数化可用
  4. SelfMind 的 unified_store.SCHEMA_SQL 能通过 ProbeDB 初始化

额外验证（2026-11 起）：
  5. 部分列 INSERT + DEFAULT + AUTOINCREMENT
  6. 约束在 persist/reopen 后仍然生效
  7. 历史/快照/日志表按 SelfMind 真实写法（省略 id）可写入

同时探测已知未补齐能力（DATE()/last_insert_rowid()/事务等），打印为 KNOWN-GAP，
不计入失败——它们属于下一步迁移适配要解决的项。

用法::

    python3 tools/selfmind_compat.py
    python3 tools/selfmind_compat.py --selfmind-root /path/to/selfmind --json
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
import tempfile
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(_REPO_ROOT / "python"))
sys.path.insert(0, str(_REPO_ROOT))

from probedb import ProbeDB, ProbeDBError  # noqa: E402

DEFAULT_SELFMIND_ROOT = Path("~/Documents/selfmind").expanduser()


def load_schema(selfmind_root: Path) -> str:
    """从 SelfMind 源码加载 SCHEMA_SQL（不导入其余模块，避免副作用）。"""
    path = selfmind_root / "selfmind_app" / "unified_store.py"
    if not path.exists():
        raise FileNotFoundError(
            f"找不到 SelfMind unified_store.py: {path}\n"
            "用 --selfmind-root 指定 SelfMind 仓库路径"
        )
    text = path.read_text(encoding="utf-8")
    start = text.find('SCHEMA_SQL = """')
    if start == -1:
        raise RuntimeError("unified_store.py 中未找到 SCHEMA_SQL")
    start += len('SCHEMA_SQL = """')
    end = text.find('"""', start)
    if end == -1:
        raise RuntimeError("SCHEMA_SQL 三引号未闭合")
    return text[start:end]


def insert_all(db: ProbeDB, table: str, values: list) -> None:
    """按表结构全列插入（SelfMind 对 entries 就是全列插入）。"""
    columns = db.execute(f"SELECT * FROM {table} LIMIT 0").columns
    if len(columns) != len(values):
        raise AssertionError(f"{table} 列数 {len(columns)} != 值数 {len(values)}")
    placeholders = ",".join("?" for _ in columns)
    db.execute(
        f"INSERT INTO {table} ({', '.join(columns)}) VALUES ({placeholders})",
        tuple(values),
    )


def probe_gaps(db: ProbeDB) -> list[dict]:
    """探测尚未补齐的 SQL 能力，返回 [{"name","status","detail"}]。"""
    gaps = []

    def check(name: str, fn) -> None:
        try:
            fn()
        except (ProbeDBError, ValueError) as e:
            gaps.append({"name": name, "status": "gap", "detail": str(e).splitlines()[0]})
        else:
            gaps.append({"name": name, "status": "supported", "detail": ""})

    check("NULL 参数", lambda: db.execute("SELECT * FROM entries WHERE id=?", (None,)))
    check(
        "省略自增 id 的 INSERT（部分列）",
        lambda: db.execute(
            "INSERT INTO decay_history (entry_id, timestamp, decay_score) VALUES ('e1','2026-01-02',0.4)"
        ),
    )
    check(
        "表达式 UPDATE（access_count+1）",
        lambda: db.execute("UPDATE entries SET access_count=access_count+1 WHERE id='e1'"),
    )
    check("COUNT(*) 列别名", lambda: db.execute("SELECT COUNT(*) as cnt FROM entries"))
    check("IN 列表谓词", lambda: db.execute("SELECT id FROM entries WHERE id IN ('e1','e2')"))
    check("DATE() 函数", lambda: db.execute("SELECT DATE(timestamp) FROM decay_history LIMIT 1"))
    check(
        "GROUP BY DATE() 表达式",
        lambda: db.execute(
            "SELECT DATE(timestamp), COUNT(*) FROM decay_history GROUP BY DATE(timestamp)"
        ),
    )
    check("last_insert_rowid() SQL 函数", lambda: db.execute("SELECT last_insert_rowid()"))
    check("事务 BEGIN/COMMIT", lambda: db.execute("BEGIN"))
    return gaps


def run(selfmind_root: Path) -> dict:
    schema = load_schema(selfmind_root)
    report: dict = {"selfmind_root": str(selfmind_root), "checks": [], "gaps": []}

    def add(name: str, passed: bool, detail: str = "") -> None:
        report["checks"].append({"name": name, "status": "pass" if passed else "fail", "detail": detail})

    tmpdir = tempfile.mkdtemp(prefix="probedb_selfmind_")
    db_path = str(Path(tmpdir) / "selfmind.pdb")
    ts = "2026-10-26T00:00:00"
    entries_row_id = None

    db = ProbeDB.open(db_path)
    try:
        # 1) schema 初始化
        try:
            results = db.executescript(schema)
            types = {r.type for r in results}
            add("SCHEMA_SQL 初始化", True, f"{len(results)} 条语句，类型={sorted(types)}")
        except (ProbeDBError, ValueError) as e:
            add("SCHEMA_SQL 初始化", False, str(e))
            return report

        # 2) 幂等：再跑一次（CREATE TABLE/INDEX IF NOT EXISTS）
        try:
            db.executescript(schema)
            add("schema 幂等（IF NOT EXISTS）", True)
        except (ProbeDBError, ValueError) as e:
            add("schema 幂等（IF NOT EXISTS）", False, str(e))

        # 3) 全列 INSERT（SelfMind entries 的真实写法）
        try:
            insert_all(
                db,
                "entries",
                [
                    "e1", "hash-e1", "hello world", "hello world", "memory", "src",
                    "hermes", "", "", "", "[]", "", "", "", "", 0.5, 0.25, 0, 1,
                    ts, ts, ts, "", "", "", "active", 0,
                ],
            )
            entries_row_id = db.last_row_id
            add("全列 INSERT（entries）", True)
        except (ProbeDBError, ValueError) as e:
            add("全列 INSERT（entries）", False, str(e))

        # 4) 部分列 INSERT + DEFAULT + AUTOINCREMENT（历史/日志表的真实写法）
        try:
            db.execute(
                "INSERT INTO entries (id, content_hash, content, source) "
                "VALUES (?, ?, ?, ?)",
                ("e2", "hash-e2", "partial row", "src2"),
            )
            r = db.execute(
                "SELECT type, source_profile, tags, importance, decay_score, "
                "access_count, version, status, pinned FROM entries WHERE id=?",
                ("e2",),
            )
            expected = [["memory", "hermes", "[]", "0.5", "0.25", "0", "1", "active", "0"]]
            add("部分列 INSERT（DEFAULT 生效）", r.rows == expected, f"rows={r.rows}")

            # entry_history：省略自增 id + trigger 默认值 'sync'
            db.execute(
                "INSERT INTO entry_history (entry_id, version, content_hash, content, timestamp) "
                "VALUES (?, ?, ?, ?, ?)",
                ("e1", 1, "hash-e1", "body", ts),
            )
            r = db.execute(
                "SELECT id, trigger, content_preview FROM entry_history WHERE entry_id=?",
                ("e1",),
            )
            add(
                "entry_history：自增 id + 默认 trigger",
                len(r.rows) == 1 and r.rows[0][1] == "sync" and r.rows[0][0] != "",
                f"rows={r.rows}",
            )

            # snapshots：省略 id / user_md / stats / trigger（trigger 默认 sync）
            db.execute(
                "INSERT INTO snapshots (timestamp, memory_md) VALUES (?, ?)",
                (ts, "mem"),
            )
            r = db.execute("SELECT id, trigger, user_md FROM snapshots")
            add(
                "snapshots：自增 id + 默认 trigger + NULL 列",
                len(r.rows) == 1 and r.rows[0][1] == "sync" and r.rows[0][2] == "NULL",
                f"rows={r.rows}",
            )

            # decay_history / operations_log：省略 id
            db.execute(
                "INSERT INTO decay_history (entry_id, timestamp, decay_score) VALUES (?, ?, ?)",
                ("e1", ts, 0.5),
            )
            db.execute(
                "INSERT INTO operations_log (timestamp, operation) VALUES (?, ?)",
                (ts, "add"),
            )
            counts_ok = (
                db.execute("SELECT COUNT(*) FROM decay_history").rows == [["1"]]
                and db.execute("SELECT COUNT(*) FROM operations_log").rows == [["1"]]
                and db.execute(
                    "SELECT auto_or_manual FROM operations_log LIMIT 1"
                ).rows == [["auto"]]
            )
            add(
                "decay_history / operations_log 写入",
                counts_ok,
                "counts/默认值校验",
            )
        except (ProbeDBError, ValueError) as e:
            add("部分列 INSERT（DEFAULT/AUTOINCREMENT）", False, str(e))

        # 5) 参数化查询
        try:
            r = db.execute("SELECT id, content FROM entries WHERE id=?", ("e1",))
            add("参数化查询（? 占位符，无空格 WHERE）", r.rows == [["e1", "hello world"]], f"rows={r.rows}")
        except (ProbeDBError, ValueError) as e:
            add("参数化查询（? 占位符，无空格 WHERE）", False, str(e))

        try:
            r = db.execute("SELECT COUNT(*) FROM entries WHERE status=?", ("active",))
            add("参数化 + COUNT(*)", r.rows == [["2"]], f"rows={r.rows}")
        except (ProbeDBError, ValueError) as e:
            add("参数化 + COUNT(*)", False, str(e))

        # 6) 表别名 JOIN（SelfMind 写法：右表在前）
        try:
            r = db.execute(
                "SELECT e.id FROM entries e INNER JOIN decay_history dh "
                "ON dh.entry_id = e.id"
            )
            add("表别名 JOIN", r.rows == [["e1"]], f"rows={r.rows}")
        except (ProbeDBError, ValueError) as e:
            add("表别名 JOIN", False, str(e))

        # 7) last_row_id / records
        add(
            "last_row_id",
            entries_row_id == 1,
            f"entries 首次插入后 last_row_id={entries_row_id}",
        )
        try:
            recs = db.execute("SELECT id, content FROM entries WHERE id=?", ("e1",)).records
            add("按列名取行（records）", recs == [{"id": "e1", "content": "hello world"}], str(recs))
        except (ProbeDBError, ValueError) as e:
            add("按列名取行（records）", False, str(e))

        # 8) 约束在 persist/reopen 后仍然生效
        try:
            db.persist()
            db.close()
            db = ProbeDB.open(db_path)
            db.execute(
                "INSERT INTO entries (id, content_hash, content, source) "
                "VALUES (?, ?, ?, ?)",
                ("e3", "hash-e3", "after reopen", "src3"),
            )
            r = db.execute(
                "SELECT type, importance, status FROM entries WHERE id=?", ("e3",)
            )
            add(
                "persist/reopen 后 DEFAULT/AUTOINCREMENT 仍生效",
                r.rows == [["memory", "0.5", "active"]],
                f"rows={r.rows}",
            )
        except (ProbeDBError, ValueError) as e:
            add("persist/reopen 后 DEFAULT/AUTOINCREMENT 仍生效", False, str(e))

        # 9) 缺口探测
        report["gaps"] = probe_gaps(db)
    finally:
        try:
            db.close()
        except Exception:
            pass
        shutil.rmtree(tmpdir, ignore_errors=True)

    report["passed"] = all(c["status"] == "pass" for c in report["checks"])
    return report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="SelfMind × ProbeDB 兼容性验收")
    parser.add_argument("--selfmind-root", type=Path, default=DEFAULT_SELFMIND_ROOT)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)

    try:
        report = run(args.selfmind_root)
    except FileNotFoundError as e:
        print(f"❌ {e}", file=sys.stderr)
        return 2

    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    else:
        print("=== SelfMind × ProbeDB 验收 ===")
        for c in report["checks"]:
            icon = "✅" if c["status"] == "pass" else "❌"
            line = f"{icon} {c['name']}"
            if c["detail"]:
                line += f" — {c['detail']}"
            print(line)
        if report["gaps"]:
            print("\n--- 能力探测（⚠️ = 尚未支持，迁移前需处理）---")
            for g in report["gaps"]:
                icon = "⚠️" if g["status"] == "gap" else "✅"
                line = f"{icon} {g['name']}"
                if g["detail"]:
                    line += f" — {g['detail']}"
                print(line)
        print("\n结果:", "PASS" if report["passed"] else "FAIL")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
