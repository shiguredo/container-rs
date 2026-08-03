mod helpers;

/// ホストからコンテナへのネットワーク接続に依存するテストをスキップする。
///
/// macOS 15+ の Local Network Privacy は、許可されていないユーザープロセスから
/// ローカルネットワークへの接続をブロックし得る。self-hosted CI の診断では
/// published port フォワーダー経由は失敗し、コンテナ IP 直結は成功する。
/// そのため直結テストは CI では本ゲートを使わず実行し、フォワーダー / HttpWait
/// 依存テストは本ゲートでスキップする。ローカルでは LNP 許可済み環境で
/// `RUN_HOST_NETWORK_TESTS=1` を指定した時のみ実行する。
#[cfg(target_os = "macos")]
fn skip_unless_host_network() -> bool {
    if std::env::var("RUN_HOST_NETWORK_TESTS").as_deref() == Ok("1") {
        return false;
    }
    eprintln!(
        "スキップ: ホストからコンテナへの接続は macOS Local Network Privacy に依存する。\
         許可済みホストでは RUN_HOST_NETWORK_TESTS=1 を指定すること"
    );
    true
}

/// amd64 (Rosetta) コンテナに依存するテストをスキップする。
///
/// `with_platform("linux/amd64")` は Virtualization.framework の Linux 向け Rosetta を
/// 要求する。未導入のホストでは bootstrap が
/// `VZErrorDomain ... Rosetta is not installed` で失敗する。
/// self-hosted CI にも Rosetta が無い場合があるため、ランタイム有無で判定する。
#[cfg(target_os = "macos")]
fn skip_unless_rosetta() -> bool {
    let installed =
        std::path::Path::new("/Library/Apple/usr/libexec/oah/libRosettaRuntime").exists();
    if !installed {
        eprintln!(
            "スキップ: Rosetta runtime が未導入。 \
             amd64 コンテナには softwareupdate --install-rosetta が必要"
        );
        true
    } else {
        false
    }
}

#[cfg(target_os = "macos")]
mod test_container_macos {
    use shiguredo_container::{
        GenericImage, ImageExt, core::ExecCommand, core::image::ContainerState,
        runners::AsyncRunner,
    };

    /// 空いているホストポートを探す。
    ///
    /// listener を drop してからコンテナが bind するまでの TOCTOU がある。
    /// 明示マッピング (`with_mapped_port`) の往復検証専用。状態 snapshot 等は
    /// `with_exposed_port` の自動割当を使うこと。
    pub(super) fn find_free_port() -> u16 {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("空きポートの bind に失敗した");
        listener
            .local_addr()
            .expect("空きポートのローカルアドレス取得に失敗した")
            .port()
    }

    /// `container list --all --quiet` の出力に対象 ID が行完全一致で含まれるか。
    fn container_id_listed(id: &str) -> bool {
        let out = std::process::Command::new("container")
            .args(["list", "--all", "--quiet"])
            .output()
            .expect("container list の実行に失敗した");
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines().any(|line| line.trim() == id)
    }

