fn main() {
    println!("cargo:rerun-if-changed=src/xpc_bridge.c");
    println!("cargo:rerun-if-changed=src/xpc_bridge.h");
    // docs.rs は Linux ホスト上で macOS ターゲットのドキュメントをビルドするため、
    // macOS SDK を要する C コンパイルは通らない。rustdoc はリンクを行わないので
    // ネイティブライブラリが無くてもドキュメント生成には支障がない。
    if std::env::var("DOCS_RS").is_ok() {
        return;
    }
    // cfg!(target_os) はビルドスクリプトを実行するホスト OS を見てしまうため、
    // クロスコンパイルでも正しく判定できる CARGO_CFG_TARGET_OS を使う。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/xpc_bridge.c")
            .flag("-fblocks")
            .compile("xpc_bridge");
    }
}
