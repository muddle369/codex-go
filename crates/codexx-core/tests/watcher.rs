use codexx_core::watcher::{
    build_spawn_launcher_command, build_watcher_install_plan, cdp_listening, codex_process_ids,
    disable_watcher_at, enable_watcher_at, filter_killable_launcher_processes,
    macos_codex_process_ids_for_debug_port, process_ids_still_running,
    should_recover_stale_launcher, watcher_disabled_flag,
};

#[test]
fn cdp_listening_returns_true_for_bound_loopback_port() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();

    assert!(cdp_listening(port));
}

#[test]
fn cdp_listening_returns_true_for_bound_ipv6_loopback_port() {
    let listener = std::net::TcpListener::bind("[::1]:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    assert!(cdp_listening(port));
}

#[test]
fn cdp_listening_returns_false_for_closed_port() {
    let port = {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    };

    assert!(!cdp_listening(port));
}

#[test]
fn watcher_enable_and_disable_toggle_flag() {
    let dir = tempfile::tempdir().unwrap();
    let flag = watcher_disabled_flag(dir.path());

    disable_watcher_at(dir.path()).unwrap();
    assert!(flag.exists());

    enable_watcher_at(dir.path()).unwrap();
    assert!(!flag.exists());
}

#[test]
fn watcher_install_plan_registers_rust_launcher_at_logon() {
    let plan = build_watcher_install_plan("C:/Tools/codexgo.exe".into(), 9333);

    assert_eq!(plan.run_value_name, "CodexGOWatcher");
    assert_eq!(plan.run_value, "\"C:/Tools/codexgo.exe\" --debug-port 9333");
    assert_eq!(plan.shortcut_name, "CodexGOWatcher.lnk");
    assert_eq!(plan.shortcut_target, "C:/Tools/codexgo.exe");
    assert_eq!(plan.shortcut_arguments, "--debug-port 9333");
}

#[test]
fn spawn_launcher_command_points_to_silent_binary_only() {
    let command = build_spawn_launcher_command("C:/Tools/codexgo.exe", 9444);

    assert_eq!(command[0], "C:/Tools/codexgo.exe");
    assert!(command.contains(&"--debug-port".to_string()));
    assert!(command.contains(&"9444".to_string()));
    assert!(!command.iter().any(|part| part.contains("manager")));
}

#[test]
fn codex_process_filter_keeps_only_windowsapps_codex_processes() {
    let processes = [
        (
            11,
            r"C:\Program Files\WindowsApps\OpenAI.Codex_1.0.0.0_x64__abc\app\Codex.exe",
        ),
        (12, r"C:\Tools\Codex.exe"),
        (
            13,
            r"C:\Program Files\WindowsApps\Other.App_1.0.0.0_x64__abc\app\Codex.exe",
        ),
    ];

    assert_eq!(codex_process_ids(processes), vec![11]);
}

#[test]
fn launcher_process_filter_protects_current_process_ancestry() {
    let processes = [
        (10, 0, "codexgo.exe"),
        (20, 10, "codexgo.exe"),
        (30, 20, "codexgo.exe"),
        (40, 10, "codexgo.exe"),
        (50, 10, "codexgo-manager.exe"),
    ];

    assert_eq!(filter_killable_launcher_processes(processes, 30), vec![40]);
}

#[test]
fn stale_launcher_recovery_only_runs_when_codex_and_cdp_are_absent() {
    assert!(should_recover_stale_launcher(false, false));
    assert!(!should_recover_stale_launcher(true, false));
    assert!(!should_recover_stale_launcher(false, true));
    assert!(!should_recover_stale_launcher(true, true));
}

#[test]
fn stop_wait_tracks_only_expected_process_ids() {
    assert_eq!(
        process_ids_still_running(&[10, 20, 30], [5, 20, 40, 30]),
        vec![20, 30]
    );
}

#[test]
fn macos_restart_targets_only_desktop_processes_using_the_selected_debug_port() {
    let processes = [
        "  101 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT --remote-debugging-port=9329",
        "  102 /Applications/ChatGPT.app/Contents/Frameworks/ChatGPT Helper.app/Contents/MacOS/ChatGPT Helper --remote-debugging-port=9329",
        "  103 /Applications/Codex.app/Contents/MacOS/Codex --remote-debugging-port=9229",
        "  104 /Applications/Codex.app/Contents/MacOS/Codex --remote-debugging-port=9329",
        "  104 /Applications/Codex.app/Contents/MacOS/Codex --remote-debugging-port=9329",
        "  105 /Applications/AI Tools/ChatGPT.app/Contents/MacOS/ChatGPT --remote-debugging-port=9329",
    ];

    assert_eq!(
        macos_codex_process_ids_for_debug_port(processes, 9329),
        vec![101, 104, 105]
    );
}
