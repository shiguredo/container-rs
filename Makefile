.PHONY: test cover check clippy fmt clean

# 全テストを実行する (統合テストは RUN_CONTAINER_TESTS=1 で有効化する)
test:
	cargo test --all-features

# 全テストをカバレッジ付きで実行する
cover:
	cargo llvm-cov --tests --all-features

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
