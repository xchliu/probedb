#!/usr/bin/env python3
"""ProbeDB 记忆场景基准测试（零外部依赖，可重复执行）。

度量：
- 批量导入 N 条记忆的耗时
- 落盘后数据库/WAL 体积
- 重新打开（进程启动恢复）耗时
- 关键词检索平均延迟（Python 侧全量扫描 + 过滤）
- CLI 单次调用端到端耗时（Hermes 若走 CLI 的真实成本）

用法::

    python3 tools/bench_probedb.py
    python3 tools/bench_probedb.py --sizes 100,1000,10000 --json
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(_REPO_ROOT / "python"))
sys.path.insert(0, str(_REPO_ROOT))

from probedb import ProbeDB  # noqa: E402
from tools.probedb_mem import ensure_schema, search, sql_literal  # noqa: E402

TAG_POOL = ["ProbeDB", "模型偏好", "工作方式", "记忆", "项目", "工具坑", "投资观", "公众号"]


def make_entry(i: int) -> tuple[str, str]:
    tag = TAG_POOL[i % len(TAG_POOL)]
    content = (
        f"[{tag}] 记忆条目 {i}：坦哥关注的项目进展与决策记录，"
        f"包含一些用于检索的中文描述、编号 {i} 以及补充说明。"
    )
    return content, tag


def import_rows(db: ProbeDB, n: int) -> float:
    ensure_schema(db, reset=True)
    db.set_batch_mode(True)
    start = time.perf_counter()
    for i in range(1, n + 1):
        content, tag = make_entry(i)
        db.execute(
            "INSERT INTO memories (id, source, content, tags, created_at) VALUES ("
            f"{i}, 'bench', {sql_literal(content)}, {sql_literal(tag)}, '2026-10-26T00:00:00')"
        )
    db.set_batch_mode(False)
    db.persist()
    return time.perf_counter() - start


def bench_size(n: int, queries: int) -> dict[str, Any]:
    tmpdir = tempfile.mkdtemp(prefix="probedb_bench_")
    path = os.path.join(tmpdir, f"mem_{n}.pdb")
    result: dict[str, Any] = {"entries": n, "db_path": path}

    db = ProbeDB.open(path)
    result["import_seconds"] = round(import_rows(db, n), 4)
    db.close()

    result["db_bytes"] = os.path.getsize(path)
    wal_path = path + ".wal"
    result["wal_bytes"] = os.path.getsize(wal_path) if os.path.exists(wal_path) else 0
    result["db_kb"] = round(result["db_bytes"] / 1024, 1)

    # 重新打开 = 进程重启后的快照加载 + WAL 重放
    start = time.perf_counter()
    db = ProbeDB.open(path)
    result["reopen_ms"] = round((time.perf_counter() - start) * 1000, 2)

    # 关键词检索：命中的编号 + 一个必然 miss 的查询
    latencies: list[float] = []
    for k in range(queries):
        target = (k * 37) % n + 1
        q = f"记忆条目 {target}"
        start = time.perf_counter()
        hits = search(db, q, limit=10)
        latencies.append((time.perf_counter() - start) * 1000)
        assert hits, f"应命中 {q}"

    start = time.perf_counter()
    assert search(db, "不存在的关键词-xyz-123", limit=10) == []
    miss_ms = (time.perf_counter() - start) * 1000

    result["search_avg_ms"] = round(statistics.mean(latencies), 2)
    result["search_p95_ms"] = round(
        sorted(latencies)[max(0, int(len(latencies) * 0.95) - 1)], 2
    )
    result["search_miss_ms"] = round(miss_ms, 2)
    db.close()

    # CLI 单次调用成本（子进程 + 打开 + 查询 + 关闭）
    cli_times: list[float] = []
    tool = _REPO_ROOT / "tools" / "probedb_mem.py"
    for _ in range(3):
        start = time.perf_counter()
        subprocess.run(
            [sys.executable, str(tool), "stats", "--db", path],
            capture_output=True,
            check=True,
        )
        cli_times.append((time.perf_counter() - start) * 1000)
    result["cli_stats_avg_ms"] = round(statistics.mean(cli_times), 1)

    import shutil

    shutil.rmtree(tmpdir, ignore_errors=True)
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="ProbeDB 记忆场景基准测试")
    parser.add_argument("--sizes", default="100,1000,5000", help="条目数，逗号分隔")
    parser.add_argument("--queries", type=int, default=20, help="每个规模的检索次数")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)

    sizes = [int(s) for s in args.sizes.split(",") if s.strip()]
    results = [bench_size(n, args.queries) for n in sizes]

    if args.json:
        print(json.dumps(results, ensure_ascii=False, indent=2))
        return 0

    print(
        f"{'条目':>8} {'导入(s)':>9} {'DB(KB)':>9} {'重开(ms)':>10} "
        f"{'检索avg(ms)':>12} {'检索p95(ms)':>12} {'CLI(ms)':>9}"
    )
    for r in results:
        print(
            f"{r['entries']:>8} {r['import_seconds']:>9.3f} {r['db_kb']:>9.1f} "
            f"{r['reopen_ms']:>10.2f} {r['search_avg_ms']:>12.2f} "
            f"{r['search_p95_ms']:>12.2f} {r['cli_stats_avg_ms']:>9.1f}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
