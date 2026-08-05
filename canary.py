from __future__ import annotations

import argparse
import re
import subprocess
from typing import Optional


# バージョン文字列を受け取り、次の canary バージョンを返す純粋関数。
# ファイル I/O や input() を含まないため、doctest で直接検証できる。
def next_canary_version(version: str) -> str:
    """次の canary バージョンを返す。

    >>> next_canary_version("2026.1.0-canary.3")
    '2026.1.0-canary.4'
    >>> next_canary_version("2026.1.0-canary.99")
    '2026.1.0-canary.100'
    >>> next_canary_version("2026.1.0")
    '2026.2.0-canary.0'
    >>> next_canary_version("abc")
    Traceback (most recent call last):
        ...
    ValueError: Invalid version format: abc
    """
    # -canary.N が含まれる場合は N をインクリメントする
    canary_match = re.fullmatch(r"(\d+\.\d+\.\d+-canary\.)(\d+)", version)
    if canary_match:
        return f"{canary_match.group(1)}{int(canary_match.group(2)) + 1}"

    # -canary.X がない場合、次のマイナーバージョンにして -canary.0 を追加する
    plain_match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", version)
    if plain_match:
        return f"{plain_match.group(1)}.{int(plain_match.group(2)) + 1}.0-canary.0"

    raise ValueError(f"Invalid version format: {version}")


# [package] セクション文字列を受け取り、行頭の `version` キーを次の canary バージョンに
# 更新して (更新後セクション, 現在バージョン, 新バージョン) を返す純粋関数。
# 行頭アンカー (`(?m)^`) で `version` キーに限定するため、`rust-version` の末尾
# `version` に誤マッチしない。マッチの span で値だけを置き換えるため、リテラル一致に
# 依存しない。ファイル I/O や input() を含まないため、doctest で直接検証できる。
def update_package_section(package_content: str) -> tuple[str, str, str]:
    """[package] セクション内の `version` を次の canary に更新する。

    `rust-version` が `version` より前に並んでも、`version` だけが更新されること
    (旧実装は `rust-version` の末尾 `version` に誤マッチして MSRV を書き換えた)。

    >>> update_package_section(
    ...     '[package]\\nname = "x"\\nrust-version = "1.93.0"\\nversion = "2026.1.0-canary.7"'
    ... )
    ('[package]\\nname = "x"\\nrust-version = "1.93.0"\\nversion = "2026.1.0-canary.8"', '2026.1.0-canary.7', '2026.1.0-canary.8')
    """
    version_match = re.search(
        r'(?m)^[ \t]*version\s*=\s*"([\d\.\w-]+)"', package_content
    )
    if not version_match:
        raise ValueError("Version not found in [package] section of Cargo.toml")
    current_version: str = version_match.group(1)
    new_version: str = next_canary_version(current_version)
    value_start, value_end = version_match.span(1)
    updated_package: str = (
        package_content[:value_start] + new_version + package_content[value_end:]
    )
    return updated_package, current_version, new_version


# Cargo.toml 全体文字列を受け取り、[package] セクションと前後に分離する純粋関数。
# 戻り値は (package セクション, 前方, 後方)。後方は [package] 以外の次のセクション
# (例: [dependencies]) 以降。`[package.metadata.*]` は `[package` で始まるため
# セクション区切りにせず package セクションに含める (Cargo.toml では直接キーは
# サブテーブルより前に書かれるため、誤更新の影響は受けない)。
# ファイル I/O や input() を含まないため、doctest で直接検証できる。
def split_package_section(content: str) -> tuple[str, str, str]:
    """Cargo.toml 全体から [package] セクションと前後を分離する。

    後続の [dependencies] などは package セクションに含めないこと。

    >>> split_package_section(
    ...     '[package]\\nversion = "1.0.0"\\n\\n[dependencies]\\ntokio = { version = "1.53" }'
    ... )
    ('[package]\\nversion = "1.0.0"\\n', '', '\\n[dependencies]\\ntokio = { version = "1.53" }')
    """
    package_start = content.find("[package]")
    if package_start == -1:
        raise ValueError("[package] section not found in Cargo.toml")
    next_section = re.search(r"\n\[(?!package)", content[package_start:])
    if next_section:
        package_end = package_start + next_section.start()
        return (
            content[package_start:package_end],
            content[:package_start],
            content[package_end:],
        )
    return content[package_start:], content[:package_start], ""


