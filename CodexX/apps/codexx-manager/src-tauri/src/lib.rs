pub mod commands;
pub mod install;

use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    RunEvent, WindowEvent,
};
const TRAY_MENU_HIDE: &str = "tray-hide";
const TRAY_MENU_QUICK_LAUNCH: &str = "tray-quick-launch";
const TRAY_MENU_QUIT: &str = "tray-quit";

pub fn run() {
    install_panic_logger();
    let _ = codexx_core::diagnostic_log::append_diagnostic_log(
        "manager.start",
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION")
        }),
    );
    let show_update = commands::startup_should_show_update();
    let first_run = commands::startup_is_first_run();
    let launch_panel = commands::startup_is_launch_panel();
    let run_result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = commands::show_main_window(app.clone());
        }))
        .setup(move |app| {
            let url = if launch_panel {
                "index.html?launchPanel=1"
            } else if first_run {
                "index.html?firstRun=1"
            } else if show_update {
                "index.html?showUpdate=1"
            } else {
                "index.html"
            };
            let main_window =
                tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App(url.into()))
                    .title(codexx_core::brand::WINDOW_TITLE)
                    .inner_size(1180.0, 820.0)
                    .min_inner_size(960.0, 720.0)
                    .build()?;
            register_main_window_events(main_window);
            configure_tray(app.handle().clone())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::backend_version,
            commands::startup_options,
            commands::show_main_window,
            commands::hide_main_window,
            commands::load_overview,
            commands::launch_codex_plus,
            commands::restart_codex_plus,
            commands::load_settings,
            commands::save_settings,
            commands::list_local_sessions,
            commands::list_zed_remote_projects,
            commands::open_zed_remote,
            commands::forget_zed_remote_project,
            commands::delete_local_session,
            commands::load_provider_sync_targets,
            commands::sync_providers_now,
            commands::refresh_script_market,
            commands::install_market_script,
            commands::set_user_script_enabled,
            commands::delete_user_script,
            commands::open_external_url,
            commands::install_entrypoints,
            commands::uninstall_entrypoints,
            commands::repair_shortcuts,
            commands::repair_backend,
            commands::check_update,
            commands::perform_update,
            commands::load_watcher_state,
            commands::install_watcher,
            commands::uninstall_watcher,
            commands::enable_watcher,
            commands::disable_watcher,
            commands::read_latest_logs,
            commands::copy_diagnostics,
            commands::reset_settings,
            commands::reset_image_overlay_settings,
            commands::relay_status,
            commands::read_relay_files,
            commands::check_env_conflicts,
            commands::remove_env_conflicts,
            commands::save_relay_file,
            commands::write_diagnostic_event,
            commands::backfill_relay_profile_from_live,
            commands::list_context_entries,
            commands::read_live_context_entries,
            commands::sync_live_context_entries,
            commands::upsert_context_entry,
            commands::delete_context_entry,
            commands::extract_relay_common_config,
            commands::test_relay_profile,
            commands::fetch_relay_profile_models,
            commands::switch_relay_profile,
            commands::quick_configure_token,
            commands::quick_switch_profile,
            commands::apply_relay_injection,
            commands::apply_pure_api_injection,
            commands::clear_relay_injection
        ])
        .build(tauri::generate_context!())
        .map(|app| {
            app.run(|app_handle, event| {
                #[cfg(target_os = "macos")]
                if matches!(event, RunEvent::Reopen { .. }) {
                    let _ = commands::show_main_window(app_handle.clone());
                }
            });
        });
    if let Err(error) = run_result {
        let _ = codexx_core::diagnostic_log::append_diagnostic_log(
            "manager.run_failed",
            serde_json::json!({
                "error": error.to_string()
            }),
        );
    }
}

fn configure_tray(app: tauri::AppHandle) -> tauri::Result<()> {
    let app_for_tray = app.clone();
    let menu = build_tray_menu(&app)?;
    let tray_icon = tray_icon_image(&app)?;
    let builder = TrayIconBuilder::with_id("main-tray")
        .icon(tray_icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TRAY_MENU_HIDE => {
                let _ = commands::hide_main_window(app.clone());
            }
            TRAY_MENU_QUICK_LAUNCH => {
                let _ = commands::show_main_window(app.clone());
            }
            TRAY_MENU_QUIT => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(move |_tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = commands::show_main_window(app_for_tray.clone());
            }
        });

    #[cfg(target_os = "macos")]
    let builder = builder.icon_as_template(true);

    builder.build(&app)?;
    Ok(())
}

fn build_tray_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let quick_launch_item =
        MenuItem::with_id(app, TRAY_MENU_QUICK_LAUNCH, "打开启动面板", true, None::<&str>)?;
    let hide_item = MenuItem::with_id(app, TRAY_MENU_HIDE, "隐藏启动面板", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, TRAY_MENU_QUIT, "退出", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    Menu::with_items(
        app,
        &[
            &quick_launch_item,
            &hide_item,
            &separator,
            &quit_item,
        ],
    )
}

fn tray_icon_image<'a>(app: &'a tauri::AppHandle) -> tauri::Result<Image<'a>> {
    #[cfg(target_os = "macos")]
    if let Ok(image) = Image::from_bytes(include_bytes!("../../../../assets/images/tray-template.png")) {
        return Ok(image);
    }
    #[cfg(windows)]
    if let Ok(image) = Image::from_bytes(include_bytes!("../../../../assets/images/tray-icon.ico")) {
        return Ok(image);
    }
    Ok(app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("缺少默认窗口图标"))?)
}

fn register_main_window_events<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
) {
    let event_window = window.clone();
    let close_window = event_window.clone();
    let minimized_window = event_window.clone();

    event_window.on_window_event(move |event| match event {
        WindowEvent::Resized(_) => {
            if matches!(minimized_window.is_minimized(), Ok(true)) {
                let _ = minimized_window.hide();
            }
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = close_window.hide();
        }
        _ => {}
    });
}

fn install_panic_logger() {
    std::panic::set_hook(Box::new(|panic_info| {
        let payload = panic_info
            .payload()
            .downcast_ref::<&str>()
            .map(|message| (*message).to_string())
            .or_else(|| panic_info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "非字符串 panic payload".to_string());
        let location = panic_info.location().map(|location| {
            serde_json::json!({
                "file": location.file(),
                "line": location.line(),
                "column": location.column()
            })
        });
        let _ = codexx_core::diagnostic_log::append_diagnostic_log(
            "manager.panic",
            serde_json::json!({
                "payload": payload,
                "location": location
            }),
        );
    }));
}
