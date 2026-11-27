"""直接の依存と、依存の総数が、.github/dependencies.toml の方針に収まっているか調べる。"""

import json
from pathlib import Path
import subprocess
import sys
import tomllib


def main():
    root = Path(__file__).resolve().parents[2]
    policy = tomllib.loads((root / ".github" / "dependencies.toml").read_text(encoding="utf-8"))
    allowed = set(policy["allowed"])

    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"],
        cwd=root, text=True, encoding="utf-8",
    ))
    package = next(p for p in metadata["packages"] if p["name"] == "disnana-akari")
    direct = {dependency["name"] for dependency in package["dependencies"]}

    errors = []
    for name in sorted(direct - allowed):
        errors.append(f"許可されていない依存 `{name}` があります。理由を添えて .github/dependencies.toml に追加してください。")
    for name in sorted(allowed - direct):
        errors.append(f"`{name}` は依存ではなくなりました。.github/dependencies.toml から削除してください。")

    lock = tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))
    total = len(lock["package"]) - 1  # Akari 自身を除く
    limit = policy["max_total_packages"]
    if total > limit:
        errors.append(f"依存クレートの総数が {total} 件で、上限 {limit} 件を超えました。")

    for error in errors:
        print(f"::error::{error}")
    print(f"直接の依存 {len(direct)} 件、総数 {total} 件（上限 {limit} 件）")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