# ファイルを読み込み、バージョンを更新
def update_version(file_path: str, dry_run: bool) -> Optional[str]:
    with open(file_path, "r", encoding="utf-8") as f:
        content: str = f.read()

    # [package] セクションと前後を分離し、セクション内のバージョンを次の canary に更新する
    package_content, before, after = split_package_section(content)
    updated_package, current_version, new_version = update_package_section(
        package_content
    )
    new_content = before + updated_package + after

    print(f"Current version: {current_version}")
    print(f"New version: {new_version}")

    # dry-run は非対話モードのため、確認プロンプトを挟まず早期 return する
    if dry_run:
        print("Dry-run: Version would be updated to:")
        print(new_content)
        return new_version

    confirmation: str = (
        input("Do you want to update the version? (Y/n): ").strip().lower()
    )

    # (Y/n) 慣例に従い、空入力 / y / yes を Yes として扱い、それ以外はキャンセルとする
    # (.lower() により大文字入力も受理される)
    if confirmation not in ("", "y", "yes"):
        print("Version update canceled.")
        return None

    with open(file_path, "w", encoding="utf-8") as f:
        f.write(new_content)
    print(f"Version updated in Cargo.toml to {new_version}")

    return new_version


# cargo update shiguredo_container を実行
def run_cargo_update(dry_run: bool) -> None:
    if dry_run:
        print("Dry-run: Would run 'cargo update shiguredo_container'")
    else:
        subprocess.run(["cargo", "update", "shiguredo_container"], check=True)
        print("cargo update shiguredo_container executed")


# git add とコミットを実行
def git_commit_version(new_version: str, dry_run: bool) -> None:
    if dry_run:
        print("Dry-run: Would run 'git add Cargo.toml Cargo.lock'")
        print(f"Dry-run: Would run '[canary] Bump version to {new_version}'")
    else:
        subprocess.run(["git", "add", "Cargo.toml", "Cargo.lock"], check=True)
        subprocess.run(
            ["git", "commit", "-m", f"[canary] Bump version to {new_version}"],
            check=True,
        )
        print(f"Version bumped and committed: {new_version}")


# git タグ付けとプッシュを実行
def git_tag_and_push(new_version: str, dry_run: bool) -> None:
    if dry_run:
        print(f"Dry-run: Would run 'git tag {new_version}'")
        print("Dry-run: Would run 'git push'")
        print(f"Dry-run: Would run 'git push origin {new_version}'")
    else:
        subprocess.run(["git", "tag", new_version], check=True)
        subprocess.run(["git", "push"], check=True)
        subprocess.run(["git", "push", "origin", new_version], check=True)


# メイン処理
def main() -> None:
    parser = argparse.ArgumentParser(
        description="Update Cargo.toml version and commit changes."
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Run in dry-run mode without making actual changes",
    )
    args = parser.parse_args()

    cargo_toml_path: str = "Cargo.toml"

    # バージョン更新
    new_version: Optional[str] = update_version(cargo_toml_path, args.dry_run)

    if not new_version:
        return  # ユーザーが確認をキャンセルした場合、処理を中断

    # cargo update shiguredo_container を実行
    run_cargo_update(args.dry_run)

    # バージョン更新後に git commit
    git_commit_version(new_version, args.dry_run)

    # git タグ付け、プッシュ
    git_tag_and_push(new_version, args.dry_run)


if __name__ == "__main__":
    main()