    /// `uname -m` を exec して終了コード 0 と非空の stdout を得られること。
    #[tokio::test]
    async fn alpine_exec_uname() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let mut result = container
            .exec(ExecCommand::new(["uname", "-m"]))
            .await
            .expect("uname -m の exec に失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0),
            "uname -m の終了コードが 0 であること"
        );
        let stdout = result
            .stdout_to_vec()
            .await
            .expect("stdout の取得に失敗した");
        let stdout = String::from_utf8_lossy(&stdout);
        let arch = stdout.trim();
        assert!(
            !arch.is_empty(),
            "uname -m の stdout が空でないこと: {stdout}"
        );
        assert!(
            arch == "aarch64" || arch == "x86_64" || arch == "arm64",
            "既知のアーキテクチャ名であること: {arch}"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    /// with_hostname で指定した値がゲスト内の hostname コマンド結果と一致すること。
    #[tokio::test]
    async fn alpine_with_hostname() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let host = format!("xpc-hn-{}", std::process::id());
        let container = GenericImage::new("alpine", "latest")
            .with_hostname(&host)
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let mut result = container
            .exec(ExecCommand::new(["hostname"]))
            .await
            .expect("hostname の exec に失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0),
            "hostname の終了コードが 0 であること"
        );
        let stdout = result
            .stdout_to_vec()
            .await
            .expect("stdout の取得に失敗した");
        assert_eq!(
            String::from_utf8_lossy(&stdout).trim(),
            host.as_str(),
            "ゲスト内 hostname が指定値と一致すること"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    /// with_hostname と with_container_name を異なる値で指定したとき独立して効くこと。
    #[tokio::test]
    async fn alpine_hostname_independent_of_container_name() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let name = format!("xpc-name-{}", std::process::id());
        let host = format!("xpc-host-{}", std::process::id());
        let container = GenericImage::new("alpine", "latest")
            .with_container_name(&name)
            .with_hostname(&host)
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        assert_eq!(
            container.id(),
            name.as_str(),
            "コンテナ ID は container_name であること"
        );

        let mut result = container
            .exec(ExecCommand::new(["hostname"]))
            .await
            .expect("hostname の exec に失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0),
            "hostname の終了コードが 0 であること"
        );
        let stdout = result
            .stdout_to_vec()
            .await
            .expect("stdout の取得に失敗した");
        assert_eq!(
            String::from_utf8_lossy(&stdout).trim(),
            host.as_str(),
            "ゲスト内 hostname は with_hostname の値であること"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    /// with_readonly_rootfs(true) でルート FS への書き込みが失敗すること。
    #[tokio::test]
    async fn alpine_with_readonly_rootfs() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_readonly_rootfs(true)
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let result = container
            .exec(ExecCommand::new(["touch", "/readonly_probe"]))
            .await
            .expect("touch の exec に失敗した");
        let code = result
            .exit_code()
            .await
            .expect("終了コードの取得に失敗した");
        assert_ne!(
            code,
            Some(0),
            "読み取り専用ルートへの touch は失敗すること (exit={code:?})"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    /// `with_readonly_paths` で指定したパスへの書き込みが失敗すること。
    #[tokio::test]
    async fn xpc_readonly_paths_blocks_write() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_readonly_paths(["/etc"])
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let result = container
            .exec(ExecCommand::new(["touch", "/etc/readonly_probe"]))
            .await
            .expect("touch の exec に失敗した");
        let code = result
            .exit_code()
            .await
            .expect("終了コードの取得に失敗した");
        assert_ne!(
            code,
            Some(0),
            "readonlyPaths 指定パスへの touch は失敗すること (exit={code:?})"
        );

        // control: 指定外パスへの書き込みは成功する (readonlyPaths が原因であることの対比)。
        let result = container
            .exec(ExecCommand::new(["touch", "/tmp/readonly_probe"]))
            .await
            .expect("touch の exec に失敗した");
        let code = result
            .exit_code()
            .await
            .expect("終了コードの取得に失敗した");
        assert_eq!(
            code,
            Some(0),
            "readonlyPaths 外のパスへの touch は成功すること (exit={code:?})"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    /// `with_masked_paths` の空リストで既定マスクが解除されること。
    ///
    /// 既定では OCI のマスク対象 (`/proc/timer_list` 等) が null デバイスへの
    /// 文字デバイスに置き換えられる。空リスト指定でマスクが解除されると
    /// 通常ファイルになり、文字デバイス判定 (`-c`) が外れる (Apple container 1.2.0)。
    #[tokio::test]
    async fn xpc_masked_paths_empty_disables_default_mask() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // control: 既定 (マスクあり) では /proc/timer_list が文字デバイス (null) に置き換わること。
        let control = GenericImage::new("alpine", "latest")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("control alpine コンテナの起動に失敗した");
        let result = control
            .exec(ExecCommand::new(["sh", "-c", "[ -c /proc/timer_list ]"]))
            .await
            .expect("control プローブの exec に失敗した");
        let code = result
            .exit_code()
            .await
            .expect("終了コードの取得に失敗した");
        assert_eq!(
            code,
            Some(0),
            "既定マスクでは /proc/timer_list が文字デバイスであること (exit={code:?})"
        );
        control
            .stop_with_timeout(Some(0))
            .await
            .expect("control コンテナの停止に失敗した");
        control
            .rm()
            .await
            .expect("control コンテナの削除に失敗した");

        // 空リスト指定でマスク解除 → 通常ファイルになり文字デバイス判定が外れること。
        let container = GenericImage::new("alpine", "latest")
            .with_masked_paths(std::iter::empty::<String>())
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");
        let result = container
            .exec(ExecCommand::new(["sh", "-c", "[ -f /proc/timer_list ]"]))
            .await
            .expect("プローブの exec に失敗した");
        let code = result
            .exit_code()
            .await
            .expect("終了コードの取得に失敗した");
        assert_eq!(
            code,
            Some(0),
            "マスク解除で /proc/timer_list が通常ファイルになること (exit={code:?})"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    /// `with_masked_paths` の明示リストが既定マスクを完全に上書きすること。
    ///
    /// 明示リストは既定セットへの追加ではなく置換であるため、既定マスク対象
    /// (`/proc/timer_list`) のマスクが外れる (Apple container 1.2.0)。
    #[tokio::test]
    async fn xpc_masked_paths_explicit_list_overrides_default_mask() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_masked_paths(["/tmp/sensitive"])
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let result = container
            .exec(ExecCommand::new(["sh", "-c", "[ -f /proc/timer_list ]"]))
            .await
            .expect("プローブの exec に失敗した");
        let code = result
            .exit_code()
            .await
            .expect("終了コードの取得に失敗した");
        assert_eq!(
            code,
            Some(0),
            "明示リストで既定マスクが上書きされ /proc/timer_list が通常ファイルになること (exit={code:?})"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    /// with_open_stdin(true) で stdin 待ちの cat がすぐ終了せず running のままであること。
    #[tokio::test]
    async fn alpine_with_open_stdin_keeps_cat_running() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_open_stdin(true)
            .with_cmd(["cat"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        assert!(
            container
                .is_running()
                .await
                .expect("is_running の取得に失敗した"),
            "open_stdin(true) なら cat は stdin EOF で即終了しないこと"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    #[tokio::test]
    async fn alpine_exec_printenv() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // with_env_var で設定した環境変数が exec 経由の printenv から見えること。
        // 旧実装は ProcCfg.environment を空配列固定で送っていたため見えなかった。
        let container = GenericImage::new("alpine", "latest")
            .with_env_var("SHIGURE_TEST", "xpc_value_42")
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let mut result = container
            .exec(ExecCommand::new(["printenv", "SHIGURE_TEST"]))
            .await
            .expect("printenv の実行に失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0)
        );
        let stdout = result
            .stdout_to_vec()
            .await
            .expect("標準出力の取得に失敗した");
        assert_eq!(String::from_utf8_lossy(&stdout).trim(), "xpc_value_42");

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    /// ExecCommand::with_env_vars の追加 env がコンテナ env にマージされ、同名は上書きされること。
    #[tokio::test]
    async fn alpine_exec_with_env_vars() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_env_var("BASE", "from_container")
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let mut result = container
            .exec(
                ExecCommand::new(["sh", "-c", "printenv BASE; printenv EXEC_ONLY"])
                    .with_env_vars([("EXEC_ONLY", "from_exec"), ("BASE", "overridden")]),
            )
            .await
            .expect("printenv の exec に失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0),
            "printenv の終了コードが 0 であること"
        );
        let stdout = result
            .stdout_to_vec()
            .await
            .expect("stdout の取得に失敗した");
        let stdout = String::from_utf8_lossy(&stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(
            lines,
            ["overridden", "from_exec"],
            "BASE が上書きされ EXEC_ONLY が見えること: {stdout}"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    /// `with_container_name` で指定した名前がコンテナ ID としてランタイムに反映されること。
    #[tokio::test]
    async fn alpine_with_container_name_listed() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let name = format!("xpc-test-hostname-{}", std::process::id());
        let container = GenericImage::new("alpine", "latest")
            .with_container_name(&name)
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        assert_eq!(
            container.id(),
            name,
            "ラッパーの id が with_container_name の値と一致すること"
        );
        assert!(
            container_id_listed(&name),
            "container list に指定名の ID が存在すること: {name}"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    #[tokio::test]
    async fn stop_and_remove() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "60"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    /// ContainerState::from_container から実行中コンテナの状態を取得できること。
    #[tokio::test]
    async fn container_state_from_container() {
        if super::helpers::skip_if_ci() {
            return;
        }
        let container = GenericImage::new("alpine", "latest")
            .with_exposed_port(80_u16.into())
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let state = ContainerState::from_container(&container)
            .await
            .expect("コンテナ状態の取得に失敗した");
        let host = container.get_host().await.expect("ホストの取得に失敗した");
        assert_eq!(state.host(), &host);
        let host_port = container
            .get_host_port_ipv4(80_u16)
            .await
            .expect("コンテナのポート 80 マッピングが取得できなかった");
        assert_eq!(
            state
                .host_port_ipv4(80_u16.into())
                .expect("状態のポート 80 マッピングが取得できなかった"),
            host_port
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    #[tokio::test]
    async fn alpine_mapped_port_and_bridge_ip() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let host_port = find_free_port();
        let container = GenericImage::new("alpine", "latest")
            .with_mapped_port(host_port, 80_u16.into())
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        // ホスト側ポートが設定通りに取得できること。
        let ports = container.ports().await.expect("ポート情報の取得に失敗した");
        eprintln!("コンテナから取得したポート情報: {ports:?}");
        let actual_host_port = ports
            .map_to_host_port_ipv4(80_u16)
            .expect("ポート 80 が公開されていない");
        assert_eq!(
            actual_host_port, host_port,
            "マップされたホストポートが一致すること"
        );

        // ブリッジ IP アドレスが取得できること。
        let bridge_ip = container
            .get_bridge_ip_address()
            .await
            .expect("ブリッジ IP アドレスの取得に失敗した");
        eprintln!("ブリッジ IP アドレス: {bridge_ip}");

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }
}

#[cfg(target_os = "macos")]
mod test_container_xpc {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use shiguredo_container::{
        GenericImage, ImageExt,
        core::{
            ExecCommand, ExtraHost, WaitFor,
            copy::CopyTargetOptions,
            error::{WaitContainerError, WaitLogError},
            logs::LogFrame,
        },
        runners::AsyncRunner,
    };
    use tokio::io::AsyncBufReadExt;

    /// 現在プロセスが開いている FD の数を数える。
    ///
    /// LogConsumer 配信タスクの dup FD 解放の検証に使う。`/dev/fd` はプロセス自身の
    /// FD 一覧なので、読んでいる最中の /dev/fd 自身も数える (相対比較のみに使う)。
    /// テストハーネスは同一プロセスで並列実行されるため、他のテストがコンテナを
    /// start / teardown すると FD 数が増減し、この検証は誤判定し得る
    /// (誤失敗: 他テストの start が FD を開く / 誤成功: 他テストの teardown が FD を閉じる)。
    /// 単独実行 (例: `cargo test xpc_alpine_log_consumer_stops_after_natural_exit`) で
    /// 正確に検証できる。
    fn open_fd_count() -> usize {
        std::fs::read_dir("/dev/fd")
            .expect("FD 一覧の取得に失敗した (検証を無言で無効化しないこと)")
            .count()
    }

    #[test]
    fn xpc_module_loads() {
        if super::helpers::skip_if_ci() {
            return;
        }
        eprintln!("XPC ランタイムモジュールを SIGABRT なしで読み込んだ");
    }

    #[tokio::test]
    async fn xpc_alpine_copy_file_from() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        // Vec<u8> にコピーできること。
        let bytes = container
            .copy_file_from("/etc/os-release", Vec::new())
            .await
            .expect("Vec<u8> へのファイルコピーに失敗した");
        let content = String::from_utf8_lossy(&bytes);
        assert!(
            content.contains("Alpine Linux"),
            "/etc/os-release の内容が想定外: {content}"
        );

        // PathBuf にコピーできること。
        let temp_path = std::env::temp_dir().join(format!(
            "container-rs-copy-file-from-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // 本家 0.27 と同じく PathBuf 宛のコピーは () を返す。
        container
            .copy_file_from("/etc/os-release", temp_path.clone())
            .await
            .expect("PathBuf へのファイルコピーに失敗した");
        let file_content = tokio::fs::read_to_string(&temp_path)
            .await
            .expect("コピー済みファイルの読み取りに失敗した");
        assert!(
            file_content.contains("Alpine Linux"),
            "コピー済みファイルの内容が想定外: {file_content}"
        );
        let _ = tokio::fs::remove_file(&temp_path).await;

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    /// ディレクトリ指定時は `is a directory` を含むエラーになり、一時パスが残留しないこと。
    ///
    /// apple/container の `containerCopyOut` はディレクトリコピーを成功させる
    /// (`ContainerClient.copyOut` / `testCopyOutDirectoryToExistingDirectory`)。
    /// 本 API はファイル専用のため、コピー成功後に `IsDirectory` で拒否する。
    /// source の第一候補は `/etc` (alpine に常に存在するディレクトリ)。
    #[tokio::test]
    async fn xpc_copy_file_from_directory_is_rejected_without_temp_leftover() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let temp_dir = std::env::temp_dir();
        let list_copy_out_temps = || -> std::collections::HashSet<std::path::PathBuf> {
            std::fs::read_dir(&temp_dir)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("container-rs-copy-out-"))
                })
                .collect()
        };

        let before = list_copy_out_temps();

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let err = container
            .copy_file_from("/etc", Vec::new())
            .await
            .expect_err("ディレクトリ指定はエラーになること");
        let msg = err.to_string();
        assert!(
            msg.contains("is a directory"),
            "エラー文面に is a directory が含まれること: {msg}"
        );

        let after = list_copy_out_temps();
        let added: Vec<_> = after.difference(&before).collect();
        assert!(
            added.is_empty(),
            "copy_file_from 失敗後に container-rs-copy-out-* が増えていないこと: {added:?}"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    /// with_platform("linux/amd64") で amd64 (Rosetta) コンテナが起動し、uname -m が x86_64 になること。
    #[tokio::test]
    async fn xpc_alpine_with_platform_amd64_uname() {
        if super::helpers::skip_if_ci() || super::skip_unless_rosetta() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_platform("linux/amd64")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("linux/amd64 指定の alpine コンテナ起動に失敗した");

        let result = container
            .exec(ExecCommand::new(["uname", "-m"]))
            .await
            .expect("uname -m の実行に失敗した");
        let mut result = result;
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0),
            "uname -m の終了コードが 0 であること"
        );
        let stdout = result
            .stdout_to_vec()
            .await
            .expect("標準出力の読み取りに失敗した");
        let stdout = String::from_utf8_lossy(&stdout);
        assert!(
            stdout.contains("x86_64"),
            "amd64 プラットフォームのコンテナは x86_64 を報告すること: {stdout}"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_with_wait_for_seconds() {
        if super::helpers::skip_if_ci() {
            return;
        }
        let container = GenericImage::new("alpine", "latest")
            .with_wait_for(WaitFor::seconds(2))
            .with_cmd(["sleep", "1"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");
        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_with_ready_conditions_override() {
        if super::helpers::skip_if_ci() {
            return;
        }
        let start = std::time::Instant::now();
        let container = GenericImage::new("alpine", "latest")
            .with_wait_for(WaitFor::seconds(20))
            .with_ready_conditions(vec![WaitFor::seconds(1)])
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "with_ready_conditions は with_wait_for を上書きすること, 経過時間: {elapsed:?}"
        );
        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_with_privileged() {
        if super::helpers::skip_if_ci() {
            return;
        }
        let container = GenericImage::new("alpine", "latest")
            .with_privileged(true)
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        // loopback インターフェースを down / up する操作は CAP_NET_ADMIN を必要とする。
        // with_privileged(true) は XPC 側で capAdd: ["ALL"] として渡される。
        let result = container
            .exec(ExecCommand::new(["ip", "link", "set", "lo", "down"]))
            .await
            .expect("ip link set lo down の実行に失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0),
            "privileged コンテナは ip link set lo down を許可すること"
        );
        let result = container
            .exec(ExecCommand::new(["ip", "link", "set", "lo", "up"]))
            .await
            .expect("ip link set lo up の実行に失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0),
            "privileged コンテナは ip link set lo up を許可すること"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_with_user() {
        if super::helpers::skip_if_ci() {
            return;
        }
        // 現状の Apple container 実行環境では非 root UID が動作しないため、
        // コードパスが壊れていないことを確認する観点で root (0:0) を指定する。
        let container = GenericImage::new("alpine", "latest")
            .with_user("0:0")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let result = container
            .exec(ExecCommand::new(["sh", "-c", "[ $(id -u) -eq 0 ]"]))
            .await
            .expect("UID 確認コマンドの実行に失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0),
            "コンテナプロセスは uid=0 で実行されること"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_with_host() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_host(
                "api.internal",
                ExtraHost::Addr("10.0.0.5".parse().expect("処理に失敗しないこと")),
            )
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let result = container
            .exec(ExecCommand::new(["getent", "hosts", "api.internal"]))
            .await
            .expect("getent の実行に失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0),
            "with_host のエントリは /etc/hosts 経由で名前解決されること"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_with_copy_to() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let src = std::env::temp_dir().join(format!(
            "shiguredo_container_test_copy_to_{}.txt",
            std::process::id()
        ));
        std::fs::write(&src, b"hello from host").expect("一時ファイルの書き込みに失敗した");

        let container = GenericImage::new("alpine", "latest")
            .with_copy_to("/data/hello.txt", src.clone())
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let result = container
            .exec(ExecCommand::new([
                "sh",
                "-c",
                "[ \"$(cat /data/hello.txt)\" = \"hello from host\" ]",
            ]))
            .await
            .expect("cat の実行に失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0),
            "コピーしたファイルがコンテナ内で読み取り可能であること"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
        let _ = std::fs::remove_file(&src);
    }

    /// ホストディレクトリを `with_copy_to` したときの XPC 挙動を実測する。
    #[tokio::test]
    async fn xpc_alpine_with_copy_to_directory() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let host_dir = std::env::temp_dir().join(format!(
            "shiguredo_container_test_copy_dir_{}",
            std::process::id()
        ));
        let nested = host_dir.join("nested");
        std::fs::create_dir_all(&nested).expect("ホスト一時ディレクトリの作成に失敗した");
        std::fs::write(host_dir.join("root.txt"), b"root from host\n")
            .expect("root.txt の書き込みに失敗した");
        std::fs::write(nested.join("child.txt"), b"child from host\n")
            .expect("child.txt の書き込みに失敗した");

        let start = GenericImage::new("alpine", "latest")
            .with_copy_to("/data/fixture", host_dir.clone())
            .with_cmd(["sleep", "30"])
            .start()
            .await;

        match start {
            Ok(container) => {
                let root_ok = container
                    .exec(ExecCommand::new([
                        "sh",
                        "-c",
                        "[ \"$(cat /data/fixture/root.txt)\" = \"root from host\" ]",
                    ]))
                    .await
                    .expect("root.txt 確認の exec に失敗した");
                let child_ok = container
                    .exec(ExecCommand::new([
                        "sh",
                        "-c",
                        "[ \"$(cat /data/fixture/nested/child.txt)\" = \"child from host\" ]",
                    ]))
                    .await
                    .expect("child.txt 確認の exec に失敗した");
                assert_eq!(
                    root_ok
                        .exit_code()
                        .await
                        .expect("root exit code の取得に失敗した"),
                    Some(0),
                    "ディレクトリ投入後に root.txt が読めること"
                );
                assert_eq!(
                    child_ok
                        .exit_code()
                        .await
                        .expect("child exit code の取得に失敗した"),
                    Some(0),
                    "ディレクトリ投入後に nested/child.txt が読めること"
                );
                container.stop_with_timeout(Some(0)).await.ok();
                container.rm().await.ok();
            }
            Err(e) => {
                panic!(
                    "macOS ディレクトリ投入が失敗した (Apple container 制約として文書化する根拠): {e}"
                );
            }
        }

        let _ = std::fs::remove_dir_all(&host_dir);
    }

    /// `CopyDataSource::Data` の内容が一時ファイル経由でコンテナにコピーされること。
    #[tokio::test]
    async fn xpc_alpine_with_copy_to_data() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_copy_to("/data/from-data.txt", b"hello from data".to_vec())
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("Data コピー付き alpine コンテナの起動に失敗した");

        let result = container
            .exec(ExecCommand::new([
                "sh",
                "-c",
                "[ \"$(cat /data/from-data.txt)\" = \"hello from data\" ]",
            ]))
            .await
            .expect("コピー済みデータの確認コマンドに失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("確認コマンドの終了コード取得に失敗した"),
            Some(0),
            "Data ソースの内容がコンテナ内で読めること"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    /// bind mount したホストファイルをコンテナ内から読めること。
    #[tokio::test]
    async fn xpc_alpine_with_bind_mount() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let source = std::env::temp_dir().join(format!(
            "shiguredo_container_mount_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&source, b"mounted content").expect("マウント元ファイルの作成に失敗した");

        let container = GenericImage::new("alpine", "latest")
            .with_mount(shiguredo_container::core::Mount::bind_mount(
                source.to_string_lossy(),
                "/data/mounted.txt",
            ))
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("bind mount 付き alpine コンテナの起動に失敗した");

        let result = container
            .exec(ExecCommand::new([
                "sh",
                "-c",
                "[ \"$(cat /data/mounted.txt)\" = \"mounted content\" ]",
            ]))
            .await
            .expect("マウント済みファイルの確認コマンドに失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("確認コマンドの終了コード取得に失敗した"),
            Some(0),
            "bind mount したファイルの内容がコンテナ内で読めること"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
        std::fs::remove_file(&source).expect("マウント元ファイルの削除に失敗した");
    }

    /// tmpfs の size / mode がコンテナ内に反映されること。
    #[tokio::test]
    async fn xpc_alpine_with_tmpfs_size_and_mode() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // 64 MiB / mode 1777 の tmpfs。df の 1K-blocks と stat の permission で確認する。
        let container = GenericImage::new("alpine", "latest")
            .with_mount(
                shiguredo_container::core::Mount::tmpfs_mount("/mnt/tmpfs")
                    .with_size_bytes(64 * 1024 * 1024)
                    .with_mode(0o1777),
            )
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("tmpfs 付き alpine コンテナの起動に失敗した");

        let result = container
            .exec(ExecCommand::new([
                "sh",
                "-c",
                // df -k の 2 列目が 1K-blocks。64MiB = 65536。多少の差を許容して 60000〜70000。
                // mode は sticky + rwxrwxrwx で 1777。
                "size_k=$(df -k /mnt/tmpfs | awk 'NR==2 {print $2}'); \
                 mode=$(stat -c %a /mnt/tmpfs); \
                 [ \"$mode\" = \"1777\" ] && [ \"$size_k\" -ge 60000 ] && [ \"$size_k\" -le 70000 ]",
            ]))
            .await
            .expect("tmpfs 確認コマンドに失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("確認コマンドの終了コード取得に失敗した"),
            Some(0),
            "tmpfs の size / mode がコンテナ内に反映されること"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    /// volume マウントしたボリュームがコンテナ内に現れ、読み書きできること。
    #[tokio::test]
    async fn xpc_alpine_with_volume_mount() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // Apple のボリューム名制約 (英数字始まり・255 文字以内) に適合するユニーク名。
        // ユニーク名のため並行実行と衝突しない。panic で残った volume は同一名に
        // ならないため掃除されず蓄積するが、次回実行には影響しない (名前が変わるため)。
        let volume_name = format!(
            "container_vol_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        );
        let _ = std::process::Command::new("container")
            .args(["volume", "rm", &volume_name])
            .output();

        let container = GenericImage::new("alpine", "latest")
            .with_mount(shiguredo_container::core::Mount::volume_mount(
                &volume_name,
                "/data",
            ))
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("volume マウント付き alpine コンテナの起動に失敗した");

        // df の出力にマウントポイントが含まれ、書き込んだファイルが読めること。
        let result = container
            .exec(ExecCommand::new([
                "sh",
                "-c",
                "df | grep -q ' /data$' && \
                 echo volume-probe > /data/probe.txt && \
                 [ \"$(cat /data/probe.txt)\" = \"volume-probe\" ]",
            ]))
            .await
            .expect("volume マウント確認コマンドに失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("確認コマンドの終了コード取得に失敗した"),
            Some(0),
            "volume が df に現れ、読み書きできること"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");

        // 名前付きボリュームはコンテナ削除後も残るため CLI で削除する。
        let rm_volume = std::process::Command::new("container")
            .args(["volume", "rm", &volume_name])
            .output()
            .expect("container volume rm の実行に失敗した");
        assert!(
            rm_volume.status.success(),
            "volume の削除に失敗した: {}",
            String::from_utf8_lossy(&rm_volume.stderr)
        );
    }

    /// 既に存在するボリュームをマウントして起動できること。
    #[tokio::test]
    async fn xpc_alpine_with_existing_volume_mount() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // 既存ボリュームへの再マウントは volumeCreate の already exists エラーを
        // 契機に volumeInspect へフォールバックする経路で、この経路を実機で固定する。
        // 事前に CLI で作成してから library (XPC) 経由でマウントする。
        // ユニーク名のため並行実行と衝突しない。panic で残った volume は同一名に
        // ならないため掃除されず蓄積するが、次回実行には影響しない。
        let volume_name = format!(
            "container_vol_existing_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        );
        let _ = std::process::Command::new("container")
            .args(["volume", "rm", &volume_name])
            .output();
        let create = std::process::Command::new("container")
            .args(["volume", "create", &volume_name])
            .output()
            .expect("container volume create の実行に失敗した");
        assert!(
            create.status.success(),
            "volume の事前作成に失敗した: {}",
            String::from_utf8_lossy(&create.stderr)
        );

        let container = GenericImage::new("alpine", "latest")
            .with_mount(shiguredo_container::core::Mount::volume_mount(
                &volume_name,
                "/data",
            ))
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("既存 volume マウント付き alpine コンテナの起動に失敗した");

        // 既存ボリュームがコンテナ内に現れること。
        let result = container
            .exec(ExecCommand::new(["sh", "-c", "df | grep -q ' /data$'"]))
            .await
            .expect("volume マウント確認コマンドに失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("確認コマンドの終了コード取得に失敗した"),
            Some(0),
            "既存 volume が df に現れること"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");

        let rm_volume = std::process::Command::new("container")
            .args(["volume", "rm", &volume_name])
            .output()
            .expect("container volume rm の実行に失敗した");
        assert!(
            rm_volume.status.success(),
            "volume の削除に失敗した: {}",
            String::from_utf8_lossy(&rm_volume.stderr)
        );
    }

    /// CopyTargetOptions でファイルモードを指定してコピーできること。
    #[tokio::test]
    async fn xpc_alpine_with_copy_to_target_options() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let src = std::env::temp_dir().join(format!(
            "shiguredo_container_test_copy_to_opts_{}.txt",
            std::process::id()
        ));
        std::fs::write(&src, b"hello with mode").expect("一時ファイルの書き込みに失敗した");

        let target = CopyTargetOptions::new("/data/mode-hello.txt").with_mode(0o755);
        let container = GenericImage::new("alpine", "latest")
            .with_copy_to(target, src.clone())
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let result = container
            .exec(ExecCommand::new([
                "sh",
                "-c",
                "[ \"$(cat /data/mode-hello.txt)\" = \"hello with mode\" ] && [ \"$(stat -c %a /data/mode-hello.txt)\" = \"755\" ]",
            ]))
            .await
            .expect("exec に失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0),
            "コピーしたファイルの内容とモードが一致すること"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
        let _ = std::fs::remove_file(&src);
    }

    /// copy_in 失敗時に Data ソースの一時ファイルが `$TMPDIR` に残留しないこと。
    ///
    /// 失敗誘発は apple/container の統合テストに合わせる:
    /// - 第一候補: 末尾 `/` 付きの未存在ディレクトリ
    ///   (`TestCLICopyCommand.testCopyInFileToNonExistingTrailingSlashFails` /
    ///   `destination directory does not exist`)
    /// - フォールバック: 親がファイルのパス (`/etc/passwd/child` → ENOTDIR)
    ///
    /// 注意: `/etc` のような既存ディレクトリは失敗しない。
    /// ファイル → 既存ディレクトリはディレクトリ内へコピーされる
    /// (`testCopyInFileToExistingDirectory`)。
    #[tokio::test]
    async fn xpc_copy_to_data_temp_cleaned_on_copy_in_failure() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // 並列テストとの誤検知を避けるため、内容で一意に特定するマーカーを使う。
        let marker = format!(
            "shiguredo_copy_temp_probe_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let data = marker.clone().into_bytes();

        let temp_dir = std::env::temp_dir();
        let list_marker_temps = || -> Vec<std::path::PathBuf> {
            let Ok(entries) = std::fs::read_dir(&temp_dir) else {
                return Vec::new();
            };
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                        n.starts_with("shiguredo_container_copy_") && n.ends_with(".bin")
                    })
                })
                .filter(|p| std::fs::read(p).ok().as_deref() == Some(data.as_slice()))
                .collect()
        };

        let destinations: &[&str] = &["/tmp/shiguredo_copy_fail_nonexistent/", "/etc/passwd/child"];
        let mut failed_dest: Option<&str> = None;

        for dest in destinations {
            assert!(
                list_marker_temps().is_empty(),
                "開始前にマーカー付き一時ファイルが存在しないこと"
            );

            let result = GenericImage::new("alpine", "latest")
                .with_copy_to(*dest, data.clone())
                .with_cmd(["sleep", "30"])
                .start()
                .await;

            match result {
                Err(_) => {
                    failed_dest = Some(*dest);
                    assert!(
                        list_marker_temps().is_empty(),
                        "copy_in 失敗後にマーカー付き一時ファイルが増えていないこと (destination={dest})"
                    );
                    break;
                }
                Ok(container) => {
                    // 想定外の成功。後始末して次の destination を試す。
                    container.stop_with_timeout(Some(0)).await.ok();
                    container.rm().await.ok();
                    assert!(
                        list_marker_temps().is_empty(),
                        "成功パスでもマーカー付き一時ファイルが残留しないこと"
                    );
                }
            }
        }

        assert!(
            failed_dest.is_some(),
            "copy_in が失敗する destination が見つからなかった \
             (未存在末尾 / と /etc/passwd/child を試した)"
        );
        eprintln!(
            "copy_in 失敗誘発に使った destination: {}",
            failed_dest.expect("上で is_some を確認済み")
        );
    }

    #[tokio::test]
    async fn xpc_alpine_is_running() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        assert!(
            container
                .is_running()
                .await
                .expect("実行状態の取得に失敗した"),
            "コンテナは実行中であること"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        assert!(
            !container
                .is_running()
                .await
                .expect("実行状態の取得に失敗した"),
            "コンテナは停止していること"
        );

        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_exit_code() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sh", "-c", "exit 42"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut code = None;
        while std::time::Instant::now() < deadline {
            if let Some(c) = container
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した")
            {
                code = Some(c);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(code, Some(42), "終了コードは 42 であること");

        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_running_exit_code_none() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        assert_eq!(
            container
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            None,
            "実行中のコンテナに終了コードがないこと"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    /// SIGTERM を無視する init に対し、グレース 60 秒超の停止が誤エラーにならないこと。
    #[tokio::test]
    async fn xpc_alpine_stop_with_timeout_grace_ignores_sigterm() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // このテストは SIGTERM 経路のシナリオ検証であり、helpers の後始末規約
        // (停止は stop_with_timeout(Some(0))) の例外。SIGTERM → グレース経過 → SIGKILL の
        // 一連の停止を検証するため、停止にグレース値 (61 秒) の時間がかかる。
        // グレース経過 + SIGKILL による停止完了 (61 秒 + α) が XPC 送信タイムアウト
        // (61 + 30 = 91 秒) 以内に収まることを確認するのが目的。
        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sh", "-c", "trap '' TERM; exec tail -f /dev/null"])
            .start()
            .await
            .expect("SIGTERM 無視の alpine コンテナの起動に失敗した");

        let started = std::time::Instant::now();
        container
            .stop_with_timeout(Some(61))
            .await
            .expect("グレース 61 秒の停止がエラーにならないこと");
        let elapsed = started.elapsed();

        // グレース 61 秒の経過を待ってから SIGKILL で止まること。
        // trap '' が効かず SIGTERM で即死する誤実装を検出するため、経過時間も検証する。
        assert!(
            elapsed.as_secs() >= 61,
            "グレース 61 秒経過後に SIGKILL で停止すること (elapsed={elapsed:?})"
        );
        assert!(
            !container
                .is_running()
                .await
                .expect("is_running の取得に失敗した"),
            "停止後にコンテナが実行中でないこと"
        );

        container.rm().await.expect("コンテナの削除に失敗した");
    }

    #[tokio::test]
    async fn xpc_alpine_stop_with_timeout_immediate() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "60"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("timeout 0 での停止に失敗した");
        assert!(
            !container
                .is_running()
                .await
                .expect("実行状態の取得に失敗した"),
            "コンテナは SIGKILL で即座に停止すること"
        );

        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_stdout_stderr_echo() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd([
                "sh",
                "-c",
                "echo STDOUT_MSG; echo STDERR_MSG >> /dev/stderr",
            ])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let stdout = container
            .stdout_to_vec()
            .await
            .expect("標準出力の取得に失敗した");
        let stdout_text = String::from_utf8_lossy(&stdout);
        assert!(
            stdout_text.contains("STDOUT_MSG"),
            "stdout に 'STDOUT_MSG' が含まれること: {stdout_text}"
        );

        let stderr = container
            .stderr_to_vec()
            .await
            .expect("標準エラー出力の取得に失敗した");
        let stderr_text = String::from_utf8_lossy(&stderr);
        // Apple container は init プロセスの stderr に vminitd のログが流れる。
        assert!(
            stderr_text.contains("setting up relay for StandardIO stderr"),
            "stderr に vminitd のログが含まれること: {stderr_text}"
        );

        container.rm().await.ok();
    }

    /// async stdout(follow=true) が追記行を読むこと。
    #[tokio::test]
    async fn xpc_alpine_stdout_follow_reads_appended_lines() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd([
                "sh",
                "-c",
                "echo FOLLOW_ASYNC_FIRST; sleep 2; echo FOLLOW_ASYNC_SECOND; sleep 30",
            ])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let mut lines = container.stdout(true).lines();
        let saw_second = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let mut saw_first = false;
            while let Some(line) = lines
                .next_line()
                .await
                .expect("follow stdout の行読み取りに失敗した")
            {
                if line.contains("FOLLOW_ASYNC_FIRST") {
                    saw_first = true;
                }
                if line.contains("FOLLOW_ASYNC_SECOND") {
                    return saw_first;
                }
            }
            false
        })
        .await
        .expect("follow stdout が timeout した");

        assert!(
            saw_second,
            "FOLLOW_ASYNC_SECOND の前に FOLLOW_ASYNC_FIRST を読んでいること"
        );

        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_exec_stdout_stderr() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let result = container
            .exec(ExecCommand::new([
                "sh",
                "-c",
                "echo STDOUT_MSG; echo STDERR_MSG >> /dev/stderr",
            ]))
            .await
            .expect("exec の実行に失敗した");

        let mut result = result;
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0),
            "exec が成功すること"
        );
        let stdout = result
            .stdout_to_vec()
            .await
            .expect("標準出力の読み取りに失敗した");
        let stdout = String::from_utf8_lossy(&stdout);
        assert!(
            stdout.contains("STDOUT_MSG"),
            "stdout に 'STDOUT_MSG' が含まれること: {stdout}"
        );
        let stderr = result
            .stderr_to_vec()
            .await
            .expect("標準エラー出力の読み取りに失敗した");
        let stderr = String::from_utf8_lossy(&stderr);
        assert!(
            stderr.contains("STDERR_MSG"),
            "stderr に 'STDERR_MSG' が含まれること: {stderr}"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_log_wait_strategy() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sh", "-c", "echo READY; sleep 30"])
            .with_ready_conditions(vec![WaitFor::message_on_stdout("READY")])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_log_consumer() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let logs = Arc::new(Mutex::new(Vec::new()));
        let logs_clone = logs.clone();
        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sh", "-c", "echo CONSUMER_READY; sleep 30"])
            .with_log_consumer(move |record: &LogFrame| {
                logs_clone
                    .lock()
                    .expect("処理に失敗しないこと")
                    .push((record.source(), record.bytes().to_vec()));
            })
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        // ログ配信タスクが行を受け取るまで待つ。
        tokio::time::sleep(Duration::from_secs(1)).await;

        {
            let captured = logs.lock().expect("処理に失敗しないこと");
            assert!(
                captured.iter().any(|(_, bytes)| {
                    String::from_utf8_lossy(bytes).contains("CONSUMER_READY")
                }),
                "log consumer が 'CONSUMER_READY' を受け取ること: {captured:?}"
            );
        }

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    /// コンテナ自然終了後に LogConsumer 配信タスクが停止し、dup FD が解放されること。
    #[tokio::test]
    async fn xpc_alpine_log_consumer_stops_after_natural_exit() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let marker = "log-consumer-exit-marker";
        let cmd = format!("echo {marker}; exit 0");
        let logs = Arc::new(Mutex::new(Vec::new()));
        let logs_clone = logs.clone();
        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sh", "-c", &cmd])
            .with_log_consumer(move |record: &LogFrame| {
                logs_clone
                    .lock()
                    .expect("処理に失敗しないこと")
                    .push((record.source(), record.bytes().to_vec()));
            })
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        // 配信タスクが spawn 済みの時点の FD 数を記録する。
        // 以降の検証はこの値を基準に「タスクの dup FD が解放された」ことを確認する。
        // タスクは stdout / stderr で各 1 dup の計 2 FD を持つため、基準から 2 減る。
        // コンテナ終了時には wait スレッドの XPC 接続 FD も 1 つ閉じるため、
        // 単独実行でもタスク break なしでは「2 減」に届かない (誤成功しない)。
        let fd_after_start = open_fd_count();

        // マーカーが配信されるまで待つ (ポーリング。固定待ちにしない)。
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let delivered = logs
                .lock()
                .expect("処理に失敗しないこと")
                .iter()
                .any(|(_, bytes)| String::from_utf8_lossy(bytes).contains(marker));
            if delivered {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "マーカーが配信されること"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // 自然終了 (exit code 記録) 後、猶予期間を過ぎると配信タスクが停止し、
        // dup FD が解放されて FD 数が spawn 時より減ることを確認する。
        // 停止は「EOF 観測 + exit code 初観測から 2 秒」で、wait_blocking 応答遅延と
        // EOF 観測周期 (最大 100ms) の分だけ遅れ得る。
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let fd_now = open_fd_count();
            if fd_now <= fd_after_start.saturating_sub(2) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "配信タスクの dup FD が解放されること (after_start={fd_after_start}, now={fd_now})"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_with_network() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // テスト用ネットワークを CLI で作成する (XPC にネットワーク作成 API を持たないため)。
        // 既に存在する場合は create が失敗するが、そのまま利用できるので無視する。
        let net_name = "shiguredo-container-rs-test-net";
        let _ = std::process::Command::new("container")
            .args(["network", "create", net_name])
            .output()
            .expect("container network create の実行に失敗した");

        let container = GenericImage::new("alpine", "latest")
            .with_network(net_name)
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("カスタムネットワーク上の alpine コンテナ起動に失敗した");

        // ブリッジ IP がテスト用ネットワークのサブネットに属すること。
        // Apple container のサブネットは /24 固定なので先頭 3 オクテットを比較する。
        let bridge_ip = container
            .get_bridge_ip_address()
            .await
            .expect("ブリッジ IP アドレスの取得に失敗した");
        let ls_output = std::process::Command::new("container")
            .args(["network", "ls"])
            .output()
            .expect("container network ls の実行に失敗した");
        let ls_text = String::from_utf8_lossy(&ls_output.stdout).to_string();
        let subnet = ls_text
            .lines()
            .find(|line| line.starts_with(net_name))
            .and_then(|line| line.split_whitespace().nth(1))
            .expect("テスト用ネットワークが network ls に現れること")
            .to_string();
        let subnet_prefix = subnet
            .rsplit_once('.')
            .map(|(prefix, _)| format!("{prefix}."))
            .expect("サブネットがドット区切り IPv4 形式であること");
        assert!(
            bridge_ip.to_string().starts_with(&subnet_prefix),
            "ブリッジ IP {bridge_ip} はテスト用ネットワークのサブネット {subnet} に属すること"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();

        // ネットワークを削除する。コンテナ削除が非同期に完了するため少し待つ。
        tokio::time::sleep(Duration::from_secs(1)).await;
        let _ = std::process::Command::new("container")
            .args(["network", "delete", net_name])
            .output();
    }

    #[tokio::test]
    async fn xpc_alpine_exec_large_output() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // exec の出力がパイプバッファ (64KB) を超えてもデッドロックしないこと。
        // 旧実装は containerWait を先に待つため、プロセスが write でブロックして
        // 永遠に終了しないデッドロックになっていた。
        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        let result = tokio::time::timeout(
            Duration::from_secs(30),
            container.exec(ExecCommand::new([
                "sh",
                "-c",
                "head -c 1000000 /dev/zero | base64",
            ])),
        )
        .await
        .expect("大量出力を伴う exec がデッドロックしないこと")
        .expect("exec が成功すること");

        let mut result = result;
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            Some(0),
            "exec の終了コードが 0 であること"
        );
        let stdout = result
            .stdout_to_vec()
            .await
            .expect("標準出力の読み取りに失敗した");
        assert!(
            stdout.len() > 1_000_000,
            "stdout に全出力が含まれること: {} バイト",
            stdout.len()
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[test]
    fn xpc_alpine_drop_outside_runtime_does_not_panic() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // ランタイム外で ContainerAsync を drop してもパニックせず、
        // 同期削除フォールバックでコンテナが削除されること。
        // 旧実装は Drop 内の tokio::spawn が "no reactor running" でパニックしていた。
        let rt = tokio::runtime::Runtime::new().expect("ランタイムの作成に失敗した");
        let container = rt.block_on(async {
            GenericImage::new("alpine", "latest")
                .with_cmd(["tail", "-f", "/dev/null"])
                .start()
                .await
                .expect("alpine コンテナの起動に失敗した")
        });
        let id = container.id().to_string();
        rt.block_on(async {
            container.stop_with_timeout(Some(0)).await.ok();
        });
        drop(rt);

        drop(container);

        // 同期削除フォールバックが実行され、コンテナが消えていること。
        let ls = std::process::Command::new("container")
            .args(["ls", "-a"])
            .output()
            .expect("container ls の実行に失敗した");
        let ls_text = String::from_utf8_lossy(&ls.stdout).to_string();
        assert!(
            !ls_text.contains(&id),
            "drop 時にコンテナが削除されること: {id}"
        );
    }

    /// Runtime 内 Drop でコンテナが削除されること。
    ///
    /// Runtime 内 Drop は削除を専用 std スレッドで実行し、`DROP_REMOVE_TIMEOUT` (5 秒)
    /// を上限に完了を待つ。通常は timeout 内に完了するが、CI 環境の XPC 混雑で
    /// 超過し得るため、最終確認にはポーリングを使う。
    #[tokio::test]
    async fn xpc_alpine_drop_inside_runtime_removes_container() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");
        let id = container.id().to_string();
        drop(container);

        // DROP_REMOVE_TIMEOUT 超過時の best-effort 経路を考慮し、ポーリングで不在を確認する。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let ls = std::process::Command::new("container")
                .args(["ls", "-a"])
                .output()
                .expect("container ls の実行に失敗した");
            let ls_text = String::from_utf8_lossy(&ls.stdout).to_string();
            if !ls_text.contains(&id) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Runtime 内 Drop 後にコンテナが削除されること: {id}"
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    /// Runtime 内の同期コンテキストから rm_blocking でコンテナが削除されること。
    ///
    /// `rm_blocking` は `block_on` を使わず `remove_blocking` (同期 XPC) を直接
    /// 呼び出すため、tokio Runtime 内の `spawn_blocking` 内から呼んでも deadlock しない。
    /// `Ok` を返した直後に 1 ショット `container ls` で不在を確認する。
    #[tokio::test]
    async fn xpc_alpine_rm_blocking_inside_runtime_removes_container() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");
        let id = container.id().to_string();
        // Runtime 内の同期コンテキスト (spawn_blocking) から rm_blocking を呼ぶ
        tokio::task::spawn_blocking(move || {
            container.rm_blocking().expect("rm_blocking に失敗した");
        })
        .await
        .expect("spawn_blocking に失敗した");

        // rm_blocking の Ok 復帰直後は削除完了済みなので、1 ショットで不在を確認できる。
        let ls = std::process::Command::new("container")
            .args(["ls", "-a"])
            .output()
            .expect("container ls の実行に失敗した");
        let ls_text = String::from_utf8_lossy(&ls.stdout).to_string();
        assert!(
            !ls_text.contains(&id),
            "rm_blocking 後にコンテナが削除されていること: {id}"
        );
    }

    #[tokio::test]
    async fn xpc_alpine_log_consumer_with_log_wait() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // with_log_consumer と message_on_stdout 待機の併用が動くこと。
        // 旧実装は consumer が FD を奪うため、ログ待機が必ず EndOfStream で失敗していた。
        let logs = Arc::new(Mutex::new(Vec::new()));
        let logs_clone = logs.clone();
        let container = GenericImage::new("alpine", "latest")
            .with_wait_for(WaitFor::message_on_stdout("CONSUMER_AND_WAIT"))
            .with_cmd(["sh", "-c", "echo CONSUMER_AND_WAIT; sleep 30"])
            .with_log_consumer(move |record: &LogFrame| {
                logs_clone
                    .lock()
                    .expect("処理に失敗しないこと")
                    .push(record.bytes().to_vec());
            })
            .start()
            .await
            .expect("consumer とログ待機を併用した起動が成功すること");

        // consumer への配信は非同期なので少し待つ。
        tokio::time::sleep(Duration::from_secs(1)).await;
        {
            let captured = logs.lock().expect("処理に失敗しないこと");
            assert!(
                captured
                    .iter()
                    .any(|bytes| String::from_utf8_lossy(bytes).contains("CONSUMER_AND_WAIT")),
                "log consumer もメッセージを受け取ること"
            );
        }

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_delayed_log_wait() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // ログ待機戦略が「待つ」こと。
        // 旧実装は EOF で即 EndOfStream になり、出力が遅いイメージで必ず失敗していた。
        let container = GenericImage::new("alpine", "latest")
            .with_wait_for(WaitFor::message_on_stdout("DELAYED_READY"))
            .with_cmd(["sh", "-c", "sleep 2; echo DELAYED_READY; sleep 30"])
            .start()
            .await
            .expect("ログ待機が遅延したメッセージを検出するまで polling すること");

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_exposed_port_auto_mapping() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // with_exposed_port のポートに空きホストポートが自動割当されること。
        // 旧実装は expose_ports を黙って無視していた。
        use shiguredo_container::core::IntoContainerPort;
        let container = GenericImage::new("alpine", "latest")
            .with_exposed_port(80.tcp())
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("公開ポート付き alpine コンテナの起動に失敗した");

        let ports = container.ports().await.expect("ポート情報の取得に失敗した");
        let host_port = ports
            .map_to_host_port_ipv4(80_u16)
            .expect("公開ポート 80 がホストポートへマッピングされること");
        assert_ne!(host_port, 0, "ホストポートが割り当てられること");

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[cfg(feature = "http_wait_plain")]
    #[tokio::test]
    async fn nginx_starts_without_with_cmd() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // HTTP wait は localhost の published port を経由する。
        // self-hosted macOS CI では Local Network Privacy の関係でフォワーダーが機能しないためスキップする。
        if std::env::var("GITHUB_ACTIONS").is_ok() {
            eprintln!(
                "スキップ: GitHub Actions では published port 経由の HTTP 待機が Local Network Privacy で使えない"
            );
            return;
        }

        // with_cmd 無しでもイメージ既定の CMD/ENTRYPOINT で起動できること。
        use shiguredo_container::core::{IntoContainerPort, wait::HttpWaitStrategy};
        let container = GenericImage::new("nginx", "latest")
            .with_exposed_port(80.tcp())
            .with_wait_for(WaitFor::http(
                HttpWaitStrategy::new("/").with_expected_status_code(200_u16),
            ))
            .start()
            .await
            .expect("with_cmd なしの nginx 起動に失敗した");

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    /// with_cmd 無し起動の e2e。CI (self-hosted) でも実行する。
    #[tokio::test]
    async fn nginx_starts_without_with_cmd_log_wait() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // with_cmd 無しでもイメージ既定の CMD/ENTRYPOINT で起動できること。
        // 並列負荷時に WaitFor::Log (follow) が既定〜数分の timeout でも
        // メッセージを見落とす事例があるため、起動後に stdout を先頭から
        // 繰り返し読んで確認する (published port / HTTP には依存しない)。
        let container = GenericImage::new("nginx", "latest")
            .start()
            .await
            .expect("with_cmd なしの nginx 起動に失敗した");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let last_stdout = loop {
            let bytes = container
                .stdout_to_vec()
                .await
                .expect("stdout の読み取りに失敗した");
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if text.contains("start worker processes") {
                container.stop_with_timeout(Some(0)).await.ok();
                container.rm().await.ok();
                return;
            }
            if std::time::Instant::now() >= deadline {
                break text;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        };
        let stderr = container.stderr_to_vec().await.unwrap_or_default();
        panic!(
            "nginx 既定起動ログに start worker processes が現れない\n\
             --- stdout ---\n{last_stdout}\n\
             --- stderr ---\n{}",
            String::from_utf8_lossy(&stderr)
        );
    }

    #[tokio::test]
    async fn xpc_alpine_exec_cmd_ready_condition_stdout() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // CmdWaitFor::message_on_stdout が取得済みの stdout に対して照合されること。
        // 旧実装は「未対応」エラーを返していた。
        use shiguredo_container::core::CmdWaitFor;
        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        container
            .exec(
                ExecCommand::new(["sh", "-c", "echo EXEC_READY"])
                    .with_cmd_ready_condition(CmdWaitFor::message_on_stdout("EXEC_READY")),
            )
            .await
            .expect("一致する標準出力メッセージを持つ exec が成功すること");

        let result = container
            .exec(
                ExecCommand::new(["sh", "-c", "echo OTHER"])
                    .with_cmd_ready_condition(CmdWaitFor::message_on_stdout("NO_SUCH_MESSAGE")),
            )
            .await;
        assert!(
            result.is_err(),
            "標準出力メッセージが無い場合はエラーになること"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_stdout_to_vec_twice() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // リーダーが独立オフセットを持ち、stdout_to_vec を複数回呼んでも
        // 全内容が取得できること。旧実装は dup がオフセットを共有し 2 回目が空になった。
        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sh", "-c", "echo OFFSET_TEST; sleep 30"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");

        // ログがファイルに書かれるまで少し待つ。
        tokio::time::sleep(Duration::from_secs(1)).await;

        let first = container
            .stdout_to_vec()
            .await
            .expect("1 回目の標準出力読み取りに失敗した");
        let second = container
            .stdout_to_vec()
            .await
            .expect("2 回目の標準出力読み取りに失敗した");
        assert!(
            String::from_utf8_lossy(&first).contains("OFFSET_TEST"),
            "1 回目の読み取りにメッセージが含まれること"
        );
        assert!(
            String::from_utf8_lossy(&second).contains("OFFSET_TEST"),
            "2 回目の読み取りにもメッセージが含まれること (独立オフセット)"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_rm_and_stop_are_idempotent() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // 外部 (CLI) で削除済みのコンテナへの stop / rm が冪等に成功すること。
        // 本家が 304 / 404 を無視するのに相当する。
        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .await
            .expect("alpine コンテナの起動に失敗した");
        let id = container.id().to_string();

        // 二重 stop が成功すること。
        container
            .stop_with_timeout(Some(0))
            .await
            .expect("1 回目の停止が成功すること");
        container
            .stop_with_timeout(Some(0))
            .await
            .expect("2 回目の停止が成功すること");

        // CLI で外部削除してから stop / rm しても成功すること。
        let output = std::process::Command::new("container")
            .args(["rm", "--force", &id])
            .output()
            .expect("container rm の実行に失敗した");
        assert!(output.status.success(), "外部の rm が成功すること");

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("外部削除後の stop が成功すること");
        container
            .rm()
            .await
            .expect("外部削除後の rm が成功すること");
    }

    #[test]
    fn xpc_alpine_runtime_drop_while_running_does_not_hang() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // コンテナが実行中のままランタイムを drop してもハングしないこと。
        // 旧実装は spawn_blocking 上の containerWait (24 時間タイムアウト) を
        // ランタイム drop が join しようとして無期限にハングした。
        let rt = tokio::runtime::Runtime::new().expect("ランタイムの作成に失敗した");
        let container = rt.block_on(async {
            GenericImage::new("alpine", "latest")
                .with_cmd(["tail", "-f", "/dev/null"])
                .start()
                .await
                .expect("alpine コンテナの起動に失敗した")
        });
        let id = container.id().to_string();
        // 削除はランタイム外 drop の同期フォールバックに任せる。
        drop(container);

        // drop(rt) を監視スレッドで実行し、30 秒以内に完了することを確認する。
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(rt);
            let _ = tx.send(());
        });
        let result = rx.recv_timeout(std::time::Duration::from_secs(30));

        // 後始末 (ハングしていた場合に備えて必ず削除する)。
        let _ = std::process::Command::new("container")
            .args(["rm", "--force", &id])
            .output();

        assert!(
            result.is_ok(),
            "コンテナ実行中にランタイムを drop してもハングしないこと"
        );
    }

    #[tokio::test]
    async fn xpc_alpine_with_nonexistent_network_fails() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // 存在しないネットワークを指定した場合、自動作成されずエラーになること。
        let result = GenericImage::new("alpine", "latest")
            .with_network("shiguredo-container-rs-no-such-net")
            .with_cmd(["sleep", "30"])
            .start()
            .await;
        assert!(
            result.is_err(),
            "存在しないネットワークでの起動は失敗すること"
        );
    }

    #[tokio::test]
    async fn xpc_alpine_message_on_either_std() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // stderr にしか出ないメッセージを message_on_either_std で待機できること。
        let container = GenericImage::new("alpine", "latest")
            .with_wait_for(WaitFor::message_on_either_std("EITHER_READY"))
            .with_cmd(["sh", "-c", "echo EITHER_READY >&2; sleep 30"])
            .start()
            .await
            .expect("message_on_either_std が標準エラー出力だけのメッセージに一致すること");

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    #[tokio::test]
    async fn xpc_alpine_exit_wait_strategy_expected_exit_code() {
        if super::helpers::skip_if_ci() {
            return;
        }

        use shiguredo_container::core::wait::ExitWaitStrategy;

        // exit 0 のコンテナを with_exit_code(0) で待機すると成功すること。
        // 旧実装は containerList 由来の exit_code (常に None) と比較していたため
        // 期待値一致でも必ず UnexpectedExitCode になっていた。
        let container = GenericImage::new("alpine", "latest")
            .with_wait_for(WaitFor::exit(ExitWaitStrategy::default().with_exit_code(0)))
            .with_cmd(["sh", "-c", "exit 0"])
            .start()
            .await
            .expect("終了コード 0 が with_exit_code(0) を満たすこと");

        container.rm().await.expect("コンテナの削除に失敗した");
    }

    #[tokio::test]
    async fn xpc_alpine_exit_wait_strategy_unexpected_exit_code() {
        if super::helpers::skip_if_ci() {
            return;
        }

        use shiguredo_container::core::wait::ExitWaitStrategy;

        // exit 3 のコンテナを with_exit_code(0) で待機すると UnexpectedExitCode になること。
        let result = GenericImage::new("alpine", "latest")
            .with_wait_for(WaitFor::exit(ExitWaitStrategy::default().with_exit_code(0)))
            .with_cmd(["sh", "-c", "exit 3"])
            .start()
            .await;

        match result {
            Err(shiguredo_container::Error::WaitContainer(
                shiguredo_container::core::error::WaitContainerError::UnexpectedExitCode {
                    expected,
                    actual,
                },
            )) => {
                assert_eq!(expected, 0);
                assert_eq!(actual, Some(3), "実際の終了コードが観測されること");
            }
            other => panic!("UnexpectedExitCode エラーを期待したが異なる値だった: {other:?}"),
        }
    }

    /// ready 条件のログが現れないと、起動 timeout が公開エラーとして返ること。
    #[tokio::test]
    async fn xpc_alpine_startup_timeout_returns_error() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let name = format!("error-path-startup-timeout-{}", std::process::id());
        let timeout = Duration::from_secs(2);
        let result = GenericImage::new("alpine", "latest")
            .with_wait_for(WaitFor::message_on_stdout("NO_SUCH_MESSAGE"))
            .with_container_name(&name)
            .with_cmd(["sleep", "30"])
            .with_startup_timeout(timeout)
            .start()
            .await;

        match result {
            Err(shiguredo_container::Error::WaitContainer(
                WaitContainerError::StartupTimeout {
                    id,
                    timeout: actual,
                },
            )) => {
                assert_eq!(id, name, "コンテナ ID が指定値と一致すること");
                assert_eq!(actual, timeout, "timeout が指定値と一致すること");
            }
            other => panic!("起動 timeout は StartupTimeout になること: {other:?}"),
        }
    }

    /// 終了したコンテナの未一致ログ待機が診断ログ付きの EndOfStream を返すこと。
    #[tokio::test]
    async fn xpc_alpine_log_wait_end_of_stream_contains_collected_log() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let result = GenericImage::new("alpine", "latest")
            .with_wait_for(WaitFor::message_on_stdout("NO_SUCH_MESSAGE"))
            .with_cmd(["sh", "-c", "echo BEFORE_EXIT"])
            .with_startup_timeout(Duration::from_secs(10))
            .start()
            .await;

        match result {
            Err(shiguredo_container::Error::WaitContainer(WaitContainerError::WaitLog(
                WaitLogError::EndOfStream(chunks),
            ))) => {
                assert!(
                    chunks.len() <= 1,
                    "EndOfStream のログチャンク数は 0 または 1 であること: {chunks:?}"
                );
                assert!(
                    chunks
                        .concat()
                        .windows(b"BEFORE_EXIT".len())
                        .any(|w| w == b"BEFORE_EXIT"),
                    "EndOfStream の診断ログに BEFORE_EXIT が含まれること: {chunks:?}"
                );
            }
            other => panic!("ログ待機は EndOfStream になること: {other:?}"),
        }
    }

    /// macOS の healthcheck 待機がコンテナ ID 付きの未設定エラーを返すこと。
    #[tokio::test]
    async fn xpc_alpine_healthcheck_wait_returns_not_configured_error() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let name = format!("healthcheck-not-configured-{}", std::process::id());
        let result = GenericImage::new("alpine", "latest")
            .with_wait_for(WaitFor::healthcheck())
            .with_container_name(&name)
            .with_cmd(["sleep", "30"])
            .start()
            .await;

        match result {
            Err(shiguredo_container::Error::WaitContainer(
                WaitContainerError::HealthCheckNotConfigured(id),
            )) => {
                assert_eq!(
                    id, name,
                    "HealthCheckNotConfigured のコンテナ ID が指定した名前と一致すること"
                );
            }
            other => panic!("healthcheck 待機は未設定エラーになること: {other:?}"),
        }
    }

    /// `with_health_check` は macOS で start 時に明示エラーになること。
    #[tokio::test]
    async fn xpc_with_health_check_is_rejected() {
        if super::helpers::skip_if_ci() {
            return;
        }

        use shiguredo_container::Healthcheck;

        let err = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "30"])
            .with_health_check(Healthcheck::cmd_shell("true"))
            .start()
            .await
            .expect_err("with_health_check は macOS で拒否されること");

        assert!(
            err.to_string()
                .contains("with_health_check() is not supported on macOS"),
            "macOS fail-fast メッセージを含むこと: {err}"
        );
    }

    /// 不正な `with_container_name` が macOS の start で pull / resolve より前に明示エラーになること。
    ///
    /// ID 検証が pull / resolve より前に走ることを実証するため、存在しないイメージ名を使う。
    /// 検証が pull より後に移動するとイメージ解決が先に失敗し、このテストは落ちる。
    /// エラー内容 (ID 値と規則の要約) も検証する。
    #[tokio::test]
    async fn xpc_invalid_container_name_is_rejected() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // 63 文字超・空文字・1 文字・禁止文字・先頭が非英数字の各ケース。
        for invalid in [
            "a".repeat(64),
            String::new(),
            "a".to_string(),
            "has space".to_string(),
            "-leading-dash".to_string(),
        ] {
            let err = GenericImage::new("shiguredo/no-such-image", "latest")
                .with_container_name(&invalid)
                .with_cmd(["sleep", "30"])
                .start()
                .await
                .expect_err("不正なコンテナ名は start で拒否されること");

            let message = err.to_string();
            assert!(
                message.contains("invalid container id")
                    && message.contains("must match ^[a-zA-Z0-9][a-zA-Z0-9_.-]+$"),
                "エラーに規則の要約が含まれること: {message}"
            );
            assert!(
                message.contains(&format!("{invalid:?}")),
                "エラーに ID 値が引用符付きで含まれること: {message}"
            );
        }
    }
}

#[cfg(all(target_os = "macos", feature = "blocking"))]
mod test_container_sync {
    use std::io::Read;
    use std::sync::{Arc, Mutex};

    use shiguredo_container::{
        GenericImage, Healthcheck, ImageExt, SyncRunner, core::ExecCommand, core::logs::LogFrame,
    };

    /// 同期経路でも `with_health_check` は macOS で明示エラーになること。
    #[test]
    fn sync_with_health_check_is_rejected() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let result = SyncRunner::start(
            GenericImage::new("alpine", "latest")
                .with_cmd(["sleep", "30"])
                .with_health_check(Healthcheck::cmd_shell("true")),
        );
        match result {
            Err(err) => {
                assert!(
                    err.to_string()
                        .contains("with_health_check() is not supported on macOS"),
                    "macOS fail-fast メッセージを含むこと: {err}"
                );
            }
            Ok(_) => panic!("with_health_check は macOS で拒否されること"),
        }
    }

    #[test]
    fn sync_alpine_log_consumer_runs_in_background() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // sync 経路でも block_on 中でない間に LogConsumer へ配信されること。
        // 旧実装は current_thread ランタイムのため、メソッド呼び出し中しか
        // バックグラウンドタスクが進まず、放置中はログが届かなかった。
        let logs = Arc::new(Mutex::new(Vec::new()));
        let logs_clone = logs.clone();
        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sh", "-c", "sleep 1; echo SYNC_BG_LOG; sleep 30"])
            .with_log_consumer(move |record: &LogFrame| {
                logs_clone
                    .lock()
                    .expect("処理に失敗しないこと")
                    .push(record.bytes().to_vec());
            })
            .start()
            .expect("alpine コンテナの起動に失敗した");

        // block_on を一切呼ばずに待つ。ワーカースレッドが配信していれば届く。
        std::thread::sleep(std::time::Duration::from_secs(3));
        {
            let captured = logs.lock().expect("処理に失敗しないこと");
            assert!(
                captured
                    .iter()
                    .any(|bytes| String::from_utf8_lossy(bytes).contains("SYNC_BG_LOG")),
                "log consumer がアイドル中もログを受け取ること"
            );
        }

        container
            .stop_with_timeout(Some(0))
            .expect("コンテナの停止に失敗した");
        container.rm().expect("コンテナの削除に失敗した");
    }

    #[test]
    fn sync_alpine_exec() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .expect("alpine コンテナの起動に失敗した");

        let result = container
            .exec(ExecCommand::new(["uname", "-a"]))
            .expect("uname の実行に失敗した");
        let mut result = result;
        assert_eq!(
            result.exit_code().expect("終了コードの取得に失敗した"),
            Some(0),
            "exec が成功すること"
        );
        let stdout = result
            .stdout_to_vec()
            .expect("標準出力の読み取りに失敗した");
        let stdout = String::from_utf8_lossy(&stdout);
        assert!(
            stdout.contains("Linux"),
            "stdout に Linux が含まれること: {stdout}"
        );

        container
            .stop_with_timeout(Some(0))
            .expect("コンテナの停止に失敗した");
        container.rm().expect("コンテナの削除に失敗した");
    }

    /// 同期 Container の状態 snapshot が host と自動割当 port mapping を返すこと。
    #[test]
    fn sync_alpine_container_state() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_exposed_port(80_u16.into())
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .expect("alpine コンテナの起動に失敗した");

        let verification = (|| -> Result<(), String> {
            let state = container
                .container_state()
                .map_err(|e| format!("コンテナ状態の取得に失敗した: {e}"))?;
            let host = container
                .get_host()
                .map_err(|e| format!("ホストの取得に失敗した: {e}"))?;
            if state.host() != &host {
                return Err(format!(
                    "状態の host と Container の host が一致しない: state={} container={host}",
                    state.host()
                ));
            }
            let host_port = container
                .get_host_port_ipv4(80_u16)
                .map_err(|e| format!("コンテナのポート 80 mapping が取得できなかった: {e}"))?;
            let actual_port = state
                .host_port_ipv4(80_u16.into())
                .map_err(|e| format!("状態のポート 80 の IPv4 mapping が取得できなかった: {e}"))?;
            if actual_port != host_port {
                return Err(format!(
                    "ポート 80 の host port が一致しない: actual={actual_port} expected={host_port}"
                ));
            }
            Ok(())
        })();

        let stop_result = container.stop_with_timeout(Some(0));
        let remove_result = container.rm();
        if let Err(e) = stop_result {
            panic!("コンテナの停止に失敗した: {e}");
        }
        if let Err(e) = remove_result {
            panic!("コンテナの削除に失敗した: {e}");
        }
        if let Err(message) = verification {
            panic!("{message}");
        }
    }

    /// 同期 stdout リーダーが独立オフセットを持つこと。
    #[test]
    fn sync_alpine_stdout_readers_are_independent() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let marker = "SYNC_STDOUT_READER_MARKER";
        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sh", "-c", "echo SYNC_STDOUT_READER_MARKER; sleep 30"])
            .start()
            .expect("alpine コンテナの起動に失敗した");

        std::thread::sleep(std::time::Duration::from_secs(1));
        let verification = (|| -> Result<(), String> {
            let mut first_reader = container.stdout(false);
            let mut second_reader = container.stdout(false);
            let mut first = Vec::new();
            let mut second = Vec::new();
            first_reader
                .read_to_end(&mut first)
                .map_err(|e| format!("1 個目の stdout リーダーの読み取りに失敗した: {e}"))?;
            second_reader
                .read_to_end(&mut second)
                .map_err(|e| format!("2 個目の stdout リーダーの読み取りに失敗した: {e}"))?;
            let all = container
                .stdout_to_vec()
                .map_err(|e| format!("stdout_to_vec の読み取りに失敗した: {e}"))?;
            if first != second || first != all {
                return Err("2 個の stdout リーダーと stdout_to_vec の内容が一致しない".to_owned());
            }
            if !String::from_utf8_lossy(&first).contains(marker) {
                return Err(format!("stdout に固定マーカーが含まれない: {first:?}"));
            }
            Ok(())
        })();

        let stop_result = container.stop_with_timeout(Some(0));
        let remove_result = container.rm();
        if let Err(e) = stop_result {
            panic!("コンテナの停止に失敗した: {e}");
        }
        if let Err(e) = remove_result {
            panic!("コンテナの削除に失敗した: {e}");
        }
        if let Err(message) = verification {
            panic!("{message}");
        }
    }

    /// 同期 stderr リーダーが独立オフセットを持つこと。
    #[test]
    fn sync_alpine_stderr_readers_are_independent() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["tail", "-f", "/dev/null"])
            .start()
            .expect("alpine コンテナの起動に失敗した");

        let verification = (|| -> Result<(), String> {
            let mut first_reader = container.stderr(false);
            let mut second_reader = container.stderr(false);
            let mut first = Vec::new();
            let mut second = Vec::new();
            first_reader
                .read_to_end(&mut first)
                .map_err(|e| format!("1 個目の stderr リーダーの読み取りに失敗した: {e}"))?;
            second_reader
                .read_to_end(&mut second)
                .map_err(|e| format!("2 個目の stderr リーダーの読み取りに失敗した: {e}"))?;
            let all = container
                .stderr_to_vec()
                .map_err(|e| format!("stderr_to_vec の読み取りに失敗した: {e}"))?;
            if first != second || first != all {
                return Err("2 個の stderr リーダーと stderr_to_vec の内容が一致しない".to_owned());
            }
            Ok(())
        })();

        let stop_result = container.stop_with_timeout(Some(0));
        let remove_result = container.rm();
        if let Err(e) = stop_result {
            panic!("コンテナの停止に失敗した: {e}");
        }
        if let Err(e) = remove_result {
            panic!("コンテナの削除に失敗した: {e}");
        }
        if let Err(message) = verification {
            panic!("{message}");
        }
    }

    /// 同期 stdout(follow=true) が追記行を読み、プロセス終了後に EOF すること。
    #[test]
    fn sync_alpine_stdout_follow_reads_appended_lines() {
        if super::helpers::skip_if_ci() {
            return;
        }

        // 短命コマンド: 追記後に終了し、follow が exit_code で打ち切られること。
        let container = GenericImage::new("alpine", "latest")
            .with_cmd([
                "sh",
                "-c",
                "echo FOLLOW_SYNC_FIRST; sleep 1; echo FOLLOW_SYNC_SECOND",
            ])
            .start()
            .expect("alpine コンテナの起動に失敗した");

        let verification = (|| -> Result<(), String> {
            let mut follow_reader = container.stdout(true);
            let mut follow = Vec::new();
            follow_reader
                .read_to_end(&mut follow)
                .map_err(|e| format!("follow=true の stdout 読み取りに失敗した: {e}"))?;
            let text = String::from_utf8_lossy(&follow);
            if !text.contains("FOLLOW_SYNC_FIRST") {
                return Err(format!("follow stdout に FIRST が含まれない: {text}"));
            }
            if !text.contains("FOLLOW_SYNC_SECOND") {
                return Err(format!("follow stdout に SECOND が含まれない: {text}"));
            }
            Ok(())
        })();

        let remove_result = container.rm();
        if let Err(e) = remove_result {
            panic!("コンテナの削除に失敗した: {e}");
        }
        if let Err(message) = verification {
            panic!("{message}");
        }
    }

    /// 同期 stderr(follow=true) がプロセス終了後に EOF すること。
    #[test]
    fn sync_alpine_stderr_follow_terminates_after_exit() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sh", "-c", "echo FOLLOW_SYNC_STDERR_DONE; sleep 1"])
            .start()
            .expect("alpine コンテナの起動に失敗した");

        let verification = (|| -> Result<(), String> {
            let mut follow_reader = container.stderr(true);
            let mut follow = Vec::new();
            follow_reader
                .read_to_end(&mut follow)
                .map_err(|e| format!("follow=true の stderr 読み取りに失敗した: {e}"))?;
            // Apple の stderr は bootlog。内容より「終了すること」が主目的。
            let _ = follow;
            Ok(())
        })();

        let remove_result = container.rm();
        if let Err(e) = remove_result {
            panic!("コンテナの削除に失敗した: {e}");
        }
        if let Err(message) = verification {
            panic!("{message}");
        }
    }
}

// コンテナ IP 直結の HTTP 検証テスト。
//
// published port フォワーダーには依存しない。self-hosted CI (`RUN_CONTAINER_TESTS=1`)
// では実行する。ローカルは macOS Local Network Privacy の許可が必要なため
// `RUN_HOST_NETWORK_TESTS=1` のときだけ実行する。
#[cfg(target_os = "macos")]
mod test_container_http_direct {
    use std::net::IpAddr;
    use std::time::Duration;

    use shiguredo_container::{
        ContainerAsync, GenericImage, Image, ImageExt, runners::AsyncRunner,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// コンテナ IP に直接 TCP 接続して HTTP GET し、(ステータスコード, レスポンス全文) を返す。
    async fn http_get(ip: IpAddr, port: u16, path: &str) -> std::io::Result<(u16, String)> {
        let mut stream = tokio::net::TcpStream::connect((ip, port)).await?;
        let req = format!("GET {path} HTTP/1.1\r\nHost: {ip}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await?;
        let text = String::from_utf8_lossy(&buf).into_owned();
        let status = text
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse::<u16>().ok())
            .unwrap_or(0);
        Ok((status, text))
    }

    /// コンテナ IP に対して HTTP が応答するまでポーリングし、最初の成功レスポンスを返す。
    async fn wait_http_ready<I: Image>(container: &ContainerAsync<I>, port: u16) -> (u16, String) {
        let ip = container
            .get_bridge_ip_address()
            .await
            .expect("ブリッジ IP アドレスの取得に失敗した");
        let poll = async {
            loop {
                if let Ok((status, body)) = http_get(ip, port, "/").await
                    && status != 0
                {
                    return (status, body);
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(180), poll)
            .await
            .expect("nginx が制限時間内にコンテナ IP 経由で HTTP 利用可能にならなかった")
    }

    #[tokio::test]
    async fn xpc_nginx_http_direct_status_code() {
        if super::helpers::skip_if_ci() {
            return;
        }
        // CI ではコンテナ IP 直結を実行する。ローカルは LNP 許可が必要なためゲートする。
        if std::env::var("GITHUB_ACTIONS").is_err() && super::skip_unless_host_network() {
            return;
        }

        // nginx を起動し、コンテナ IP 直結でステータス 200 を確認できること。
        let container = GenericImage::new("nginx", "latest")
            .with_cmd(["nginx", "-g", "daemon off;"])
            .start()
            .await
            .expect("nginx が起動すること");

        let (status, _body) = wait_http_ready(&container, 80).await;
        assert_eq!(status, 200, "nginx は 200 を返すこと");

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    #[tokio::test]
    async fn xpc_nginx_http_direct_body() {
        if super::helpers::skip_if_ci() {
            return;
        }
        // CI ではコンテナ IP 直結を実行する。ローカルは LNP 許可が必要なためゲートする。
        if std::env::var("GITHUB_ACTIONS").is_err() && super::skip_unless_host_network() {
            return;
        }

        // レスポンスボディの内容まで検証できること。
        let container = GenericImage::new("nginx", "latest")
            .with_cmd(["nginx", "-g", "daemon off;"])
            .start()
            .await
            .expect("nginx が起動すること");

        let (status, body) = wait_http_ready(&container, 80).await;
        assert_eq!(status, 200, "nginx は 200 を返すこと");
        assert!(
            body.contains("nginx"),
            "レスポンスボディに 'nginx' が含まれること"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }
}

// published port + HttpWaitStrategy のフォワーダー経由テスト。
//
// macOS の Local Network Privacy によりフォワーダーがヘッドレス CI で機能しないため、
// LNP を許可済みのローカル環境で RUN_HOST_NETWORK_TESTS=1 を指定した時のみ実行する。
#[cfg(all(target_os = "macos", feature = "http_wait_plain"))]
mod test_container_http_wait {
    use shiguredo_container::{
        GenericImage, ImageExt,
        core::{IntoContainerPort, WaitFor, wait::HttpWaitStrategy},
        runners::AsyncRunner,
    };

    #[tokio::test]
    async fn xpc_nginx_http_wait_expected_status_code() {
        if super::helpers::skip_if_ci() || super::skip_unless_host_network() {
            return;
        }

        // nginx の起動を WaitFor::http で待機できること。
        // ポート未指定時は最初の公開ポートが使われるが、ここでは明示する。
        let container = GenericImage::new("nginx", "latest")
            .with_exposed_port(80.tcp())
            .with_wait_for(WaitFor::http(
                HttpWaitStrategy::new("/")
                    .with_port(80.tcp())
                    .with_expected_status_code(200_u16),
            ))
            .with_cmd(["nginx", "-g", "daemon off;"])
            .with_startup_timeout(std::time::Duration::from_secs(180))
            .start()
            .await
            .expect("nginx が HTTP 待機で利用可能になること");

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    #[tokio::test]
    async fn xpc_nginx_http_wait_response_matcher_body() {
        if super::helpers::skip_if_ci() || super::skip_unless_host_network() {
            return;
        }

        // response matcher でボディの内容まで検証できること。
        let container = GenericImage::new("nginx", "latest")
            .with_exposed_port(80.tcp())
            .with_wait_for(WaitFor::http(
                HttpWaitStrategy::new("/")
                    .with_port(80.tcp())
                    .with_response_matcher(|response| {
                        response.status() == 200
                            && String::from_utf8_lossy(response.body()).contains("nginx")
                    }),
            ))
            .with_cmd(["nginx", "-g", "daemon off;"])
            .with_startup_timeout(std::time::Duration::from_secs(180))
            .start()
            .await
            .expect("nginx が HTTP 本文 matcher で利用可能になること");

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }
}

/// 構築後エラーパスで Drop 任せにせず明示的に削除されることの回帰テスト。
///
/// `cleanup_container` は `test_container_sync_drop_macos.rs` の同名ヘルパーの複製。
#[cfg(target_os = "macos")]
mod test_drop_removal_race {
    use std::time::Duration;

    use shiguredo_container::{
        GenericImage, ImageExt,
        core::{WaitFor, error::WaitContainerError},
        runners::AsyncRunner,
    };

    /// drop 経路のテストで削除されずに残り得るコンテナを強制削除する (ベストエフォート)。
    /// `test_container_sync_drop_macos.rs` の `cleanup_container` の複製。
    fn cleanup_container(id: &str) {
        let _ = std::process::Command::new("container")
            .args(["rm", "--force", id])
            .output();
    }

    /// `container list --all --quiet` の出力に対象 ID が行完全一致で含まれるか。
    fn container_id_listed(id: &str) -> bool {
        let out = std::process::Command::new("container")
            .args(["list", "--all", "--quiet"])
            .output()
            .expect("container list の実行に失敗した");
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines().any(|line| line.trim() == id)
    }

    /// 起動タイムアウトの構築後エラーで、Err 返却後にコンテナが残らないこと。
    ///
    /// Err 受領から確認まで await を挟まない (キュー内の削除タスクを駆動しないため)。
    #[tokio::test(flavor = "current_thread")]
    async fn async_startup_timeout_removes_container() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let name = format!("drop-removal-async-timeout-{}", std::process::id());
        cleanup_container(&name);

        let result = GenericImage::new("alpine", "latest")
            .with_wait_for(WaitFor::message_on_stdout("NO_SUCH_MESSAGE_DROP_REMOVAL"))
            .with_container_name(&name)
            .with_cmd(["sleep", "30"])
            .with_startup_timeout(Duration::from_secs(2))
            .start()
            .await;

        match result {
            Err(shiguredo_container::Error::WaitContainer(
                WaitContainerError::StartupTimeout { id, timeout },
            )) => {
                assert_eq!(id, name, "コンテナ ID が指定値と一致すること");
                assert_eq!(
                    timeout,
                    Duration::from_secs(2),
                    "timeout が指定値と一致すること"
                );
            }
            other => panic!("起動タイムアウトは StartupTimeout になること: {other:?}"),
        }

        // await を挟まず blocking で確認する。
        let listed = container_id_listed(&name);
        cleanup_container(&name);
        assert!(
            !listed,
            "構築後エラー後にコンテナが残っていないこと: {name}"
        );
    }

    /// 構築後エラー (with_health_check 未対応) で Err 返却後にコンテナが残らないこと。
    #[tokio::test(flavor = "current_thread")]
    async fn async_host_gateway_removes_container() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let name = format!("drop-removal-async-hostgw-{}", std::process::id());
        cleanup_container(&name);

        let result = GenericImage::new("alpine", "latest")
            .with_container_name(&name)
            .with_health_check(shiguredo_container::core::healthcheck::Healthcheck::none())
            .with_cmd(["sleep", "30"])
            .start()
            .await;

        match result {
            Err(err) => {
                assert!(
                    err.to_string().contains("with_health_check"),
                    "with_health_check 未対応のメッセージを含むこと: {err}"
                );
            }
            Ok(_) => panic!("with_health_check は macOS で必ずエラーになること"),
        }

        let listed = container_id_listed(&name);
        cleanup_container(&name);
        assert!(
            !listed,
            "構築後エラー後にコンテナが残っていないこと: {name}"
        );
    }
}

/// `Image::exec_after_start` が `AsyncRunner::start` 経由で実行されることの回帰テスト。
///
/// 同期経路 (`SyncRunner`) は `AsyncRunner::start` への委譲のため、本モジュールの
/// 非同期テストでカバーする。
#[cfg(target_os = "macos")]
mod test_exec_after_start {
    use shiguredo_container::{
        ExecCommand, Image, ImageExt,
        core::{WaitFor, image::ContainerState, wait::CmdWaitFor},
        runners::AsyncRunner,
    };

    /// `exec_after_start` でマーカーファイルを作成するテスト用イメージ。
    struct MarkerAfterStart;

    impl Image for MarkerAfterStart {
        fn name(&self) -> &str {
            "alpine"
        }

        fn tag(&self) -> &str {
            "latest"
        }

        fn ready_conditions(&self) -> Vec<WaitFor> {
            Vec::new()
        }

        fn exec_after_start(
            &self,
            _cs: ContainerState,
        ) -> shiguredo_container::core::error::Result<Vec<ExecCommand>> {
            Ok(vec![
                ExecCommand::new(["touch", "/tmp/marker"])
                    .with_cmd_ready_condition(CmdWaitFor::exit_code(0)),
            ])
        }
    }

    /// `exec_before_ready` でマーカーファイルを作成するテスト用イメージ。
    struct MarkerBeforeReady;

    impl Image for MarkerBeforeReady {
        fn name(&self) -> &str {
            "alpine"
        }

        fn tag(&self) -> &str {
            "latest"
        }

        fn ready_conditions(&self) -> Vec<WaitFor> {
            Vec::new()
        }

        fn exec_before_ready(
            &self,
            _cs: ContainerState,
        ) -> shiguredo_container::core::error::Result<Vec<ExecCommand>> {
            Ok(vec![
                ExecCommand::new(["touch", "/tmp/before-ready-marker"])
                    .with_cmd_ready_condition(CmdWaitFor::exit_code(0)),
            ])
        }
    }

    /// `exec_after_start` が失敗するテスト用イメージ。
    struct FailAfterStart;

    impl Image for FailAfterStart {
        fn name(&self) -> &str {
            "alpine"
        }

        fn tag(&self) -> &str {
            "latest"
        }

        fn ready_conditions(&self) -> Vec<WaitFor> {
            Vec::new()
        }

        fn exec_after_start(
            &self,
            _cs: ContainerState,
        ) -> shiguredo_container::core::error::Result<Vec<ExecCommand>> {
            // 既定の CmdWaitFor::Nothing は終了コードを検査しないため、
            // exit_code(0) を付けて false の非ゼロ終了を確定的にエラーにする。
            Ok(vec![
                ExecCommand::new(["false"]).with_cmd_ready_condition(CmdWaitFor::exit_code(0)),
            ])
        }
    }

    /// runner 経由の start 後に exec_after_start のコマンドが実行されていること。
    #[tokio::test]
    async fn runner_runs_exec_after_start() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = MarkerAfterStart
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("exec_after_start 付きイメージの起動に失敗した");

        let result = container
            .exec(
                ExecCommand::new(["test", "-f", "/tmp/marker"])
                    .with_cmd_ready_condition(CmdWaitFor::exit_code(0)),
            )
            .await
            .expect("マーカーファイルの存在確認に失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("exit_code の取得に失敗した"),
            Some(0),
            "マーカーファイルが存在すること"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    /// runner 経由の start 中に exec_before_ready のコマンドが実行されていること。
    #[tokio::test]
    async fn runner_runs_exec_before_ready() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = MarkerBeforeReady
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("exec_before_ready 付きイメージの起動に失敗した");

        let result = container
            .exec(
                ExecCommand::new(["test", "-f", "/tmp/before-ready-marker"])
                    .with_cmd_ready_condition(CmdWaitFor::exit_code(0)),
            )
            .await
            .expect("マーカーファイルの存在確認に失敗した");
        assert_eq!(
            result
                .exit_code()
                .await
                .expect("exit_code の取得に失敗した"),
            Some(0),
            "マーカーファイルが存在すること"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("コンテナの停止に失敗した");
        container.rm().await.expect("コンテナの削除に失敗した");
    }

    /// exec_after_start のコマンド失敗が start の Err として伝播すること。
    #[tokio::test]
    async fn runner_propagates_exec_after_start_failure() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let result = FailAfterStart.with_cmd(["sleep", "30"]).start().await;
        assert!(
            result.is_err(),
            "exec_after_start の失敗は start の Err になること"
        );
    }
}

/// `ContainerAsync::start()` の stop → start 再起動と関連状態の回帰テスト。
#[cfg(target_os = "macos")]
mod test_container_restart {
    use std::time::Duration;

    use shiguredo_container::{
        ExecCommand, GenericImage, Image, ImageExt,
        core::{WaitFor, image::ContainerState, wait::CmdWaitFor},
        runners::AsyncRunner,
    };

    /// exec_after_start でカウントを増やすテスト用イメージ。
    struct CountAfterStart;

    impl Image for CountAfterStart {
        fn name(&self) -> &str {
            "alpine"
        }

        fn tag(&self) -> &str {
            "latest"
        }

        fn ready_conditions(&self) -> Vec<WaitFor> {
            Vec::new()
        }

        fn exec_after_start(
            &self,
            _cs: ContainerState,
        ) -> shiguredo_container::core::error::Result<Vec<ExecCommand>> {
            Ok(vec![
                ExecCommand::new(["sh", "-c", "echo x >> /tmp/count"])
                    .with_cmd_ready_condition(CmdWaitFor::exit_code(0)),
            ])
        }
    }

    async fn count_markers(
        container: &shiguredo_container::ContainerAsync<CountAfterStart>,
    ) -> usize {
        let mut result = container
            .exec(
                ExecCommand::new(["sh", "-c", "cat /tmp/count 2>/dev/null || true"])
                    .with_cmd_ready_condition(CmdWaitFor::exit_code(0)),
            )
            .await
            .expect("カウント読み取りに失敗した");
        let buf = result
            .stdout_to_vec()
            .await
            .expect("stdout 読み取りに失敗した");
        String::from_utf8_lossy(&buf).matches('x').count()
    }

    /// stop → start で再起動し、exit_code リセットと wait 再武装、exec_after_start 再実行を検証する。
    #[tokio::test]
    async fn stop_start_restarts_and_resets_wait_state() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = CountAfterStart
            .with_cmd(["sh", "-c", "echo MARKER_V1; sleep 60"])
            .start()
            .await
            .expect("初回起動に失敗した");

        let after_boot = count_markers(&container).await;
        assert_eq!(
            after_boot, 1,
            "runner の exec_after_start で 1 回増えること"
        );

        // 起動済みへの start() は再起動せず exec_after_start のみ。
        container
            .start()
            .await
            .expect("起動済みへの start に失敗した");
        let after_running_start = count_markers(&container).await;
        assert_eq!(
            after_running_start,
            after_boot + 1,
            "起動済み start で exec_after_start が再実行されること"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("停止に失敗した");
        assert!(
            !container
                .is_running()
                .await
                .expect("実行状態の取得に失敗した"),
            "stop 後は running でないこと"
        );

        // 実機確認項目 1: bootstrap + start_process 再送が受理され running に戻ること。
        container
            .start()
            .await
            .expect("stop 後の start (再起動) に失敗した");
        assert!(
            container
                .is_running()
                .await
                .expect("再起動後の実行状態取得に失敗した"),
            "再 start 後は running であること"
        );

        // exec 疎通。
        container
            .exec(ExecCommand::new(["true"]).with_cmd_ready_condition(CmdWaitFor::exit_code(0)))
            .await
            .expect("再 start 後の exec に失敗した");

        let after_restart = count_markers(&container).await;
        assert_eq!(
            after_restart,
            after_running_start + 1,
            "再起動経路でも exec_after_start が再実行されること"
        );

        // 再 start 直後の running 中は exit_code が None (リセット検証)。
        assert_eq!(
            container
                .exit_code()
                .await
                .expect("終了コードの取得に失敗した"),
            None,
            "再 start 後の running 中は exit_code が None であること"
        );

        // 実機確認項目 2: 旧ログ FD から再起動後の出力が読めるか。
        // 同一 CMD のため MARKER_V1 が再度出る。stdout_to_vec は先頭から読む。
        tokio::time::sleep(Duration::from_secs(1)).await;
        let stdout = String::from_utf8_lossy(
            &container
                .stdout_to_vec()
                .await
                .expect("再起動後の標準出力読み取りに失敗した"),
        )
        .to_string();
        eprintln!("再起動後の標準出力: {stdout}");
        assert!(
            stdout.contains("MARKER_V1"),
            "ログ FD 差し替え後に再起動分のログが読めること: {stdout}"
        );

        // 2 回目の stop 後、wait スレッド再 spawn により exit_code が Some になること。
        container
            .stop_with_timeout(Some(0))
            .await
            .expect("2 回目の stop に失敗した");
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut got = None;
        while std::time::Instant::now() < deadline {
            got = container
                .exit_code()
                .await
                .expect("終了コードの polling に失敗した");
            if got.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        assert!(
            got.is_some(),
            "再武装した wait スレッドにより exit_code が Some になること"
        );

        container.rm().await.ok();
    }

    /// GenericImage でも stop → start が動くこと (空の exec_after_start)。
    #[tokio::test]
    async fn generic_image_stop_start_is_running() {
        if super::helpers::skip_if_ci() {
            return;
        }

        let container = GenericImage::new("alpine", "latest")
            .with_cmd(["sleep", "30"])
            .start()
            .await
            .expect("起動に失敗した");

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("停止に失敗した");
        container.start().await.expect("再 start に失敗した");
        assert!(
            container
                .is_running()
                .await
                .expect("実行状態の取得に失敗した"),
            "再 start 後は running であること"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }

    /// with_log_consumer 付きで再 start 後に、再起動後のログ行を consumer が受け取ること。
    #[tokio::test]
    async fn log_consumer_receives_lines_after_restart() {
        if super::helpers::skip_if_ci() {
            return;
        }

        use std::sync::{Arc, Mutex};

        use shiguredo_container::core::logs::LogFrame;

        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let lines_clone = lines.clone();

        let container = GenericImage::new("alpine", "latest")
            .with_log_consumer(move |frame: &LogFrame| {
                lines_clone
                    .lock()
                    .expect("処理に失敗しないこと")
                    .push(String::from_utf8_lossy(frame.bytes()).to_string());
            })
            .with_cmd(["sh", "-c", "echo CONSUMER_BOOT_1; sleep 60"])
            .start()
            .await
            .expect("起動に失敗した");

        tokio::time::sleep(Duration::from_secs(1)).await;
        let before = lines
            .lock()
            .expect("処理に失敗しないこと")
            .iter()
            .filter(|l| l.contains("CONSUMER_BOOT_1"))
            .count();
        assert!(before >= 1, "初回起動のログを consumer が受け取ること");

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("停止に失敗した");
        container.start().await.expect("再起動に失敗した");

        // 再起動後のログ配信を待つ。
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let mut after = before;
        while std::time::Instant::now() < deadline {
            after = lines
                .lock()
                .expect("処理に失敗しないこと")
                .iter()
                .filter(|l| l.contains("CONSUMER_BOOT_1"))
                .count();
            if after > before {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        assert!(
            after > before,
            "再 start 後に consumer が再起動分のログを受け取ること: before={before} after={after}"
        );

        container.stop_with_timeout(Some(0)).await.ok();
        container.rm().await.ok();
    }
}
