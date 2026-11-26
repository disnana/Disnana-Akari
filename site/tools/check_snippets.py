#!/usr/bin/env python3
"""ドキュメント中の Rust コードブロックが、実際にコンパイルできるか確認する。

使い方: python3 site/tools/check_snippets.py [作業ディレクトリ]

`site/content/**/*.md` の ```rust ブロック（直前に `<!-- no-check -->` があるものを除く）を 1 つずつモジュールにして、
Akari に依存する一時クレートで `cargo check` する。
"""
import pathlib
import re
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
CONTENT = ROOT / "site" / "content"
FENCE = re.compile(r"^```(\S*)[^\n]*\n(.*?)^```[ \t]*$", re.S | re.M)


def blocks():
    for path in sorted(CONTENT.rglob("*.md")):
        text = path.read_text(encoding="utf-8")
        for match in FENCE.finditer(text):
            info, body = match.group(1), match.group(2)
            if info != "rust":
                continue
            before = text[: match.start()].rstrip()
            if before.endswith("<!-- no-check -->"):
                continue
            line = text[: match.start()].count("\n") + 1
            yield path.relative_to(ROOT), line, body


def main() -> int:
    work = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else pathlib.Path(tempfile.mkdtemp())
    (work / "src").mkdir(parents=True, exist_ok=True)
    (work / "Cargo.toml").write_text(
        f"""[package]
name = "snippets"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
disnana-akari = {{ path = "{ROOT.as_posix()}" }}
tokio = {{ version = "1", features = ["macros", "rt"] }}
""",
        encoding="utf-8",
    )
    modules, index = [], []
    for number, (path, line, body) in enumerate(blocks()):
        index.append(f"snippet_{number}: {path}:{line}")
        modules.append(f"#[allow(dead_code, unused)]\nmod snippet_{number} {{\n{body}\n}}\n")
    (work / "src" / "lib.rs").write_text("\n".join(modules), encoding="utf-8")
    print(f"{len(modules)} 個のコードブロックを検査します（{work}）")
    result = subprocess.run(["cargo", "check", "--quiet", "--color", "never"], cwd=work, text=True, capture_output=True)
    if result.returncode:
        sys.stderr.write(result.stderr)
        sys.stderr.write("\n".join(index) + "\n")
        return 1
    print("すべてコンパイルできました")
    return 0


if __name__ == "__main__":
    sys.exit(main())
