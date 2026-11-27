"""Generate Japanese release notes for the commits between published versions."""

import json
import os
from pathlib import Path
import subprocess
from urllib.parse import quote


# 0.1.0 was published manually. Its crates.io archive records this commit.
FIRST_RELEASE_COMMIT = "7a0d0aee4f4c8292ee77aef70d5596982455ea47"


def api_pages(path):
    return json.loads(subprocess.check_output(
        ["gh", "api", "--paginate", "--slurp", path], text=True, encoding="utf-8"
    ))


def main():
    repo = os.environ["GITHUB_REPOSITORY"]
    tag = os.environ["GITHUB_REF_NAME"]
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    previous = subprocess.run(
        ["git", "describe", "--first-parent", "--tags", "--abbrev=0", "--match", "v[0-9]*", f"{head}^"],
        capture_output=True, text=True,
    )
    base = previous.stdout.strip() if previous.returncode == 0 else FIRST_RELEASE_COMMIT
    subprocess.run(["git", "merge-base", "--is-ancestor", base, head], check=True)

    version = tag.removeprefix("v")
    root = f"https://github.com/{repo}"
    compare = f"{quote(base, safe='')}...{quote(tag, safe='')}"
    changelog = Path("CHANGELOG.md").read_text(encoding="utf-8")
    summary = changelog.split(f"\n## {version}\n", 1)[1].split("\n## ", 1)[0].strip()
    print(f"{summary}\n")
    print(f"[crates.io](https://crates.io/crates/disnana-akari/{quote(version, safe='')})"
          f" · [最新リリース]({root}/releases/latest) · [変更全体]({root}/compare/{compare})\n")

    commits = [commit for page in api_pages(f"repos/{repo}/compare/{quote(base, safe='')}...{head}?per_page=100")
               for commit in page["commits"]]
    authors = {}
    pull_requests = {}
    print(f"<details>\n<summary>変更コミット（{len(commits)}件）</summary>\n")
    for commit in commits:
        sha = commit["sha"]
        subject = commit["commit"]["message"].splitlines()[0]
        print(f"- [`{sha[:7]}`]({root}/commit/{sha}) {subject}")
        author = commit.get("author")
        if author:
            login = author["login"]
            authors[login] = f"[@{login}](https://github.com/{login})"
        else:
            name = commit["commit"]["author"]["name"]
            authors[name] = name
        for page in api_pages(f"repos/{repo}/commits/{sha}/pulls?per_page=100"):
            for pr in page:
                if pr["merged_at"] and pr["base"]["repo"]["full_name"].casefold() == repo.casefold():
                    pull_requests[pr["number"]] = pr

    print("\n</details>\n")
    print(f"<details>\n<summary>マージされた PR（{len(pull_requests)}件）</summary>\n")
    if pull_requests:
        for number, pr in sorted(pull_requests.items()):
            print(f"- [#{number}]({root}/pull/{number}) {pr['title']}")
            if pr.get("user"):
                login = pr["user"]["login"]
                authors[login] = f"[@{login}](https://github.com/{login})"
    else:
        print("対象期間にマージされた PR はありません。")

    print("\n</details>\n")
    print("## コントリビューター\n")
    for name in sorted(authors, key=str.casefold):
        print(f"- {authors[name]}")

    crate = f"disnana-akari-{version}.crate"
    print("\n## 配布ファイルの検証\n")
    print("配布アーカイブに SHA-256 チェックサムと署名付き来歴証明を添付しています。\n")
    print(f"```sh\nsha256sum -c SHA256SUMS\n"
          f"gh attestation verify {crate} --repo {repo} --bundle {crate}.sigstore.json\n```")


if __name__ == "__main__":
    main()
