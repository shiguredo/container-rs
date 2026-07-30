import argparse
import re
import subprocess
from typing import Optional


# バージョン文字列を受け取り、次の canary バージョンを返す純粋関数。
# ファイル I/O や input() を含まないため、単体テストで直接検証できる。
def next_canary_version(version: str) -> str:
    # -canary.N が含まれる場合は N をインクリメントする
    canary_match = re.fullmatch(r"(\d+\.\d+\.\d+-canary\.)(\d+)", version)
    if canary_match:
        return f"{canary_match.group(1)}{int(canary_match.group(2)) + 1}"

    # -canary.X がない場合、次のマイナーバージョンにして -canary.0 を追加する
    plain_match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", version)
    if plain_match:
        return f"{plain_match.group(1)}.{int(plain_match.group(2)) + 1}.0-canary.0"

    raise ValueError(f"Invalid version format: {version}")


# ファイルを読み込み、バージョンを更新
def update_version(file_path: str, dry_run: bool) -> Optional[str]:
    with open(file_path, "r", encoding="utf-8") as f:
        content: str = f.read()

    # [package] セクション内のバージョンのみを取得
    package_section_match = re.search(
        r'\[package\].*?version\s*=\s*"([\d\.\w-]+)"', content, re.DOTALL
    )
    if not package_section_match:
        raise ValueError("Version not found in [package] section of Cargo.toml")

    current_version: str = package_section_match.group(1)
    new_version: str = next_canary_version(current_version)

    # [package] セクションの開始位置を見つける
    package_start = content.find("[package]")
    # 次のセクション ([dependencies] など) の開始位置を見つける
    next_section = re.search(r"\n\[(?!package)", content[package_start:])
    if next_section:
        package_end = package_start + next_section.start()
        package_content = content[package_start:package_end]
    else:
        package_content = content[package_start:]

    # [package] セクション内の旧バージョン文字列を新バージョン文字列に置換する。
    # 抽出正規表現は version\s*=\s*"..." と空白に寛容だが、置換はリテラル一致のため、
    # 置換後に実際に変更が起きたかを検証する
    old_version_literal: str = f'version = "{current_version}"'
    new_version_literal: str = f'version = "{new_version}"'
    updated_package: str = package_content.replace(
        old_version_literal, new_version_literal, 1
    )
    if updated_package == package_content:
        raise ValueError(
            f"Failed to replace version in [package] section: "
            f"expected '{old_version_literal}' not found"
        )

    # 元のコンテンツの [package] セクション部分を更新後の内容に置き換える
    if next_section:
        new_content = content[:package_start] + updated_package + content[package_end:]
    else:
        new_content = content[:package_start] + updated_package

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
