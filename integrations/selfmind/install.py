#!/usr/bin/env python3
"""把 ProbeDB 后端安装进 SelfMind 仓库（可审查、可回滚）。

做三件事：
1. 复制 `probedb_conn.py` 到 `selfmind_app/`；
2. 在 `unified_store.py` 中注入 `_make_connection()` 工厂，并把两处
   `sqlite3.connect(...)` 改为工厂调用（`SELFMIND_BACKEND=probedb` 时走 ProbeDB）；
3. 原文件写 `.bak` 备份，重复执行幂等。

用法::

    python3 integrations/selfmind/install.py --selfmind-root ~/Documents/selfmind
    python3 integrations/selfmind/install.py --selfmind-root /tmp/copy --dry-run

注意：需要目标仓库写权限；本脚本只改两处连接创建，不碰业务逻辑。
"""

from __future__ import annotations

import argparse
import difflib
import shutil
import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[2]
CONN_SOURCE = _REPO_ROOT / "integrations" / "selfmind" / "probedb_conn.py"

MARKER = "_make_connection"
FACTORY = '''
def _make_connection(db_path):
    """连接工厂：SELFMIND_BACKEND=probedb 时走 ProbeDB，否则保持 SQLite。"""
    backend = os.environ.get("SELFMIND_BACKEND", "sqlite").strip().lower()
    if backend == "probedb":
        try:
            from .probedb_conn import ProbeDBConnection
        except ImportError:  # 以顶层模块方式导入时
            from probedb_conn import ProbeDBConnection
        path = os.environ.get("SELFMIND_PROBEDB_PATH", db_path)
        return ProbeDBConnection(path)
    return sqlite3.connect(db_path, check_same_thread=False)

'''


def patch_source(text: str) -> tuple[str, list[str]]:
    notes: list[str] = []
    if MARKER in text:
        return text, ["already patched"]

    anchor = "from datetime import datetime, timedelta\n"
    if anchor not in text:
        raise RuntimeError("unified_store.py 结构与预期不符：找不到 datetime import")
    text = text.replace(anchor, anchor + FACTORY, 1)
    notes.append("inserted _make_connection factory")

    old_init = "self.conn = sqlite3.connect(db_path, check_same_thread=False)"
    if old_init not in text:
        raise RuntimeError("找不到 UnifiedStore.__init__ 的 sqlite3.connect")
    text = text.replace(old_init, "self.conn = _make_connection(db_path)", 1)
    notes.append("patched UnifiedStore.__init__")

    old_switch = "self.conn = sqlite3.connect(new_db_path, check_same_thread=False)"
    if old_switch in text:
        text = text.replace(old_switch, "self.conn = _make_connection(new_db_path)", 1)
        notes.append("patched switch_db")
    else:
        notes.append("switch_db 未发现 sqlite3.connect（跳过）")
    return text, notes


def install(selfmind_root: Path, dry_run: bool = False) -> int:
    store_path = selfmind_root / "selfmind_app" / "unified_store.py"
    conn_dest = selfmind_root / "selfmind_app" / "probedb_conn.py"
    if not store_path.exists():
        print(f"❌ 找不到 {store_path}", file=sys.stderr)
        return 2
    if not CONN_SOURCE.exists():
        print(f"❌ 找不到 {CONN_SOURCE}", file=sys.stderr)
        return 2

    original = store_path.read_text(encoding="utf-8")
    patched, notes = patch_source(original)

    for note in notes:
        print(f"  - {note}")

    if dry_run:
        diff = difflib.unified_diff(
            original.splitlines(), patched.splitlines(),
            fromfile="selfmind_app/unified_store.py (current)",
            tofile="selfmind_app/unified_store.py (patched)",
            lineterm="",
        )
        print("\n".join(diff))
        print(f"\n[dry-run] 将复制 {CONN_SOURCE.name} → {conn_dest}")
        return 0

    # 备份 + 写入
    backup = store_path.with_suffix(".py.bak")
    if not backup.exists():
        shutil.copy2(store_path, backup)
        print(f"  - 备份: {backup}")

    conn_dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(CONN_SOURCE, conn_dest)
    print(f"  - 复制: {conn_dest}")

    if patched != original:
        store_path.write_text(patched, encoding="utf-8")
        print(f"  - 已修改: {store_path}")
    else:
        print("  - unified_store.py 无需修改")

    print("\n✅ 安装完成。启用方式：")
    print("  SELFMIND_BACKEND=probedb SELFMIND_PROBEDB_PATH=/path/to/selfmind.pdb python3 server.py")
    print("回滚：恢复 selfmind_app/unified_store.py.bak 并删除 selfmind_app/probedb_conn.py")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="把 ProbeDB 后端安装进 SelfMind")
    parser.add_argument("--selfmind-root", type=Path, default=Path("~/Documents/selfmind").expanduser())
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    return install(args.selfmind_root.expanduser(), dry_run=args.dry_run)


if __name__ == "__main__":
    raise SystemExit(main())
