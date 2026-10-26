#!/usr/bin/env python3
"""probedb_mem — Hermes 记忆的独立 ProbeDB 存储工具（方案A 第一版）。

不修改 hermes-agent 内部存储，先把真实的 ~/.hermes/memories 数据导入
ProbeDB，验证"导入 → 持久化 → 重启 → 检索"全链路。Hermes 侧通过
CLI/JSON 调用即可接入。

用法::

    # 导入默认记忆文件（MEMORY.md + USER.md）
    python3 tools/probedb_mem.py import

    # 搜索
    python3 tools/probedb_mem.py search "ProbeDB"
    python3 tools/probedb_mem.py search "模型偏好" --json

    # 查看 / 统计 / 追加
    python3 tools/probedb_mem.py list --limit 20
    python3 tools/probedb_mem.py stats
    python3 tools/probedb_mem.py add "新记忆内容" --tag 项目 --source manual

数据库默认在 ``~/.probedb/hermes_memory.pdb``（可用 --db 或环境变量
PROBEDB_MEMORY_DB 覆盖）。
"""

from __future__ import annotations

import argparse
import fcntl
import json
import os
import sys
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Iterable, Optional

# 允许直接从仓库运行，无需安装
_REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(_REPO_ROOT / "python"))

from probedb import ProbeDB, ProbeDBError  # noqa: E402

DEFAULT_DB = Path(
    os.environ.get("PROBEDB_MEMORY_DB", "~/.probedb/hermes_memory.pdb")
).expanduser()

DEFAULT_SOURCES = [
    Path("~/.hermes/memories/MEMORY.md").expanduser(),
    Path("~/.hermes/memories/USER.md").expanduser(),
]

SCHEMA_SQL = (
    "CREATE TABLE memories "
    "(id INTEGER, source TEXT, content TEXT, tags TEXT, created_at TEXT)"
)


# ---------------------------------------------------------------------------
# 基础工具
# ---------------------------------------------------------------------------


def sql_literal(value: str) -> str:
    """把 Python 字符串变成 SQL 字符串字面量（SQL 标准 '' 转义）。

    ProbeDB 解析器已支持字符串内的分号/括号/换行与 '' 转义。
    """
    if "\x00" in value:
        raise ValueError("内容包含 NUL 字节，ProbeDB 不支持")
    return "'" + value.replace("'", "''") + "'"


def now_iso() -> str:
    return datetime.now().isoformat(timespec="seconds")


def ensure_schema(db: ProbeDB, reset: bool = False) -> None:
    if reset:
        try:
            db.execute("DROP TABLE memories")
        except ProbeDBError:
            pass  # 表不存在
    try:
        db.execute("SELECT id FROM memories LIMIT 0")
    except ProbeDBError:
        db.execute(SCHEMA_SQL)


def next_id(db: ProbeDB) -> int:
    result = db.execute("SELECT id FROM memories ORDER BY id DESC LIMIT 1")
    if not result.rows:
        return 1
    return int(result.rows[0][0]) + 1


# ---------------------------------------------------------------------------
# 导入
# ---------------------------------------------------------------------------


@dataclass
class MemoryEntry:
    source: str
    content: str
    tags: str
    created_at: str


def parse_memory_text(text: str, source: str) -> list[MemoryEntry]:
    """解析 Hermes 记忆文件：条目以单独一行 ``§`` 分隔，行内多行用空格拼接。"""
    blocks: list[list[str]] = []
    current: list[str] = []
    for line in text.splitlines():
        if line.strip() == "§":
            if current:
                blocks.append(current)
                current = []
        else:
            stripped = line.strip()
            if stripped:
                current.append(stripped)
    if current:
        blocks.append(current)

    entries: list[MemoryEntry] = []
    created_at = now_iso()
    for block in blocks:
        content = " ".join(block)
        if not content:
            continue
        tags = ""
        if content.startswith("["):
            end = content.find("]")
            if end > 1:
                tags = content[1:end].strip()
        entries.append(
            MemoryEntry(source=source, content=content, tags=tags, created_at=created_at)
        )
    return entries


