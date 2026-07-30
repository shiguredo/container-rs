# canary.py のバージョン変換ロジック (next_canary_version) の単体テスト。
# canary.py は pyproject.toml も uv 環境も持たない単発のリリーススクリプトのため、
# pytest ではなく標準ライブラリの unittest で書く。
# 実行方法: python3 -m unittest test_canary
from __future__ import annotations

import unittest

from canary import next_canary_version


class TestNextCanaryVersion(unittest.TestCase):
    # canary バージョンのインクリメント: N が 1 増えること
    def test_canary_increment(self) -> None:
        self.assertEqual(next_canary_version("2026.1.0-canary.3"), "2026.1.0-canary.4")

    # canary インクリメント: N が 0 の場合も正しく 1 になること
    def test_canary_increment_from_zero(self) -> None:
        self.assertEqual(next_canary_version("2026.1.0-canary.0"), "2026.1.0-canary.1")

    # canary インクリメント: 2 桁の N も正しくインクリメントされること
    def test_canary_increment_two_digits(self) -> None:
        self.assertEqual(
            next_canary_version("2026.1.0-canary.99"), "2026.1.0-canary.100"
        )

    # 通常バージョンからの変換: マイナーバージョンが 1 増え、パッチが 0 リセットされ、
    # -canary.0 が付与されること
    def test_plain_version_to_canary(self) -> None:
        self.assertEqual(next_canary_version("2026.1.0"), "2026.2.0-canary.0")

    # 通常バージョンからの変換: マイナーバージョンが 9 の場合も正しく 10 になること
    def test_plain_version_minor_nine(self) -> None:
        self.assertEqual(next_canary_version("2026.9.5"), "2026.10.0-canary.0")

    # 不正形式の入力は ValueError を投げること
    def test_invalid_version_raises_value_error(self) -> None:
        with self.assertRaises(ValueError):
            next_canary_version("abc")

    # 空文字列は ValueError を投げること
    def test_empty_string_raises_value_error(self) -> None:
        with self.assertRaises(ValueError):
            next_canary_version("")

    # セマンティックバージョンニング形式 (4 要素) は ValueError を投げること
    def test_four_part_version_raises_value_error(self) -> None:
        with self.assertRaises(ValueError):
            next_canary_version("1.2.3.4")


if __name__ == "__main__":
    unittest.main()
