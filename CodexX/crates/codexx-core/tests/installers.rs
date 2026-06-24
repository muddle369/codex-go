use codexx_core::install::{
    InstallOptions, SILENT_BINARY, app_bundle_names, build_macos_app_bundle,
    build_windows_entrypoint_plan, companion_binary_path_from_exe, default_install_root_strategy,
    shortcut_names,
};

#[test]
fn windows_entrypoint_plan_contains_single_visible_entrypoint_and_manager_sidecar() {
    let options = InstallOptions {
        install_root: Some("C:/Users/A/Desktop".into()),
        launcher_path: Some("C:/Tools/codexgo.exe".into()),
        manager_path: Some("C:/Tools/codexgo-manager.exe".into()),
        remove_owned_data: false,
    };

    let plan = build_windows_entrypoint_plan(&options);

    assert!(plan.silent_shortcut.ends_with("CodexGO.lnk"));
    assert_eq!(plan.launcher_path, "C:/Tools/codexgo.exe");
    assert_eq!(plan.manager_path, "C:/Tools/codexgo-manager.exe");
    assert_eq!(plan.silent_icon_path, "C:/Tools/codexgo.exe");
    assert_eq!(plan.uninstall_key, "CodexGO");
    assert_eq!(plan.legacy_uninstall_key, "CodexX");
    assert_eq!(
        plan.uninstaller_path.replace('\\', "/"),
        "C:/Tools/uninstall.exe"
    );
    assert_eq!(
        plan.uninstall_command.replace('\\', "/"),
        "\"C:/Tools/uninstall.exe\""
    );
    assert_eq!(
        plan.quiet_uninstall_command.replace('\\', "/"),
        "\"C:/Tools/uninstall.exe\" /S"
    );
    assert_ne!(plan.uninstall_command, "\"C:/Tools/codexgo-manager.exe\"");
}

#[test]
fn windows_entrypoint_plan_can_request_owned_data_removal_without_shell_script() {
    let options = InstallOptions {
        install_root: Some("C:/Users/A/Desktop".into()),
        launcher_path: None,
        manager_path: None,
        remove_owned_data: true,
    };

    let plan = build_windows_entrypoint_plan(&options);

    assert!(plan.silent_shortcut.ends_with("CodexGO.lnk"));
    assert!(plan.remove_owned_data);
}

#[test]
fn macos_bundle_metadata_uses_single_visible_app_with_manager_sidecar() {
    let options = InstallOptions {
        install_root: Some("/Applications".into()),
        launcher_path: Some("/opt/CodexGO/codexgo".into()),
        manager_path: Some("/opt/CodexGO/codexgo-manager".into()),
        remove_owned_data: false,
    };

    let silent = build_macos_app_bundle(&options, false);

    assert!(silent.app_path.ends_with("CodexGO.app"));
    assert!(silent.info_plist.contains("<string>CodexGO</string>"));
    assert!(silent.launch_script.contains("codexgo"));
}

#[test]
fn installer_exports_expected_single_entrypoint_name() {
    assert_eq!(shortcut_names(), ("CodexGO.lnk", ""));
    assert_eq!(app_bundle_names(), ("CodexGO.app", ""));
}

#[test]
fn macos_dmg_includes_applications_shortcut_for_drag_install() {
    let script = std::fs::read_to_string("../../scripts/installer/macos/package-dmg.sh")
        .expect("read macOS DMG packaging script");

    assert!(script.contains("ln -s /Applications \"$STAGE/Applications\""));
}

#[test]
fn companion_binary_path_prefers_same_app_sidecar() {
    let manager_exe =
        std::path::Path::new("/Applications/CodexGO.app/Contents/MacOS/codexgo-manager");

    let companion = companion_binary_path_from_exe(manager_exe, SILENT_BINARY);

    assert_eq!(
        companion,
        std::path::PathBuf::from("/Applications/CodexGO.app/Contents/MacOS/CodexGO")
    );
}

#[test]
fn macos_bundle_does_not_wrap_the_bundle_executable_in_itself() {
    let options = InstallOptions {
        install_root: Some("/Applications".into()),
        launcher_path: Some("/Applications/CodexGO.app/Contents/MacOS/CodexGO".into()),
        manager_path: Some("/Applications/CodexGO.app/Contents/MacOS/codexgo-manager".into()),
        remove_owned_data: false,
    };

    let silent = build_macos_app_bundle(&options, false);

    assert!(!silent.launch_script.contains("CodexGO\""));
    assert!(silent.launch_script.contains("codexgo"));
}

#[test]
fn windows_default_install_root_uses_known_folder_before_userprofile_desktop() {
    let strategy = default_install_root_strategy();

    if cfg!(windows) {
        assert_eq!(strategy, "windows-known-folder");
    } else if cfg!(target_os = "macos") {
        assert_eq!(strategy, "macos-applications");
    } else {
        assert_eq!(strategy, "user-dirs-desktop");
    }
}