def import_files(db: ProbeDB, paths: Iterable[Path], reset: bool = True) -> int:
    """导入一个或多个记忆文件，返回导入条数。"""
    paths = list(paths)
    entries: list[MemoryEntry] = []
    for path in paths:
        path = Path(path).expanduser()
        if not path.exists():
            print(f"⚠️  跳过不存在的文件: {path}", file=sys.stderr)
            continue
        entries.extend(parse_memory_text(path.read_text(encoding="utf-8"), path.stem))

    ensure_schema(db, reset=reset)
    if not entries:
        db.persist()
        return 0

    db.set_batch_mode(True)
    base_id = next_id(db)
    for offset, entry in enumerate(entries):
        db.execute(
            "INSERT INTO memories (id, source, content, tags, created_at) VALUES ("
            f"{base_id + offset}, {sql_literal(entry.source)}, "
            f"{sql_literal(entry.content)}, {sql_literal(entry.tags)}, "
            f"{sql_literal(entry.created_at)})"
        )
    db.set_batch_mode(False)  # 关闭时自动 flush
    db.persist()
    return len(entries)


# ---------------------------------------------------------------------------
# 查询 / 写入
# ---------------------------------------------------------------------------


def fetch_all(db: ProbeDB) -> list[dict[str, Any]]:
    try:
        result = db.execute(
            "SELECT id, source, content, tags, created_at FROM memories ORDER BY id ASC"
        )
    except ProbeDBError as e:
        # 空库/尚未导入：表不存在视为空集合，其余错误照常抛出
        if "不存在" in str(e):
            return []
        raise
    return [
        {
            "id": int(row[0]),
            "source": row[1],
            "content": row[2],
            "tags": row[3],
            "created_at": row[4],
        }
        for row in result.rows
    ]


def search(db: ProbeDB, query: str, limit: int = 10) -> list[dict[str, Any]]:
    """关键词检索（标签命中优先，再按内容命中次数与 id 倒序）。"""
    q = query.strip().lower()
    if not q:
        return []
    hits: list[tuple[int, dict[str, Any]]] = []
    for entry in fetch_all(db):
        content_l = entry["content"].lower()
        tags_l = entry["tags"].lower()
        if q not in content_l and q not in tags_l:
            continue
        score = 0
        if tags_l == q:
            score += 10
        elif q in tags_l:
            score += 5
        score += content_l.count(q)
        hits.append((score, entry))
    hits.sort(key=lambda item: (-item[0], -item[1]["id"]))
    return [entry for _, entry in hits[:limit]]


def list_entries(
    db: ProbeDB, limit: int = 20, source: Optional[str] = None
) -> list[dict[str, Any]]:
    entries = fetch_all(db)
    if source:
        entries = [e for e in entries if e["source"] == source]
    return entries[-limit:][::-1]


def add_entry(db: ProbeDB, text: str, tags: str = "", source: str = "manual") -> int:
    ensure_schema(db)
    entry_id = next_id(db)
    db.execute(
        "INSERT INTO memories (id, source, content, tags, created_at) VALUES ("
        f"{entry_id}, {sql_literal(source)}, {sql_literal(text)}, "
        f"{sql_literal(tags)}, {sql_literal(now_iso())})"
    )
    db.persist()
    return entry_id


def stats(db: ProbeDB) -> dict[str, Any]:
    ensure_schema(db)
    entries = fetch_all(db)
    by_source: dict[str, int] = {}
    tags: set[str] = set()
    for e in entries:
        by_source[e["source"]] = by_source.get(e["source"], 0) + 1
        if e["tags"]:
            tags.add(e["tags"])
    return {
        "total": len(entries),
        "by_source": by_source,
        "tags": sorted(tags),
        "database": str(db_path_for_display(db)),
    }


def db_path_for_display(db: ProbeDB) -> str:
    return getattr(db, "_display_path", "<memory>")


# ---------------------------------------------------------------------------
# 文件锁 + CLI
# ---------------------------------------------------------------------------


