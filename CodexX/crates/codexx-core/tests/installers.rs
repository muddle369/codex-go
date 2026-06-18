use codexx_core::install::{
    InstallOptions, SILENT_BINARY, app_bundle_names, build_macos_app_bundle,
    build_windows_entrypoint_plan, companion_binary_path_from_exe, default_install_root_strategy,
    shortcut_names,
};

#[test]
fn windows_entrypoint_plan_contains_silent_and_manager_entrypoints() {
    let options = InstallOptions {
        install_root: Some("C:/Users/A/Desktop".into()),
        launcher_path: Some("C:/Tools/codexx.exe".into()),
        manager_path: Some("C:/Tools/codexx-manager.exe".into()),
        remove_owned_data: false,
    };

    let plan = build_windows_entrypoint_plan(&options);

    assert!(plan.silent_shortcut.ends_with("CodexX.lnk"));
    assert!(plan.manager_shortcut.ends_with("CodexX Manager.lnk"));
    assert_eq!(plan.launcher_path, "C:/Tools/codexx.exe");
    assert_eq!(plan.manager_path, "C:/Tools/codexx-manager.exe");
    assert_eq!(plan.silent_icon_path, "C:/Tools/codexx.exe");
    assert_eq!(plan.manager_icon_path, "C:/Tools/codexx-manager.exe");
    assert_eq!(plan.uninstall_key, "CodexX");
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
    assert_ne!(plan.uninstall_command, "\"C:/Tools/codexx-manager.exe\"");
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

    assert!(plan.silent_shortcut.ends_with("CodexX.lnk"));
    assert!(plan.manager_shortcut.ends_with("CodexX Manager.lnk"));
    assert!(plan.remove_owned_data);
}

#[test]
fn macos_bundle_metadata_contains_silent_and_manager_apps() {
    let options = InstallOptions {
        install_root: Some("/Applications".into()),
        launcher_path: Some("/opt/CodexX/codexx".into()),
        manager_path: Some("/opt/CodexX/codexx-manager".into()),
        remove_owned_data: false,
    };

    let silent = build_macos_app_bundle(&options, false);
    let manager = build_macos_app_bundle(&options, true);

    assert!(silent.app_path.ends_with("CodexX.app"));
    assert!(manager.app_path.ends_with("CodexX Manager.app"));
    assert!(silent.info_plist.contains("<string>CodexX</string>"));
    assert!(
        manager
            .info_plist
            .contains("<string>CodexX Manager</string>")
    );
    assert!(silent.launch_script.contains("codexx"));
    assert!(manager.launch_script.contains("codexx-manager"));
}

#[test]
fn installer_exports_expected_two_entrypoint_names() {
    assert_eq!(shortcut_names(), ("CodexX.lnk", "CodexX Manager.lnk"));
    assert_eq!(app_bundle_names(), ("CodexX.app", "CodexX Manager.app"));
}

#[test]
fn macos_dmg_includes_applications_shortcut_for_drag_install() {
    let script = std::fs::read_to_string("../../scripts/installer/macos/package-dmg.sh")
        .expect("read macOS DMG packaging script");

    assert!(script.contains("ln -s /Applications \"$STAGE/Applications\""));
}

#[test]
fn companion_binary_path_resolves_macos_silent_app_next_to_manager_app() {
    let manager_exe =
        std::path::Path::new("/Applications/CodexX Manager.app/Contents/MacOS/CodexXManager");

    let companion = companion_binary_path_from_exe(manager_exe, SILENT_BINARY);

    assert_eq!(
        companion,
        std::path::PathBuf::from("/Applications/CodexX.app/Contents/MacOS/CodexX")
    );
    assert_ne!(
        companion,
        std::path::PathBuf::from("/Applications/CodexX Manager.app/Contents/MacOS/codexx")
    );
}

#[test]
fn macos_bundle_does_not_wrap_the_bundle_executable_in_itself() {
    let options = InstallOptions {
        install_root: Some("/Applications".into()),
        launcher_path: Some("/Applications/CodexX.app/Contents/MacOS/CodexX".into()),
        manager_path: Some("/Applications/CodexX Manager.app/Contents/MacOS/CodexXManager".into()),
        remove_owned_data: false,
    };

    let silent = build_macos_app_bundle(&options, false);
    let manager = build_macos_app_bundle(&options, true);

    assert!(!silent.launch_script.contains("CodexX\""));
    assert!(!manager.launch_script.contains("CodexXManager\""));
    assert!(silent.launch_script.contains("codexx"));
    assert!(manager.launch_script.contains("codexx-manager"));
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
