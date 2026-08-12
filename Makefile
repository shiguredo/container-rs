.PHONY: test cover test-canary check clippy fmt clean

# 全テストを実行する (統合テストは RUN_CONTAINER_TESTS=1 で有効化する)
test:
	cargo test --all-features

# 全テストをカバレッジ付きで実行する
cover:
	cargo llvm-cov --tests --all-features

# canary.py の doctest を実行する (バージョン変換ロジックの回帰検出)
test-canary:
	python3 -m doctest canary.py

# cargo check を実行する
check:
	cargo check --all-features

# cargo clippy を実行する
clippy:
	cargo clippy --all-targets --all-features -- -D warnings

# cargo fmt を実行する
fmt:
	cargo fmt --all

# ビルド成果物を削除する
clean:
	cargo clean