@contextmanager
def locked_db(path: Path, display_path: Optional[str] = None):
    """独占文件锁 + 打开数据库，避免多个 CLI 进程并发丢更新。"""
    path = Path(path).expanduser()
    path.parent.mkdir(parents=True, exist_ok=True)
    lock_path = path.with_suffix(path.suffix + ".lock")
    with open(lock_path, "w") as lock_file:
        fcntl.flock(lock_file, fcntl.LOCK_EX)
        db = ProbeDB.open(str(path))
        db._display_path = display_path or str(path)
        try:
            yield db
        finally:
            db.close()
            fcntl.flock(lock_file, fcntl.LOCK_UN)


def _print_entries(entries: list[dict[str, Any]]) -> None:
    if not entries:
        print("（无结果）")
        return
    for e in entries:
        tag = f" [{e['tags']}]" if e["tags"] else ""
        content = e["content"].replace("\n", " ")
        # 标签已在前面展示，去掉正文里重复的 [tag] 前缀
        if e["tags"] and content.startswith(f"[{e['tags']}]"):
            content = content[len(e["tags"]) + 2 :].lstrip()
        print(f"{e['id']:>4}  ({e['source']}){tag}  {content}")


def _print_json(payload: Any) -> None:
    print(json.dumps(payload, ensure_ascii=False, indent=2))


def build_parser() -> argparse.ArgumentParser:
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument(
        "--db",
        default=str(DEFAULT_DB),
        help=f"数据库路径（默认 {DEFAULT_DB}）",
    )

    parser = argparse.ArgumentParser(
        prog="probedb_mem",
        description="Hermes 记忆的 ProbeDB 独立存储工具",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p_import = sub.add_parser("import", parents=[common], help="导入 Hermes 记忆文件")
    p_import.add_argument(
        "files",
        nargs="*",
        type=Path,
        default=DEFAULT_SOURCES,
        help="记忆文件（默认 ~/.hermes/memories/MEMORY.md 与 USER.md）",
    )
    p_import.add_argument("--append", action="store_true", help="追加导入（默认先清空）")

    p_search = sub.add_parser("search", parents=[common], help="关键词检索")
    p_search.add_argument("query")
    p_search.add_argument("--limit", type=int, default=10)
    p_search.add_argument("--json", action="store_true")

    p_list = sub.add_parser("list", parents=[common], help="列出记忆")
    p_list.add_argument("--limit", type=int, default=20)
    p_list.add_argument("--source")
    p_list.add_argument("--json", action="store_true")

    p_add = sub.add_parser("add", parents=[common], help="追加一条记忆")
    p_add.add_argument("text")
    p_add.add_argument("--tag", default="")
    p_add.add_argument("--source", default="manual")

    p_stats = sub.add_parser("stats", parents=[common], help="统计信息")
    p_stats.add_argument("--json", action="store_true")

    return parser


def main(argv: Optional[list[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    db_path = Path(args.db)

    if args.command == "import":
        files = args.files or DEFAULT_SOURCES
        with locked_db(db_path) as db:
            count = import_files(db, files, reset=not args.append)
        print(f"✅ 导入 {count} 条记忆 → {db_path}")
        return 0

    if args.command == "search":
        with locked_db(db_path) as db:
            entries = search(db, args.query, args.limit)
        if args.json:
            _print_json(entries)
        else:
            print(f"🔍 搜索「{args.query}」命中 {len(entries)} 条：")
            _print_entries(entries)
        return 0

    if args.command == "list":
        with locked_db(db_path) as db:
            entries = list_entries(db, args.limit, args.source)
        if args.json:
            _print_json(entries)
        else:
            _print_entries(entries)
        return 0

    if args.command == "add":
        with locked_db(db_path) as db:
            entry_id = add_entry(db, args.text, tags=args.tag, source=args.source)
        print(f"✅ 已写入记忆 id={entry_id} → {db_path}")
        return 0

    if args.command == "stats":
        with locked_db(db_path) as db:
            info = stats(db)
        if args.json:
            _print_json(info)
        else:
            print(f"数据库: {db_path}")
            print(f"记忆总数: {info['total']}")
            for source, count in sorted(info["by_source"].items()):
                print(f"  - {source}: {count}")
            if info["tags"]:
                print(f"标签({len(info['tags'])}): {', '.join(info['tags'])}")
        return 0

    return 1


if __name__ == "__main__":
    raise SystemExit(main())
