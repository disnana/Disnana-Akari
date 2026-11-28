#!/usr/bin/env python3
"""ドキュメント中の Rust コードブロックが、実際にコンパイルできるか確認する。

使い方: python3 site/tools/check_snippets.py [作業ディレクトリ]

`site/content/**/*.md`、`README.md`、`DESIGN.md`、`docs/*.md` の ```rust ブロック（直前に `<!-- no-check -->` があるものを除く）を
1 つずつモジュールにして、
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


def documents():
    """検査するドキュメント。サイトのガイドと、リポジトリ直下の文書。"""
    yield from sorted(CONTENT.rglob("*.md"))
    for name in ("README.md", "DESIGN.md"):
        if (ROOT / name).exists():
            yield ROOT / name
    yield from sorted((ROOT / "docs").glob("*.md"))


def blocks():
    for path in documents():
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


def missing_intents() -> list[str]:
    """テキストコマンドやメッセージを待つ例が、Intent を指定しているか確認する。

    Akari は Intent を自動で足さないので、指定がない例は、コンパイルできても、起動時にエラーになる。
    """
    problems = []
    for path in documents():
        text = path.read_text(encoding="utf-8")
        for match in FENCE.finditer(text):
            if match.group(1) != "rust":
                continue
            body = match.group(2)
            waits_for_messages = re.search(r"\.run_commands\(|\.on_message\(|\.run\(\s*\w+\s*\)", body)
            if waits_for_messages and not re.search(r"add_intents|with_intents", body):
                line = text[: match.start()].count("\n") + 1
                problems.append(
                    f"{path.relative_to(ROOT)}:{line}: メッセージを待つ例に、Intent の指定（add_intents / with_intents）がありません"
                )
    return problems


def undocumented_exports() -> list[str]:
    """クレート直下の公開名が、どの文書にも出てこないものを探す。"""
    lib = (ROOT / "src" / "lib.rs").read_text(encoding="utf-8")
    names = set()
    for match in re.finditer(r"^pub use ([^;]+);", lib, re.M | re.S):
        body = match.group(1)
        if "{" in body:
            body = body[body.index("{") + 1 : body.rindex("}")]
        for item in body.split(","):
            item = item.strip().split(" as ")[-1].strip()
            if item and re.fullmatch(r"[A-Z][A-Za-z0-9]*", item):
                names.add(item)
    corpus = "\n".join(p.read_text(encoding="utf-8") for p in documents())
    return [
        f"公開型 {name} がどの文書にも出てきません。docs/api.md などで説明してください"
        for name in sorted(names)
        if not re.search(rf"\b{name}\b", corpus)
    ]


def stale_version_pins() -> list[str]:
    """ドキュメントの TOML と cargo add の版指定を Cargo.toml と照合する。

    0.x では、`"0.1"` は 0.2 以降を含まない。古い指定のままだと、読者は古い版を取得してしまう。
    """
    import tomllib

    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    major, minor, *_ = version.split(".")
    expected = f"{major}.{minor}" if major == "0" else major
    problems = []
    pins = re.compile(r'^disnana-akari[ \t]*=[ \t]*"([^"\n]+)"|^cargo add disnana-akari@([^\s]+)', re.M)
    for path in documents():
        text = path.read_text(encoding="utf-8")
        for match in pins.finditer(text):
            pin = match.group(1) or match.group(2)
            if pin != expected:
                line = text[: match.start()].count("\n") + 1
                problems.append(
                    f"{path.relative_to(ROOT)}:{line}: disnana-akari の版指定 \"{pin}\" は、現在の版 {version} と合いません（\"{expected}\" にしてください）"
                )
    return problems


def main() -> int:
    problems = stale_version_pins() + missing_intents() + undocumented_exports()
    if problems:
        sys.stderr.write("\n".join(problems) + "\n")
        return 1
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
