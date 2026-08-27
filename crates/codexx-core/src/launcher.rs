use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::settings::{BackendSettings, SettingsStore, normalize_codex_extra_args};
use crate::status::{LaunchStatus, StatusStore};

#[cfg(windows)]
const POST_LAUNCH_COMPUTER_USE_GUARD_SECONDS: &[u64] = &[0, 5, 15, 30, 60, 120, 180, 240, 300];
#[cfg_attr(not(windows), allow(dead_code))]
const POST_LAUNCH_COMPUTER_USE_GUARD_STABLE_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexLaunch {
    Process {
        command: Vec<String>,
        wait_strategy: ProcessWaitStrategy,
        macos_cleanup_policy: Option<MacosCleanupPolicy>,
    },
    PackagedActivation {
        app_user_model_id: String,
        arguments: String,
        process_id: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessWaitStrategy {
    TrackedChild,
    ExternalWaitCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosCleanupPolicy {
    QuitIfNotPreviouslyRunning,
    SkipQuitBecauseAlreadyRunning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsProcessControlStrategy {
    NativeWindowsApi,
}

#[cfg(windows)]
pub fn windows_process_control_strategy() -> WindowsProcessControlStrategy {
    WindowsProcessControlStrategy::NativeWindowsApi
}

impl CodexLaunch {
    pub fn process_id(&self) -> Option<u32> {
        match self {
            Self::PackagedActivation { process_id, .. } => *process_id,
            Self::Process { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub app_dir: Option<PathBuf>,
    pub debug_port: u16,
    pub helper_port: u16,
    pub status_store: StatusStore,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            app_dir: None,
            debug_port: 9329,
            helper_port: 58321,
            status_store: StatusStore::default(),
        }
    }
}

#[derive(Clone)]
pub struct LaunchHandle {
    pub debug_port: u16,
    pub helper_port: u16,
    pub app_dir: PathBuf,
    pub launch: CodexLaunch,
    pub status_store: StatusStore,
    helper_started: bool,
    hooks: Arc<dyn LaunchHooks>,
}

impl std::fmt::Debug for LaunchHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LaunchHandle")
            .field("debug_port", &self.debug_port)
            .field("helper_port", &self.helper_port)
            .field("app_dir", &self.app_dir)
            .field("launch", &self.launch)
            .field("status_store", &self.status_store)
            .finish_non_exhaustive()
    }
}

impl LaunchHandle {
    pub async fn wait_for_codex_exit(&self) -> anyhow::Result<()> {
        let result = self.hooks.wait_for_codex_exit(&self.launch).await;
        if self.helper_started {
            self.hooks.shutdown_helper(self.helper_port).await;
        }
        result
    }
}

#[async_trait(?Send)]
pub trait LaunchHooks: Send + Sync {
    fn resolve_app_dir(
        &self,
        app_dir: Option<&Path>,
        settings: &BackendSettings,
    ) -> anyhow::Result<PathBuf>;
    fn select_debug_port(&self, requested: u16) -> u16;
    fn select_helper_port(&self, requested: u16) -> u16;
    async fn load_settings(&self) -> anyhow::Result<BackendSettings>;
    async fn run_provider_sync(&self) -> anyhow::Result<()>;
    async fn apply_active_relay_profile(&self, _settings: &BackendSettings) -> anyhow::Result<()> {
        Ok(())
    }
    async fn ensure_computer_use_config(&self, _settings: &BackendSettings) -> anyhow::Result<()> {
        Ok(())
    }
    async fn start_helper(&self, helper_port: u16) -> anyhow::Result<()>;
    async fn launch_codex(
        &self,
        app_dir: &Path,
        debug_port: u16,
        extra_args: &[String],
    ) -> anyhow::Result<CodexLaunch>;
    async fn bridge_context(
        &self,
        _debug_port: u16,
        _app_dir: &Path,
    ) -> anyhow::Result<Option<crate::routes::BridgeContext>> {
        Ok(None)
    }
    async fn inject(&self, debug_port: u16, helper_port: u16) -> anyhow::Result<()>;
    async fn inject_bridge(
        &self,
        debug_port: u16,
        helper_port: u16,
        _ctx: crate::routes::BridgeContext,
    ) -> anyhow::Result<()> {
        self.inject(debug_port, helper_port).await
    }
    async fn ensure_injection(&self, debug_port: u16, helper_port: u16, app_dir: &Path) -> bool {
        for attempt in 1..=120 {
            let result = match self.bridge_context(debug_port, app_dir).await {
                Ok(Some(ctx)) => self.inject_bridge(debug_port, helper_port, ctx).await,
                Ok(None) => self.inject(debug_port, helper_port).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => return true,
                Err(error) => {
                    let _ = crate::diagnostic_log::append_diagnostic_log(
                        "launcher.ensure_injection_retry_failed",
                        serde_json::json!({
                            "debug_port": debug_port,
                            "helper_port": helper_port,
                            "attempt": attempt,
                            "message": error.to_string()
                        }),
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
        false
    }
    async fn start_bridge_watchdog(
        &self,
        _debug_port: u16,
        _helper_port: u16,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    async fn start_computer_use_guard_watchdog(
        &self,
        _settings: &BackendSettings,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    async fn write_status(&self, status: &str);
    async fn wait_for_codex_exit(&self, launch: &CodexLaunch) -> anyhow::Result<()>;
    async fn shutdown_helper(&self, helper_port: u16);
    async fn terminate_codex(&self, launch: &CodexLaunch);
}

#[derive(Default)]
pub struct DefaultLaunchHooks {
    child: Mutex<Option<Child>>,
    helper: Mutex<Option<HelperRuntime>>,
    bridge_watchdog: Mutex<Option<BridgeWatchdogRuntime>>,
    computer_use_guard_watchdog: Mutex<Option<ComputerUseGuardWatchdogRuntime>>,
    computer_use_guard_artifacts: Mutex<Option<crate::computer_use_guard::GuardArtifacts>>,
}

struct HelperRuntime {
    shutdown: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

struct BridgeWatchdogRuntime {
    shutdown: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

struct ComputerUseGuardWatchdogRuntime {
    shutdown: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

pub async fn launch_and_inject(options: LaunchOptions) -> anyhow::Result<LaunchHandle> {
    launch_and_inject_with_hooks(options, DefaultLaunchHooks::shared()).await
}

pub async fn launch_and_inject_with_hooks<H>(
    options: LaunchOptions,
    hooks: H,
) -> anyhow::Result<LaunchHandle>
where
    H: IntoLaunchHooks,
{
    let hooks = hooks.into_launch_hooks();
    let debug_port = hooks.select_debug_port(options.debug_port);
    let mut helper_port = hooks.select_helper_port(options.helper_port);
    let settings = hooks.load_settings().await?;
    let app_dir = hooks.resolve_app_dir(options.app_dir.as_deref(), &settings)?;
    let status_store = options.status_store.clone();
    let mut helper_started = false;
    let mut launched = None;
    let mut keep_launched_on_error = false;

    let result: anyhow::Result<LaunchHandle> = async {
        let home = crate::relay_config::default_codex_home_dir();
        if settings.provider_sync_enabled {
            crate::codex_app_state::capture_app_state_snapshot_nonfatal(&home, "launcher.before");
            hooks.run_provider_sync().await?;
            crate::codex_app_state::sync_app_state_after_provider_switch_nonfatal(
                &home,
                "launcher.after_provider_sync",
            );
        }
        if settings.computer_use_guard_enabled {
            hooks.ensure_computer_use_config(&settings).await?;
        }
        let protocol_proxy_enabled = relay_protocol_proxy_enabled(&settings)
            || remote_control_provider_proxy_enabled(&settings);
        if protocol_proxy_enabled {
            helper_port = crate::protocol_proxy::DEFAULT_PROTOCOL_PROXY_PORT;
        }
        if settings.enhancements_enabled || protocol_proxy_enabled {
            hooks.start_helper(helper_port).await?;
            helper_started = true;
        }

        if settings.enhancements_enabled {
            crate::codex_app_state::prepare_projectless_main_window_nonfatal(
                &home,
                "launcher.prelaunch",
            );
        }
        let launch = hooks
            .launch_codex(&app_dir, debug_port, &settings.codex_extra_args)
            .await?;
        launched = Some(launch.clone());
        keep_launched_on_error = true;
        if settings.computer_use_guard_enabled {
            hooks.start_computer_use_guard_watchdog(&settings).await?;
        }

        let mut injection_degraded = false;
        if settings.enhancements_enabled {
            let injection_ready = hooks
                .ensure_injection(debug_port, helper_port, &app_dir)
                .await;
            if injection_ready {
                keep_launched_on_error = false;
                hooks.start_bridge_watchdog(debug_port, helper_port).await?;
            } else {
                let degraded = launch_status(
                    "running_degraded",
                    "Codex launched; CodexX enhancements are still waiting for the page bridge.",
                    debug_port,
                    helper_port,
                    &app_dir,
                );
                options.status_store.save_latest(&degraded)?;
                hooks.write_status("running_degraded").await;
                injection_degraded = true;
            }
        }

        if !settings.enhancements_enabled || !injection_degraded {
            let status = launch_status(
                "running",
                "CodexX launcher ready",
                debug_port,
                helper_port,
                &app_dir,
            );
            options.status_store.save_latest(&status)?;
            hooks.write_status("running").await;
        }

        Ok(LaunchHandle {
            debug_port,
            helper_port,
            app_dir: app_dir.clone(),
            launch,
            status_store: status_store.clone(),
            helper_started,
            hooks: Arc::clone(&hooks),
        })
    }
    .await;

    match result {
        Ok(handle) => Ok(handle),
        Err(error) => {
            if helper_started {
                hooks.shutdown_helper(helper_port).await;
            }
            if let Some(launch) = &launched {
                if !keep_launched_on_error {
                    hooks.terminate_codex(launch).await;
                }
            }
            let message = error.to_string();
            let failure = launch_status("failed", &message, debug_port, helper_port, &app_dir);
            let _ = status_store.save_latest(&failure);
            hooks.write_status("failed").await;
            Err(error)
        }
    }
}

fn relay_protocol_proxy_enabled(settings: &BackendSettings) -> bool {
    settings.active_relay_uses_protocol_proxy()
}

fn remote_control_provider_proxy_enabled(settings: &BackendSettings) -> bool {
    if !settings.relay_profiles_enabled {
        return false;
    }
    let profile = settings.active_relay_profile();
    profile.relay_mode == crate::settings::RelayMode::Official && profile.official_mix_api_key
}

pub trait IntoLaunchHooks {
    fn into_launch_hooks(self) -> Arc<dyn LaunchHooks>;
}

impl<T> IntoLaunchHooks for &T
where
    T: LaunchHooks + Clone + 'static,
{
    fn into_launch_hooks(self) -> Arc<dyn LaunchHooks> {
        Arc::new(self.clone())
    }
}

impl IntoLaunchHooks for Arc<dyn LaunchHooks> {
    fn into_launch_hooks(self) -> Arc<dyn LaunchHooks> {
        self
    }
}

impl IntoLaunchHooks for DefaultLaunchHooks {
    fn into_launch_hooks(self) -> Arc<dyn LaunchHooks> {
        Arc::new(self)
    }
}

impl DefaultLaunchHooks {
    pub fn shared() -> Arc<dyn LaunchHooks> {
        Arc::new(Self::default())
    }
}

#[async_trait(?Send)]
impl LaunchHooks for DefaultLaunchHooks {
    fn resolve_app_dir(
        &self,
        app_dir: Option<&Path>,
        settings: &BackendSettings,
    ) -> anyhow::Result<PathBuf> {
        crate::app_paths::resolve_codex_app_dir_with_saved(
            app_dir,
            Some(settings.codex_app_path.as_str()),
        )
        .ok_or_else(|| anyhow::anyhow!("Codex App directory not found"))
    }

    fn select_debug_port(&self, requested: u16) -> u16 {
        crate::ports::select_packaged_codex_debug_port(requested)
    }

    fn select_helper_port(&self, requested: u16) -> u16 {
        crate::ports::select_platform_loopback_port(requested)
    }

    async fn load_settings(&self) -> anyhow::Result<BackendSettings> {
        SettingsStore::default().load()
    }

    async fn run_provider_sync(&self) -> anyhow::Result<()> {
        anyhow::bail!("provider sync requires launcher hooks with codexx-data integration")
    }

    async fn apply_active_relay_profile(&self, settings: &BackendSettings) -> anyhow::Result<()> {
        if !settings.relay_profiles_enabled {
            return Ok(());
        }
        let profile = settings.active_relay_profile();
        let home = crate::relay_config::default_codex_home_dir();
        let common_config = crate::relay_config::normalize_config_text(
            &[
                settings.relay_common_config_contents.as_str(),
                settings.relay_context_config_contents.as_str(),
            ]
            .into_iter()
            .map(str::trim)
            .filter(|section| !section.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        );
        if profile.relay_mode == crate::settings::RelayMode::Official
            && !profile.official_mix_api_key
        {
            let auth_contents = (!profile.auth_contents.trim().is_empty())
                .then_some(profile.auth_contents.as_str());
            crate::relay_config::clear_relay_config_to_home_with_auth_and_computer_use_guard(
                &home,
                auth_contents,
                settings.computer_use_guard_enabled,
            )?;
            return Ok(());
        }
        crate::relay_config::apply_relay_profile_to_home_with_switch_rules_and_computer_use_guard(
            &home,
            &profile,
            &common_config,
            settings.computer_use_guard_enabled,
        )?;
        Ok(())
    }

    async fn ensure_computer_use_config(&self, settings: &BackendSettings) -> anyhow::Result<()> {
        if !settings.computer_use_guard_enabled {
            return Ok(());
        }
        let home = crate::relay_config::default_codex_home_dir();
        let artifacts = crate::computer_use_guard::resolve_computer_use_guard_artifacts(&home)?;
        crate::computer_use_guard::ensure_computer_use_config_with_artifacts(&home, &artifacts)?;
        *self.computer_use_guard_artifacts.lock().await = Some(artifacts);
        Ok(())
    }

    async fn start_helper(&self, helper_port: u16) -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", helper_port))
            .await
            .with_context(|| format!("failed to bind helper runtime on 127.0.0.1:{helper_port}"))?;
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "helper.listening",
            serde_json::json!({
                "helper_port": helper_port,
                "address": format!("http://127.0.0.1:{helper_port}")
            }),
        );
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        if let Ok((stream, addr)) = accepted {
                            tokio::spawn(async move {
                                let _ = handle_helper_connection(stream, Some(addr)).await;
                            });
                        }
                    }
                }
            }
        });
        *self.helper.lock().await = Some(HelperRuntime {
            shutdown: shutdown_tx,
            task,
        });
        Ok(())
    }

    async fn launch_codex(
        &self,
        app_dir: &Path,
        debug_port: u16,
        extra_args: &[String],
    ) -> anyhow::Result<CodexLaunch> {
        if cfg!(windows) {
            if let Some(activation) = build_packaged_activation(app_dir, debug_port, extra_args) {
                let CodexLaunch::PackagedActivation {
                    app_user_model_id,
                    arguments,
                    ..
                } = &activation
                else {
                    unreachable!();
                };
                let process_id = activate_packaged_app(app_user_model_id, arguments).await?;
                return Ok(match activation {
                    CodexLaunch::PackagedActivation {
                        app_user_model_id,
                        arguments,
                        ..
                    } => CodexLaunch::PackagedActivation {
                        app_user_model_id,
                        arguments,
                        process_id: Some(process_id),
                    },
                    CodexLaunch::Process { .. } => unreachable!(),
                });
            }
        }

        if app_dir.extension().and_then(|value| value.to_str()) == Some("app") {
            let cleanup_policy = if is_macos_app_running(app_dir).await {
                MacosCleanupPolicy::SkipQuitBecauseAlreadyRunning
            } else {
                MacosCleanupPolicy::QuitIfNotPreviouslyRunning
            };
            let command = build_macos_open_command(app_dir, debug_port, extra_args);
            let executable = command
                .first()
                .ok_or_else(|| anyhow::anyhow!("macOS open command is empty"))?;
            let child = Command::new(executable)
                .args(&command[1..])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("failed to launch macOS Codex app")?;
            *self.child.lock().await = Some(child);
            return Ok(CodexLaunch::Process {
                command,
                wait_strategy: ProcessWaitStrategy::ExternalWaitCommand,
                macos_cleanup_policy: Some(cleanup_policy),
            });
        }

        let command = build_codex_command(app_dir, debug_port, extra_args);
        let executable = command
            .first()
            .ok_or_else(|| anyhow::anyhow!("Codex command is empty"))?;
        let mut child_command = Command::new(executable);
        child_command
            .args(&command[1..])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        child_command.creation_flags(crate::windows_integration::CREATE_NO_WINDOW);
        let child = child_command
            .spawn()
            .with_context(|| format!("failed to launch Codex executable {executable}"))?;
        *self.child.lock().await = Some(child);
        Ok(CodexLaunch::Process {
            command,
            wait_strategy: ProcessWaitStrategy::TrackedChild,
            macos_cleanup_policy: None,
        })
    }

    async fn inject(&self, debug_port: u16, helper_port: u16) -> anyhow::Result<()> {
        retry_injection(debug_port, helper_port).await
    }

    async fn start_bridge_watchdog(&self, debug_port: u16, helper_port: u16) -> anyhow::Result<()> {
        let (shutdown, mut shutdown_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let mut observed_browser_id: Option<String> = None;
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    _ = interval.tick() => {
                        let current_browser_id = match crate::cdp::browser_identity(debug_port).await {
                            Ok(identity) => identity.browser_id().ok(),
                            Err(_) => None,
                        };
                        let identity_changed = current_browser_id
                            .as_deref()
                            .is_some_and(|current| browser_identity_changed(observed_browser_id.as_deref(), current));
                        if let Some(current) = current_browser_id {
                            observed_browser_id = Some(current);
                        }
                        let _ = check_and_reinject_bridge_inner(debug_port, helper_port, identity_changed).await;
                    }
                }
            }
        });
        if let Some(runtime) = self
            .bridge_watchdog
            .lock()
            .await
            .replace(BridgeWatchdogRuntime { shutdown, task })
        {
            let _ = runtime.shutdown.send(());
            let _ = runtime.task.await;
        }
        Ok(())
    }

    async fn start_computer_use_guard_watchdog(
        &self,
        settings: &BackendSettings,
    ) -> anyhow::Result<()> {
        if !settings.computer_use_guard_enabled {
            return Ok(());
        }
        #[cfg(windows)]
        {
            let home = crate::relay_config::default_codex_home_dir();
            let artifacts = self.computer_use_guard_artifacts.lock().await.clone();
            let (shutdown, mut shutdown_rx) = tokio::sync::oneshot::channel();
            let task = tokio::spawn(async move {
                run_post_launch_computer_use_guard(home, artifacts, &mut shutdown_rx).await;
            });
            if let Some(runtime) = self
                .computer_use_guard_watchdog
                .lock()
                .await
                .replace(ComputerUseGuardWatchdogRuntime { shutdown, task })
            {
                let _ = runtime.shutdown.send(());
                let _ = runtime.task.await;
            }
        }
        Ok(())
    }

    async fn write_status(&self, _status: &str) {}

    async fn wait_for_codex_exit(&self, launch: &CodexLaunch) -> anyhow::Result<()> {
        match launch {
            CodexLaunch::Process { .. } => {
                if let Some(mut child) = self.child.lock().await.take() {
                    let _ = child.wait().await;
                }
            }
            CodexLaunch::PackagedActivation { process_id, .. } => {
                if let Some(process_id) = process_id {
                    wait_for_windows_process_id(*process_id).await?;
                }
            }
        }
        let mut empty_streak = 0u32;
        loop {
            if crate::watcher::find_codex_processes().is_empty() {
                empty_streak = empty_streak.saturating_add(1);
                if empty_streak >= 3 {
                    break;
                }
            } else {
                empty_streak = 0;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        Ok(())
    }

    async fn shutdown_helper(&self, _helper_port: u16) {
        if let Some(runtime) = self.computer_use_guard_watchdog.lock().await.take() {
            let _ = runtime.shutdown.send(());
            let _ = runtime.task.await;
        }
        if let Some(runtime) = self.bridge_watchdog.lock().await.take() {
            let _ = runtime.shutdown.send(());
            let _ = runtime.task.await;
        }
        if let Some(runtime) = self.helper.lock().await.take() {
            let _ = runtime.shutdown.send(());
            let _ = runtime.task.await;
        }
    }

    async fn terminate_codex(&self, launch: &CodexLaunch) {
        match launch {
            CodexLaunch::Process {
                wait_strategy: ProcessWaitStrategy::ExternalWaitCommand,
                command,
                macos_cleanup_policy,
            } => {
                if let Some(mut child) = self.child.lock().await.take() {
                    let _ = child.kill().await;
                }
                if let (Some(app_dir), Some(cleanup_policy)) = (
                    macos_app_dir_from_open_command(command),
                    *macos_cleanup_policy,
                ) {
                    let _ = run_macos_cleanup_command(&app_dir, cleanup_policy).await;
                }
            }
            CodexLaunch::Process { .. } => {
                if let Some(mut child) = self.child.lock().await.take() {
                    let _ = child.kill().await;
                }
            }
            CodexLaunch::PackagedActivation {
                process_id: Some(process_id),
                ..
            } => {
                let _ = terminate_windows_process_id(*process_id).await;
            }
            CodexLaunch::PackagedActivation {
                process_id: None, ..
            } => {}
        }
    }
}

async fn handle_helper_connection(
    mut stream: tokio::net::TcpStream,
    remote_addr: Option<SocketAddr>,
) -> anyhow::Result<()> {
    let request = match read_http_request(&mut stream).await {
        Ok(request) => request,
        Err(error) => {
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "helper.request_parse_failed",
                serde_json::json!({
                    "status": error.status(),
                    "error": error.to_string(),
                    "remote_addr": remote_addr.map(|addr| addr.to_string())
                }),
            );
            let body = serde_json::to_vec(&serde_json::json!({
                "status": "failed",
                "message": error.to_string()
            }))?;
            write_http_response(
                &mut stream,
                error.status(),
                "application/json; charset=utf-8",
                &body,
            )
            .await?;
            stream.shutdown().await?;
            return Ok(());
        }
    };
    let request_headers = String::from_utf8_lossy(&request.headers);
    let request_body_bytes = &request.body;
    let request_body = String::from_utf8_lossy(request_body_bytes);
    let request_line = request_headers.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let raw_path = parts.next().unwrap_or_default();
    let path = raw_path.split('?').next().unwrap_or(raw_path);
    let request_user_agent = header_value_from_headers(&request_headers, "user-agent");
    let request_content_type = header_value_from_headers(&request_headers, "content-type");
    let request_audio_language =
        header_value_from_headers(&request_headers, "x-codexgo-audio-language");
    let remote_addr_text = remote_addr.map(|addr| addr.to_string());

    let _ = crate::diagnostic_log::append_diagnostic_log(
        "helper.request",
        serde_json::json!({
            "method": method,
            "path": path,
            "request_line": request_line,
            "remote_addr": remote_addr_text,
            "body_bytes": request_body_bytes.len()
        }),
    );

    if crate::protocol_proxy::is_audio_transcriptions_proxy_path(path) && method == "POST" {
        return handle_audio_transcriptions_proxy_connection(
            &mut stream,
            request_body_bytes,
            request_content_type.as_deref(),
            request_user_agent.as_deref(),
            request_audio_language.as_deref(),
            method,
            path,
            remote_addr_text,
        )
        .await;
    }

    if crate::protocol_proxy::is_responses_proxy_path(path) && method == "POST" {
        return handle_protocol_proxy_connection(
            &mut stream,
            request_body.as_ref(),
            request_user_agent.as_deref(),
            method,
            path,
            remote_addr_text,
        )
        .await;
    }
    if crate::protocol_proxy::is_chat_completions_proxy_path(path) && method == "POST" {
        return handle_chat_completions_proxy_connection(
            &mut stream,
            request_body.as_ref(),
            request_user_agent.as_deref(),
            method,
            path,
            remote_addr_text,
        )
        .await;
    }
    if crate::protocol_proxy::is_models_proxy_path(path) && matches!(method, "GET" | "OPTIONS") {
        return handle_models_proxy_connection(
            &mut stream,
            request_user_agent.as_deref(),
            method,
            path,
            remote_addr_text,
        )
        .await;
    }

    let (status, body, content_type, log_event) =
        if matches!(path, "/backend/status" | "/backend/repair")
            && matches!(method, "GET" | "POST" | "OPTIONS")
        {
            (
                "200 OK".to_string(),
                serde_json::to_vec(&serde_json::json!({
                    "status": "ok",
                    "message": "后端已连接",
                    "version": crate::version::VERSION,
                    "transport": "http-helper"
                }))?,
                "application/json; charset=utf-8".to_string(),
                if path == "/backend/status" {
                    "helper.backend_status_ok"
                } else {
                    "helper.backend_repair_ok"
                },
            )
        } else if path == "/diagnostics/log" && matches!(method, "POST" | "OPTIONS") {
            if method == "POST" {
                let detail = serde_json::from_str::<serde_json::Value>(request_body.as_ref())
                    .unwrap_or_else(|error| {
                        serde_json::json!({
                            "parse_error": error.to_string(),
                            "raw": request_body.as_ref()
                        })
                    });
                let event = detail
                    .get("event")
                    .and_then(serde_json::Value::as_str)
                    .map(sanitize_diagnostic_event)
                    .unwrap_or_else(|| "event".to_string());
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    &format!("renderer.{event}"),
                    detail,
                );
            }
            (
                "200 OK".to_string(),
                serde_json::to_vec(&serde_json::json!({
                    "status": "ok",
                    "message": "日志已记录"
                }))?,
                "application/json; charset=utf-8".to_string(),
                "helper.diagnostics_log_ok",
            )
        } else if path == "/overlay/image" && matches!(method, "GET" | "OPTIONS") {
            if method == "OPTIONS" {
                (
                    "200 OK".to_string(),
                    Vec::new(),
                    "application/octet-stream".to_string(),
                    "helper.overlay_image_options",
                )
            } else {
                overlay_image_response()
            }
        } else {
            (
                "404 Not Found".to_string(),
                serde_json::to_vec(&serde_json::json!({
                    "status": "failed",
                    "message": "未知后端路径"
                }))?,
                "application/json; charset=utf-8".to_string(),
                "helper.unknown_path",
            )
        };
    let _ = crate::diagnostic_log::append_diagnostic_log(
        log_event,
        serde_json::json!({
            "method": method,
            "path": path,
            "status": status,
            "remote_addr": remote_addr_text
        }),
    );
    let response = if method == "OPTIONS" {
        format!(
            "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Authorization\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    } else {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Authorization\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
    };
    stream.write_all(response.as_bytes()).await?;
    if method != "OPTIONS" {
        stream.write_all(&body).await?;
    }
    stream.shutdown().await?;
    Ok(())
}

fn overlay_image_response() -> (String, Vec<u8>, String, &'static str) {
    let not_found = || {
        (
            "404 Not Found".to_string(),
            serde_json::to_vec(&serde_json::json!({
                "status": "failed",
                "message": "图片覆盖层未启用或图片不可用"
            }))
            .unwrap_or_default(),
            "application/json; charset=utf-8".to_string(),
            "helper.overlay_image_not_found",
        )
    };
    let settings = SettingsStore::default().load().unwrap_or_default();
    if !settings.codex_app_image_overlay_enabled {
        return not_found();
    }
    let image_path = PathBuf::from(settings.codex_app_image_overlay_path.trim());
    if image_path.as_os_str().is_empty() || !image_path.is_file() {
        return not_found();
    }
    let Some(content_type) = overlay_image_content_type(&image_path) else {
        return not_found();
    };
    match std::fs::read(&image_path) {
        Ok(bytes) => (
            "200 OK".to_string(),
            bytes,
            content_type.to_string(),
            "helper.overlay_image_ok",
        ),
        Err(_) => not_found(),
    }
}

fn overlay_image_content_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        Some("bmp") => Some("image/bmp"),
        _ => None,
    }
}

async fn handle_models_proxy_connection(
    stream: &mut tokio::net::TcpStream,
    request_user_agent: Option<&str>,
    method: &str,
    path: &str,
    remote_addr_text: Option<String>,
) -> anyhow::Result<()> {
    if method == "OPTIONS" {
        write_http_response(
            stream,
            "204 No Content",
            "application/json; charset=utf-8",
            &[],
        )
        .await?;
        stream.shutdown().await?;
        return Ok(());
    }

    let upstream = match crate::protocol_proxy::open_models_proxy_request(request_user_agent).await
    {
        Ok(upstream) => upstream,
        Err(error) => {
            let body = serde_json::to_vec(&serde_json::json!({
                "status": "failed",
                "message": error.to_string()
            }))?;
            write_http_response(
                stream,
                "502 Bad Gateway",
                "application/json; charset=utf-8",
                &body,
            )
            .await?;
            log_helper_response(
                "helper.models_proxy_failed",
                method,
                path,
                "502 Bad Gateway",
                remote_addr_text,
            );
            stream.shutdown().await?;
            return Ok(());
        }
    };

    let status = upstream.status();
    let is_success = upstream.is_success();
    let content_type = if upstream.content_type.is_empty() {
        "application/json; charset=utf-8".to_string()
    } else {
        upstream.content_type.clone()
    };
    let body = upstream.response.bytes().await?.to_vec();
    write_http_response(stream, &status, &content_type, &body).await?;
    log_helper_response(
        if is_success {
            "helper.models_proxy_ok"
        } else {
            "helper.models_proxy_upstream_error"
        },
        method,
        path,
        &status,
        remote_addr_text,
    );
    stream.shutdown().await?;
    Ok(())
}

async fn handle_audio_transcriptions_proxy_connection(
    stream: &mut tokio::net::TcpStream,
    request_body: &[u8],
    request_content_type: Option<&str>,
    request_user_agent: Option<&str>,
    request_audio_language: Option<&str>,
    method: &str,
    path: &str,
    remote_addr_text: Option<String>,
) -> anyhow::Result<()> {
    let upstream =
        match crate::protocol_proxy::open_audio_transcriptions_proxy_request_with_language(
            request_body,
            request_content_type.unwrap_or_default(),
            request_user_agent,
            request_audio_language,
        )
        .await
        {
            Ok(upstream) => upstream,
            Err(error) => {
                let body = serde_json::to_vec(&serde_json::json!({
                    "status": "failed",
                    "message": error.to_string()
                }))?;
                write_http_response(
                    stream,
                    "502 Bad Gateway",
                    "application/json; charset=utf-8",
                    &body,
                )
                .await?;
                log_helper_response(
                    "helper.audio_transcriptions_proxy_failed",
                    method,
                    path,
                    "502 Bad Gateway",
                    remote_addr_text,
                );
                stream.shutdown().await?;
                return Ok(());
            }
        };
    let status = upstream.status();
    let is_success = upstream.is_success();
    let content_type = if upstream.content_type.is_empty() {
        "application/json; charset=utf-8".to_string()
    } else {
        upstream.content_type.clone()
    };
    let body = upstream.response.bytes().await?.to_vec();
    let normalized =
        normalize_audio_transcription_response(status, is_success, &content_type, body);
    write_http_response(stream, &normalized.status, &content_type, &normalized.body).await?;
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "helper.audio_transcriptions_response",
        serde_json::json!({
            "method": method,
            "path": path,
            "status": normalized.status,
            "success": normalized.success,
            "textChars": normalized.text_chars,
            "hasError": normalized.has_error,
            "remote_addr": remote_addr_text
        }),
    );
    log_helper_response(
        if normalized.success {
            "helper.audio_transcriptions_proxy_ok"
        } else {
            "helper.audio_transcriptions_proxy_upstream_error"
        },
        method,
        path,
        &normalized.status,
        remote_addr_text,
    );
    stream.shutdown().await?;
    Ok(())
}

async fn handle_protocol_proxy_connection(
    stream: &mut tokio::net::TcpStream,
    request_body: &str,
    request_user_agent: Option<&str>,
    method: &str,
    path: &str,
    remote_addr_text: Option<String>,
) -> anyhow::Result<()> {
    let request_json = serde_json::from_str::<serde_json::Value>(request_body).ok();
    let upstream = match crate::protocol_proxy::open_responses_proxy_request_for_path(
        request_body,
        request_user_agent,
        path,
    )
    .await
    {
        Ok(upstream) => upstream,
        Err(error) => {
            let body = serde_json::to_vec(&serde_json::json!({
                "status": "failed",
                "message": error.to_string()
            }))?;
            write_http_response(
                stream,
                "502 Bad Gateway",
                "application/json; charset=utf-8",
                &body,
            )
            .await?;
            log_helper_response(
                "helper.protocol_proxy_failed",
                method,
                path,
                "502 Bad Gateway",
                remote_addr_text,
            );
            stream.shutdown().await?;
            return Ok(());
        }
    };

    if !upstream.is_success() {
        let status = upstream.status();
        let upstream_content_type = upstream.content_type.clone();
        let upstream_body = upstream.response.bytes().await?.to_vec();
        let error = crate::protocol_proxy::responses_error_from_upstream(
            upstream.status_code,
            &upstream_content_type,
            &upstream_body,
        );
        let body = serde_json::to_vec(&error)?;
        write_http_response(stream, &status, "application/json; charset=utf-8", &body).await?;
        log_helper_response(
            "helper.protocol_proxy_upstream_error",
            method,
            path,
            &status,
            remote_addr_text,
        );
        stream.shutdown().await?;
        return Ok(());
    }

    if upstream.is_stream {
        write_http_stream_headers(stream, "200 OK", "text/event-stream; charset=utf-8").await?;
        if upstream.wire_api == crate::protocol_proxy::UpstreamWireApi::Responses {
            let mut bytes_stream = upstream.response.bytes_stream();
            while let Some(chunk) = bytes_stream.next().await {
                if let Ok(bytes) = chunk {
                    stream.write_all(&bytes).await?;
                } else {
                    break;
                }
            }
            log_helper_response(
                "helper.protocol_proxy_stream_ok",
                method,
                path,
                "200 OK",
                remote_addr_text,
            );
            stream.shutdown().await?;
            return Ok(());
        }

        let mut converter = request_json
            .as_ref()
            .map(crate::protocol_proxy::ChatSseToResponsesConverter::with_request)
            .unwrap_or_default();
        let mut bytes_stream = upstream.response.bytes_stream();
        let mut stream_failed = false;

        while let Some(chunk) = bytes_stream.next().await {
            match chunk {
                Ok(bytes) => {
                    let converted = converter.push_bytes(&bytes);
                    if !converted.is_empty() {
                        stream.write_all(&converted).await?;
                    }
                }
                Err(error) => {
                    let failed = converter.fail(
                        format!("Stream error: {error}"),
                        Some("stream_error".to_string()),
                    );
                    if !failed.is_empty() {
                        stream.write_all(&failed).await?;
                    }
                    stream_failed = true;
                    break;
                }
            }
        }

        if !stream_failed {
            let tail = converter.finish();
            if !tail.is_empty() {
                stream.write_all(&tail).await?;
            }
        }
        log_helper_response(
            "helper.protocol_proxy_stream_ok",
            method,
            path,
            "200 OK",
            remote_addr_text,
        );
        stream.shutdown().await?;
        return Ok(());
    }

    let upstream_body = upstream.response.bytes().await?;
    if upstream.wire_api == crate::protocol_proxy::UpstreamWireApi::Responses {
        write_http_response(
            stream,
            "200 OK",
            if upstream.content_type.is_empty() {
                "application/json; charset=utf-8"
            } else {
                &upstream.content_type
            },
            &upstream_body,
        )
        .await?;
        log_helper_response(
            "helper.protocol_proxy_ok",
            method,
            path,
            "200 OK",
            remote_addr_text,
        );
        stream.shutdown().await?;
        return Ok(());
    }

    let chat_json: serde_json::Value = serde_json::from_slice(&upstream_body)?;
    let response_json = if let Some(request_json) = request_json.as_ref() {
        crate::protocol_proxy::chat_completion_to_response_with_request(chat_json, request_json)?
    } else {
        crate::protocol_proxy::chat_completion_to_response(chat_json)?
    };
    let body = serde_json::to_vec(&response_json)?;
    write_http_response(stream, "200 OK", "application/json; charset=utf-8", &body).await?;
    log_helper_response(
        "helper.protocol_proxy_ok",
        method,
        path,
        "200 OK",
        remote_addr_text,
    );
    stream.shutdown().await?;
    Ok(())
}

async fn handle_chat_completions_proxy_connection(
    stream: &mut tokio::net::TcpStream,
    request_body: &str,
    request_user_agent: Option<&str>,
    method: &str,
    path: &str,
    remote_addr_text: Option<String>,
) -> anyhow::Result<()> {
    let upstream = match crate::protocol_proxy::open_chat_completions_proxy_request(
        request_body,
        request_user_agent,
    )
    .await
    {
        Ok(upstream) => upstream,
        Err(error) => {
            let body = serde_json::to_vec(&serde_json::json!({
                "status": "failed",
                "message": error.to_string()
            }))?;
            write_http_response(
                stream,
                "502 Bad Gateway",
                "application/json; charset=utf-8",
                &body,
            )
            .await?;
            log_helper_response(
                "helper.chat_completions_proxy_failed",
                method,
                path,
                "502 Bad Gateway",
                remote_addr_text,
            );
            stream.shutdown().await?;
            return Ok(());
        }
    };

    let status = upstream.status();
    let is_success = upstream.is_success();
    let content_type = if upstream.content_type.is_empty() {
        "application/json; charset=utf-8".to_string()
    } else {
        upstream.content_type.clone()
    };

    if upstream.is_stream && is_success {
        write_http_stream_headers(stream, &status, &content_type).await?;
        let mut bytes_stream = upstream.response.bytes_stream();
        while let Some(chunk) = bytes_stream.next().await {
            stream.write_all(&chunk?).await?;
        }
        log_helper_response(
            "helper.chat_completions_proxy_stream_ok",
            method,
            path,
            &status,
            remote_addr_text,
        );
        stream.shutdown().await?;
        return Ok(());
    }

    let body = upstream.response.bytes().await?.to_vec();
    write_http_response(stream, &status, &content_type, &body).await?;
    log_helper_response(
        if is_success {
            "helper.chat_completions_proxy_ok"
        } else {
            "helper.chat_completions_proxy_upstream_error"
        },
        method,
        path,
        &status,
        remote_addr_text,
    );
    stream.shutdown().await?;
    Ok(())
}

async fn write_http_response(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Authorization\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.write_all(body).await?;
    Ok(())
}

async fn write_http_stream_headers(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    content_type: &str,
) -> anyhow::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nCache-Control: no-cache\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Authorization\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

fn log_helper_response(
    event: &str,
    method: &str,
    path: &str,
    status: &str,
    remote_addr_text: Option<String>,
) {
    let _ = crate::diagnostic_log::append_diagnostic_log(
        event,
        serde_json::json!({
            "method": method,
            "path": path,
            "status": status,
            "remote_addr": remote_addr_text
        }),
    );
}

struct NormalizedAudioTranscriptionResponse {
    status: String,
    body: Vec<u8>,
    success: bool,
    text_chars: Option<usize>,
    has_error: bool,
}

fn normalize_audio_transcription_response(
    status: String,
    success: bool,
    content_type: &str,
    body: Vec<u8>,
) -> NormalizedAudioTranscriptionResponse {
    let parsed = if content_type.to_ascii_lowercase().contains("json") {
        serde_json::from_slice::<serde_json::Value>(&body).ok()
    } else {
        None
    };
    let text_chars = parsed
        .as_ref()
        .and_then(|value| value.get("text"))
        .and_then(serde_json::Value::as_str)
        .map(|text| text.chars().count());
    let error_message = parsed
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(|error| error.get("message").or(Some(error)))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let has_error = error_message.is_some();

    if success && (has_error || text_chars.is_none()) {
        let message = error_message.unwrap_or_else(|| "上游未返回有效的音频转写文本".to_string());
        return NormalizedAudioTranscriptionResponse {
            status: "502 Bad Gateway".to_string(),
            body: serde_json::to_vec(&serde_json::json!({
                "error": {
                    "message": format!("音频转写响应无效：{message}")
                }
            }))
            .unwrap_or_else(|_| body.clone()),
            success: false,
            text_chars,
            has_error: true,
        };
    }

    NormalizedAudioTranscriptionResponse {
        status,
        body,
        success,
        text_chars,
        has_error,
    }
}

#[cfg(test)]
mod computer_use_tests {
    use super::{
        ChunkedBody, ChunkedBodyScan, MAX_HTTP_BODY_BYTES, MAX_HTTP_HEADER_BYTES,
        content_length_body, decode_chunked_body, header_value_from_headers, http_body_framing,
        normalize_audio_transcription_response, overlay_image_content_type, scan_chunked_body,
    };
    use std::path::Path;

    #[test]
    fn overlay_image_content_type_accepts_common_images_only() {
        assert_eq!(
            overlay_image_content_type(Path::new("overlay.PNG")),
            Some("image/png")
        );
        assert_eq!(
            overlay_image_content_type(Path::new("overlay.jpeg")),
            Some("image/jpeg")
        );
        assert_eq!(
            overlay_image_content_type(Path::new("overlay.webp")),
            Some("image/webp")
        );
        assert_eq!(overlay_image_content_type(Path::new("overlay.txt")), None);
    }

    #[test]
    fn header_value_from_headers_reads_user_agent_case_insensitively() {
        let request = "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\nUser-Agent: Codex/26.614\r\nContent-Length: 2";

        assert_eq!(
            header_value_from_headers(request, "user-agent").as_deref(),
            Some("Codex/26.614")
        );
    }

    #[test]
    fn chunked_audio_body_is_decoded_without_touching_binary_bytes() {
        let encoded = b"4\r\nRIFF\r\n5\r\n\0DATA\r\n0\r\n\r\n";
        let ChunkedBody::Complete(decoded) = decode_chunked_body(encoded).unwrap() else {
            panic!("expected complete chunked body");
        };
        assert_eq!(decoded, b"RIFF\0DATA");
    }

    #[test]
    fn chunked_audio_body_accepts_extensions_trailers_and_partial_prefixes() {
        let encoded = b"3;name=value\r\n\x00\x80\xff\r\n2\r\nAB\r\n0\r\nX-Trace: yes\r\n\r\n";
        for prefix_len in 0..encoded.len() {
            assert!(matches!(
                scan_chunked_body(&encoded[..prefix_len]).unwrap(),
                ChunkedBodyScan::Incomplete
            ));
        }
        assert!(matches!(
            scan_chunked_body(encoded).unwrap(),
            ChunkedBodyScan::Complete
        ));
    }

    #[test]
    fn chunked_audio_body_rejects_oversized_headers_and_payloads() {
        let oversized_size_line = vec![b'f'; MAX_HTTP_HEADER_BYTES + 1];
        assert_eq!(
            scan_chunked_body(&oversized_size_line)
                .unwrap_err()
                .status(),
            "400 Bad Request"
        );

        let oversized_size = format!("{:X}\r\n", MAX_HTTP_BODY_BYTES + 1);
        assert_eq!(
            decode_chunked_body(oversized_size.as_bytes())
                .unwrap_err()
                .status(),
            "413 Payload Too Large"
        );
    }

    #[test]
    fn body_framing_rejects_content_length_and_transfer_encoding_together() {
        let error = http_body_framing(
            b"POST /v1/audio/transcriptions HTTP/1.1\r\nContent-Length: 4\r\nTransfer-Encoding: chunked",
        )
        .unwrap_err();
        assert_eq!(error.status(), "400 Bad Request");
    }

    #[test]
    fn content_length_body_enforces_audio_body_limit() {
        assert_eq!(
            content_length_body(&[], MAX_HTTP_BODY_BYTES + 1)
                .unwrap_err()
                .status(),
            "413 Payload Too Large"
        );
    }

    #[test]
    fn audio_response_rejects_success_status_with_error_payload() {
        let normalized = normalize_audio_transcription_response(
            "200 OK".to_string(),
            true,
            "application/json",
            br#"{"error":{"message":"webm unsupported"}}"#.to_vec(),
        );

        assert_eq!(normalized.status, "502 Bad Gateway");
        assert!(!normalized.success);
        assert!(normalized.has_error);
        assert!(String::from_utf8_lossy(&normalized.body).contains("webm unsupported"));
    }

    #[test]
    fn audio_response_accepts_json_text_payload() {
        let normalized = normalize_audio_transcription_response(
            "200 OK".to_string(),
            true,
            "application/json",
            r#"{"text":"测试成功"}"#.as_bytes().to_vec(),
        );

        assert_eq!(normalized.status, "200 OK");
        assert!(normalized.success);
        assert_eq!(normalized.text_chars, Some(4));
        assert!(!normalized.has_error);
    }
}

const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;
const MAX_HTTP_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_HTTP_ENCODED_BODY_BYTES: usize = 64 * 1024 * 1024;

struct HttpRequest {
    headers: Vec<u8>,
    body: Vec<u8>,
}

#[derive(Debug)]
struct HttpRequestReadError {
    status: &'static str,
    message: String,
}

impl HttpRequestReadError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: "400 Bad Request",
            message: message.into(),
        }
    }

    fn payload_too_large() -> Self {
        Self {
            status: "413 Payload Too Large",
            message: format!("HTTP 请求体超过 {MAX_HTTP_BODY_BYTES} 字节限制"),
        }
    }

    fn status(&self) -> &'static str {
        self.status
    }
}

impl std::fmt::Display for HttpRequestReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HttpRequestReadError {}

impl From<std::io::Error> for HttpRequestReadError {
    fn from(error: std::io::Error) -> Self {
        Self::bad_request(format!("读取 HTTP 请求失败: {error}"))
    }
}

#[derive(Debug)]
enum HttpBodyFraming {
    Empty,
    ContentLength(usize),
    Chunked,
}

#[derive(Debug)]
enum ChunkedBody {
    Incomplete,
    Complete(Vec<u8>),
}

#[derive(Debug)]
enum ChunkedBodyScan {
    Incomplete,
    Complete,
}

#[derive(Default)]
struct ChunkedScanState {
    position: usize,
    decoded_len: usize,
    complete: bool,
}

impl ChunkedScanState {
    fn advance(&mut self, encoded: &[u8]) -> Result<ChunkedBodyScan, HttpRequestReadError> {
        if self.complete {
            return Ok(ChunkedBodyScan::Complete);
        }
        loop {
            let chunk_start = self.position;
            let Some(line_end_offset) = encoded[chunk_start..]
                .windows(2)
                .position(|window| window == b"\r\n")
            else {
                if encoded.len().saturating_sub(chunk_start) > MAX_HTTP_HEADER_BYTES {
                    return Err(HttpRequestReadError::bad_request("chunk size 行过大"));
                }
                return Ok(ChunkedBodyScan::Incomplete);
            };
            if line_end_offset > MAX_HTTP_HEADER_BYTES {
                return Err(HttpRequestReadError::bad_request("chunk size 行过大"));
            }
            let line_end = chunk_start + line_end_offset;
            let size_text = std::str::from_utf8(&encoded[chunk_start..line_end])
                .map_err(|_| HttpRequestReadError::bad_request("chunk size 不是有效 ASCII"))?;
            let size_token = size_text.split(';').next().unwrap_or_default().trim();
            let chunk_size = usize::from_str_radix(size_token, 16)
                .map_err(|_| HttpRequestReadError::bad_request("chunk size 无效"))?;
            let data_start = line_end + 2;

            if chunk_size == 0 {
                let mut trailer_start = data_start;
                loop {
                    let Some(trailer_end_offset) = encoded[trailer_start..]
                        .windows(2)
                        .position(|window| window == b"\r\n")
                    else {
                        if encoded.len().saturating_sub(data_start) > MAX_HTTP_HEADER_BYTES {
                            return Err(HttpRequestReadError::bad_request("chunk trailer 过大"));
                        }
                        return Ok(ChunkedBodyScan::Incomplete);
                    };
                    if trailer_start + trailer_end_offset - data_start > MAX_HTTP_HEADER_BYTES {
                        return Err(HttpRequestReadError::bad_request("chunk trailer 过大"));
                    }
                    if trailer_end_offset == 0 {
                        self.position = trailer_start + 2;
                        self.complete = true;
                        return Ok(ChunkedBodyScan::Complete);
                    }
                    trailer_start += trailer_end_offset + 2;
                }
            }

            let next_decoded_len = self
                .decoded_len
                .checked_add(chunk_size)
                .ok_or_else(HttpRequestReadError::payload_too_large)?;
            if next_decoded_len > MAX_HTTP_BODY_BYTES {
                return Err(HttpRequestReadError::payload_too_large());
            }
            let chunk_end = data_start
                .checked_add(chunk_size)
                .ok_or_else(HttpRequestReadError::payload_too_large)?;
            if encoded.len() < chunk_end + 2 {
                return Ok(ChunkedBodyScan::Incomplete);
            }
            if &encoded[chunk_end..chunk_end + 2] != b"\r\n" {
                return Err(HttpRequestReadError::bad_request("chunk 数据后缺少 CRLF"));
            }
            self.decoded_len = next_decoded_len;
            self.position = chunk_end + 2;
        }
    }
}

async fn read_http_request(
    stream: &mut tokio::net::TcpStream,
) -> Result<HttpRequest, HttpRequestReadError> {
    let mut buffer = Vec::new();
    let mut chunk = vec![0_u8; 4096];
    let mut header_end = None;
    let mut framing = HttpBodyFraming::Empty;
    let mut chunked_scan = ChunkedScanState::default();

    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if header_end.is_none() {
            header_end = find_header_end(&buffer);
            if let Some(end) = header_end {
                if end > MAX_HTTP_HEADER_BYTES {
                    return Err(HttpRequestReadError::bad_request("HTTP 请求头过大"));
                }
                framing = http_body_framing(&buffer[..end])?;
            } else if buffer.len() > MAX_HTTP_HEADER_BYTES {
                return Err(HttpRequestReadError::bad_request("HTTP 请求头过大"));
            }
        }
        if let Some(end) = header_end {
            let body = &buffer[end + 4..];
            if body.len() > MAX_HTTP_ENCODED_BODY_BYTES {
                return Err(HttpRequestReadError::payload_too_large());
            }
            match framing {
                HttpBodyFraming::Empty => break,
                HttpBodyFraming::ContentLength(content_length) => {
                    if content_length > MAX_HTTP_BODY_BYTES {
                        return Err(HttpRequestReadError::payload_too_large());
                    }
                    if body.len() >= content_length {
                        break;
                    }
                }
                HttpBodyFraming::Chunked => match chunked_scan.advance(body)? {
                    ChunkedBodyScan::Incomplete => {}
                    ChunkedBodyScan::Complete => break,
                },
            }
        }
    }

    let header_end =
        header_end.ok_or_else(|| HttpRequestReadError::bad_request("HTTP 请求头不完整"))?;
    let headers = buffer[..header_end].to_vec();
    let encoded_body = &buffer[header_end + 4..];
    let body = match framing {
        HttpBodyFraming::Empty => Vec::new(),
        HttpBodyFraming::ContentLength(content_length) => {
            content_length_body(encoded_body, content_length)?
        }
        HttpBodyFraming::Chunked => match decode_chunked_body(encoded_body)? {
            ChunkedBody::Complete(body) => body,
            ChunkedBody::Incomplete => {
                return Err(HttpRequestReadError::bad_request(
                    "chunked HTTP 请求体不完整",
                ));
            }
        },
    };
    Ok(HttpRequest { headers, body })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn http_body_framing(headers: &[u8]) -> Result<HttpBodyFraming, HttpRequestReadError> {
    let text = String::from_utf8_lossy(headers);
    let mut content_length = None;
    let mut transfer_encoding: Option<String> = None;
    for line in text.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let parsed = value
                .trim()
                .parse::<usize>()
                .map_err(|_| HttpRequestReadError::bad_request("Content-Length 无效"))?;
            if content_length
                .replace(parsed)
                .is_some_and(|existing| existing != parsed)
            {
                return Err(HttpRequestReadError::bad_request(
                    "存在冲突的 Content-Length 请求头",
                ));
            }
        } else if name.trim().eq_ignore_ascii_case("transfer-encoding") {
            let value = value.trim().to_ascii_lowercase();
            if let Some(existing) = transfer_encoding.as_mut() {
                existing.push(',');
                existing.push_str(&value);
            } else {
                transfer_encoding = Some(value);
            }
        }
    }
    if transfer_encoding.is_some() && content_length.is_some() {
        return Err(HttpRequestReadError::bad_request(
            "Transfer-Encoding 与 Content-Length 不能同时使用",
        ));
    }
    match transfer_encoding.as_deref() {
        Some("chunked") => Ok(HttpBodyFraming::Chunked),
        Some(_) => Err(HttpRequestReadError::bad_request(
            "仅支持 Transfer-Encoding: chunked",
        )),
        None => Ok(content_length
            .map(HttpBodyFraming::ContentLength)
            .unwrap_or(HttpBodyFraming::Empty)),
    }
}

fn content_length_body(
    encoded: &[u8],
    content_length: usize,
) -> Result<Vec<u8>, HttpRequestReadError> {
    if content_length > MAX_HTTP_BODY_BYTES {
        return Err(HttpRequestReadError::payload_too_large());
    }
    if encoded.len() < content_length {
        return Err(HttpRequestReadError::bad_request("HTTP 请求体不完整"));
    }
    Ok(encoded[..content_length].to_vec())
}

fn decode_chunked_body(encoded: &[u8]) -> Result<ChunkedBody, HttpRequestReadError> {
    let mut decoded = Vec::new();
    let mut position = 0;
    loop {
        let Some(line_end_offset) = encoded[position..]
            .windows(2)
            .position(|window| window == b"\r\n")
        else {
            if encoded.len().saturating_sub(position) > MAX_HTTP_HEADER_BYTES {
                return Err(HttpRequestReadError::bad_request("chunk size 行过大"));
            }
            return Ok(ChunkedBody::Incomplete);
        };
        if line_end_offset > MAX_HTTP_HEADER_BYTES {
            return Err(HttpRequestReadError::bad_request("chunk size 行过大"));
        }
        let line_end = position + line_end_offset;
        let size_text = std::str::from_utf8(&encoded[position..line_end])
            .map_err(|_| HttpRequestReadError::bad_request("chunk size 不是有效 ASCII"))?;
        let size_token = size_text.split(';').next().unwrap_or_default().trim();
        let chunk_size = usize::from_str_radix(size_token, 16)
            .map_err(|_| HttpRequestReadError::bad_request("chunk size 无效"))?;
        position = line_end + 2;
        if chunk_size == 0 {
            loop {
                let Some(trailer_end_offset) = encoded[position..]
                    .windows(2)
                    .position(|window| window == b"\r\n")
                else {
                    if encoded.len().saturating_sub(line_end + 2) > MAX_HTTP_HEADER_BYTES {
                        return Err(HttpRequestReadError::bad_request("chunk trailer 过大"));
                    }
                    return Ok(ChunkedBody::Incomplete);
                };
                if position + trailer_end_offset - (line_end + 2) > MAX_HTTP_HEADER_BYTES {
                    return Err(HttpRequestReadError::bad_request("chunk trailer 过大"));
                }
                if trailer_end_offset == 0 {
                    return Ok(ChunkedBody::Complete(decoded));
                }
                position += trailer_end_offset + 2;
            }
        }
        if decoded.len().saturating_add(chunk_size) > MAX_HTTP_BODY_BYTES {
            return Err(HttpRequestReadError::payload_too_large());
        }
        let chunk_end = position
            .checked_add(chunk_size)
            .ok_or_else(HttpRequestReadError::payload_too_large)?;
        if encoded.len() < chunk_end + 2 {
            return Ok(ChunkedBody::Incomplete);
        }
        if &encoded[chunk_end..chunk_end + 2] != b"\r\n" {
            return Err(HttpRequestReadError::bad_request("chunk 数据后缺少 CRLF"));
        }
        decoded.extend_from_slice(&encoded[position..chunk_end]);
        position = chunk_end + 2;
    }
}

#[cfg(test)]
fn scan_chunked_body(encoded: &[u8]) -> Result<ChunkedBodyScan, HttpRequestReadError> {
    ChunkedScanState::default().advance(encoded)
}

fn header_value_from_headers(headers: &str, header_name: &str) -> Option<String> {
    headers
        .lines()
        .skip(1)
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case(header_name)
                .then(|| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
}

fn sanitize_diagnostic_event(event: &str) -> String {
    let sanitized = event
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "event".to_string()
    } else {
        sanitized
    }
}

pub fn build_codex_arguments(debug_port: u16, extra_args: &[String]) -> Vec<String> {
    let mut args = vec![
        format!("--remote-debugging-port={debug_port}"),
        format!("--remote-allow-origins=http://127.0.0.1:{debug_port}"),
    ];
    args.extend(normalize_codex_extra_args(extra_args));
    args
}

pub fn build_codex_command(app_dir: &Path, debug_port: u16, extra_args: &[String]) -> Vec<String> {
    let mut command = vec![
        crate::app_paths::build_codex_executable(app_dir)
            .to_string_lossy()
            .to_string(),
    ];
    command.extend(build_codex_arguments(debug_port, extra_args));
    command
}

pub fn build_packaged_activation(
    app_dir: &Path,
    debug_port: u16,
    extra_args: &[String],
) -> Option<CodexLaunch> {
    Some(CodexLaunch::PackagedActivation {
        app_user_model_id: crate::app_paths::packaged_app_user_model_id(app_dir)?,
        arguments: command_line_arguments(&build_codex_arguments(debug_port, extra_args)),
        process_id: None,
    })
}

async fn retry_injection(debug_port: u16, helper_port: u16) -> anyhow::Result<()> {
    let mut last_error = None;
    for _ in 0..20 {
        match try_inject(debug_port, helper_port).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Codex injection failed")))
}

pub async fn check_and_reinject_bridge(debug_port: u16, helper_port: u16) -> bool {
    check_and_reinject_bridge_inner(debug_port, helper_port, false).await
}

pub fn browser_identity_changed(previous: Option<&str>, current: &str) -> bool {
    previous.is_some_and(|previous| previous != current)
}

async fn check_and_reinject_bridge_inner(
    debug_port: u16,
    helper_port: u16,
    browser_identity_changed: bool,
) -> bool {
    let healthy = if browser_identity_changed {
        false
    } else {
        match bridge_health_ok(debug_port).await {
            Ok(healthy) => healthy,
            Err(error) => {
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "bridge.health_check_failed",
                    serde_json::json!({
                        "debug_port": debug_port,
                        "helper_port": helper_port,
                        "message": error.to_string()
                    }),
                );
                false
            }
        }
    };
    if healthy {
        return false;
    }

    let _ = crate::diagnostic_log::append_diagnostic_log(
        "bridge.reinject_start",
        serde_json::json!({
            "debug_port": debug_port,
            "helper_port": helper_port,
            "browser_identity_changed": browser_identity_changed
        }),
    );
    match retry_injection(debug_port, helper_port).await {
        Ok(()) => {
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "bridge.reinject_ok",
                serde_json::json!({
                    "debug_port": debug_port,
                    "helper_port": helper_port
                }),
            );
            true
        }
        Err(error) => {
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "bridge.reinject_failed",
                serde_json::json!({
                    "debug_port": debug_port,
                    "helper_port": helper_port,
                    "message": error.to_string()
                }),
            );
            false
        }
    }
}

async fn bridge_health_ok(debug_port: u16) -> anyhow::Result<bool> {
    let targets = crate::cdp::list_targets(debug_port).await?;
    let target = crate::cdp::pick_injectable_codex_page_target(&targets)?;
    let websocket_url = target
        .web_socket_debugger_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("selected CDP target has no websocket URL"))?;
    let result = crate::bridge::evaluate_script_with_await_promise(
        websocket_url,
        crate::bridge::bridge_health_check_script(),
        true,
    )
    .await?;
    if !runtime_evaluate_result_is_true(&result) {
        return Ok(false);
    }

    Ok(true)
}

fn runtime_evaluate_result_is_true(result: &Value) -> bool {
    result
        .get("result")
        .and_then(|result| result.get("result"))
        .and_then(|result| result.get("value"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

async fn try_inject(debug_port: u16, helper_port: u16) -> anyhow::Result<()> {
    let targets = crate::cdp::list_targets(debug_port).await?;
    let primary_target = crate::cdp::pick_injectable_codex_page_target(&targets)?;
    let settings = SettingsStore::default().load().unwrap_or_default();
    let mut injection_targets = vec![primary_target.clone()];
    for target in targets.iter().filter(|target| {
        crate::cdp::is_injectable_page_target(target)
            && crate::cdp::is_global_dictation_page_target(target)
            && settings.audio_transcription_enabled
    }) {
        if target.id != primary_target.id {
            injection_targets.push(target.clone());
        }
    }
    let mut primary_error = None;
    for target in injection_targets {
        let Some(websocket_url) = target.web_socket_debugger_url.as_deref() else {
            if target.id == primary_target.id {
                primary_error = Some(anyhow::anyhow!("selected CDP target has no websocket URL"));
            }
            continue;
        };
        let is_global_dictation = crate::cdp::is_global_dictation_page_target(&target);
        if is_global_dictation {
            let script = crate::assets::audio_transcription_injection_script(helper_port);
            let result = crate::bridge::evaluate_script(websocket_url, &script).await;
            if let Err(error) = result {
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "bridge.page_injection_failed",
                    serde_json::json!({
                        "target_id": target.id,
                        "target_url": target.url,
                        "is_primary": false,
                        "injection_kind": "audio-lightweight",
                        "message": error.to_string()
                    }),
                );
            } else {
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "bridge.page_injection_ok",
                    serde_json::json!({
                        "target_id": target.id,
                        "target_url": target.url,
                        "is_primary": false,
                        "injection_kind": "audio-lightweight"
                    }),
                );
            }
            continue;
        }
        let script = crate::assets::injection_script_with_settings(helper_port, &settings);
        let ctx = crate::routes::BridgeContext::core(Arc::new(
            crate::routes::CoreRuntimeService::new(debug_port, StatusStore::default()),
        ));
        let result = crate::bridge::install_bridge(
            websocket_url,
            crate::bridge::BRIDGE_BINDING_NAME,
            Arc::new(move |path, payload| {
                let ctx = ctx.clone();
                Box::pin(async move {
                    Ok(crate::routes::handle_bridge_request(ctx, &path, payload).await)
                })
            }),
            &[script],
        )
        .await;
        if let Err(error) = result {
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "bridge.page_injection_failed",
                serde_json::json!({
                    "target_id": target.id,
                    "target_url": target.url,
                    "is_primary": target.id == primary_target.id,
                    "injection_kind": if is_global_dictation { "audio" } else { "full" },
                    "message": error.to_string()
                }),
            );
            if target.id == primary_target.id {
                primary_error = Some(error);
            }
        } else {
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "bridge.page_injection_ok",
                serde_json::json!({
                    "target_id": target.id,
                    "target_url": target.url,
                    "is_primary": target.id == primary_target.id,
                    "injection_kind": if is_global_dictation { "audio" } else { "full" }
                }),
            );
        }
    }
    if let Some(error) = primary_error {
        return Err(error);
    }

    crate::codex_local_storage::sanitize_local_storage_model_suffixes_nonfatal(debug_port).await;
    Ok(())
}

pub fn build_macos_open_command(
    app_dir: &Path,
    debug_port: u16,
    extra_args: &[String],
) -> Vec<String> {
    let mut command = vec![
        "open".to_string(),
        "-W".to_string(),
        "-a".to_string(),
        app_dir.to_string_lossy().to_string(),
        "--args".to_string(),
    ];
    command.extend(build_codex_arguments(debug_port, extra_args));
    command
}

pub fn build_macos_cleanup_command(
    app_dir: &Path,
    policy: MacosCleanupPolicy,
) -> Option<Vec<String>> {
    if policy == MacosCleanupPolicy::SkipQuitBecauseAlreadyRunning {
        return None;
    }
    let app_name = app_dir
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Codex");
    Some(vec![
        "osascript".to_string(),
        "-e".to_string(),
        format!(
            r#"tell application "{}" to quit"#,
            app_name.replace('"', "\\\"")
        ),
    ])
}

async fn run_macos_cleanup_command(
    app_dir: &Path,
    policy: MacosCleanupPolicy,
) -> anyhow::Result<()> {
    let Some(command) = build_macos_cleanup_command(app_dir, policy) else {
        return Ok(());
    };
    let Some(executable) = command.first() else {
        return Ok(());
    };
    let _ = Command::new(executable)
        .args(&command[1..])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .with_context(|| format!("failed to request macOS app quit for {}", app_dir.display()))?;
    Ok(())
}

fn macos_app_dir_from_open_command(command: &[String]) -> Option<PathBuf> {
    let app_index = command.iter().position(|part| part == "-a")?;
    command.get(app_index + 1).map(PathBuf::from)
}

async fn is_macos_app_running(app_dir: &Path) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let app_name = app_dir
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Codex");
    let script = format!(
        r#"application "{}" is running"#,
        app_name.replace('"', "\\\"")
    );
    let Ok(output) = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
    else {
        return false;
    };
    output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .trim()
            .eq_ignore_ascii_case("true")
}

#[cfg_attr(not(windows), allow(dead_code))]
fn post_launch_guard_artifacts_ready(
    artifacts: &crate::computer_use_guard::GuardArtifacts,
) -> bool {
    artifacts.notify_exe.is_some()
        && artifacts.marketplace_path.is_some()
        && (!artifacts.runtime_exports_needed || artifacts.sky_package_json.is_some())
}

#[cfg_attr(not(windows), allow(dead_code))]
fn should_stop_post_launch_computer_use_guard(
    stable_unchanged_attempts: usize,
    artifacts: &crate::computer_use_guard::GuardArtifacts,
) -> bool {
    stable_unchanged_attempts >= POST_LAUNCH_COMPUTER_USE_GUARD_STABLE_ATTEMPTS
        && post_launch_guard_artifacts_ready(artifacts)
}

#[cfg(windows)]
async fn run_post_launch_computer_use_guard(
    home: PathBuf,
    mut artifacts: Option<crate::computer_use_guard::GuardArtifacts>,
    shutdown_rx: &mut tokio::sync::oneshot::Receiver<()>,
) {
    let mut previous_delay = 0_u64;
    let mut stable_unchanged_attempts = 0_usize;
    for (index, delay) in POST_LAUNCH_COMPUTER_USE_GUARD_SECONDS
        .iter()
        .copied()
        .enumerate()
    {
        let wait_seconds = delay.saturating_sub(previous_delay);
        previous_delay = delay;
        if wait_seconds > 0 {
            tokio::select! {
                _ = &mut *shutdown_rx => return,
                _ = tokio::time::sleep(std::time::Duration::from_secs(wait_seconds)) => {}
            }
        }
        let attempt = index + 1;
        let resolved_artifacts = match artifacts.take() {
            Some(artifacts) => artifacts,
            None => match crate::computer_use_guard::resolve_computer_use_guard_artifacts(&home) {
                Ok(resolved) => resolved,
                Err(error) => {
                    stable_unchanged_attempts = 0;
                    let _ = crate::diagnostic_log::append_diagnostic_log(
                        "computer_use_guard.post_launch_failed",
                        serde_json::json!({
                            "attempt": attempt,
                            "delay_seconds": delay,
                            "phase": "resolve_artifacts",
                            "message": error.to_string()
                        }),
                    );
                    continue;
                }
            },
        };
        let artifacts_ready = post_launch_guard_artifacts_ready(&resolved_artifacts);
        artifacts = artifacts_ready.then_some(resolved_artifacts.clone());
        match crate::computer_use_guard::ensure_computer_use_config_with_artifacts(
            &home,
            &resolved_artifacts,
        ) {
            Ok(result) => {
                if !result.changed && artifacts_ready {
                    stable_unchanged_attempts += 1;
                } else {
                    stable_unchanged_attempts = 0;
                }
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "computer_use_guard.post_launch_ok",
                    serde_json::json!({
                        "attempt": attempt,
                        "delay_seconds": delay,
                        "changed": result.changed,
                        "stable_unchanged_attempts": stable_unchanged_attempts,
                        "notify_exe": result
                            .notify_exe
                            .map(|path| path.to_string_lossy().to_string())
                    }),
                );
                if should_stop_post_launch_computer_use_guard(
                    stable_unchanged_attempts,
                    &resolved_artifacts,
                ) {
                    let _ = crate::diagnostic_log::append_diagnostic_log(
                        "computer_use_guard.post_launch_stable_stop",
                        serde_json::json!({
                            "attempt": attempt,
                            "delay_seconds": delay,
                            "stable_unchanged_attempts": stable_unchanged_attempts
                        }),
                    );
                    return;
                }
            }
            Err(error) => {
                stable_unchanged_attempts = 0;
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "computer_use_guard.post_launch_failed",
                    serde_json::json!({
                        "attempt": attempt,
                        "delay_seconds": delay,
                        "message": error.to_string()
                    }),
                );
            }
        }
    }
}

#[cfg(windows)]
async fn wait_for_windows_process_id(process_id: u32) -> anyhow::Result<()> {
    tokio::task::spawn_blocking(move || wait_for_windows_process_id_blocking(process_id))
        .await
        .context("Windows process wait task failed")?
}

#[cfg(windows)]
async fn terminate_windows_process_id(process_id: u32) -> anyhow::Result<()> {
    tokio::task::spawn_blocking(move || terminate_windows_process_id_blocking(process_id))
        .await
        .context("Windows process termination task failed")?
}

#[cfg(windows)]
fn wait_for_windows_process_id_blocking(process_id: u32) -> anyhow::Result<()> {
    use windows::Win32::Foundation::{CloseHandle, WAIT_FAILED};
    use windows::Win32::System::Threading::{
        INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        WaitForSingleObject,
    };

    unsafe {
        let handle = OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            process_id,
        )
        .with_context(|| format!("failed to open Windows process id {process_id}"))?;
        let wait_result = WaitForSingleObject(handle, INFINITE);
        let _ = CloseHandle(handle);
        if wait_result == WAIT_FAILED {
            anyhow::bail!("failed to wait for Windows process id {process_id}");
        }
    }
    Ok(())
}

#[cfg(windows)]
fn terminate_windows_process_id_blocking(process_id: u32) -> anyhow::Result<()> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
    };

    unsafe {
        let handle = OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            process_id,
        )
        .with_context(|| format!("failed to open Windows process id {process_id}"))?;
        let terminate_result = TerminateProcess(handle, 1);
        let _ = CloseHandle(handle);
        terminate_result
            .with_context(|| format!("failed to terminate Windows process id {process_id}"))?;
    }
    Ok(())
}

#[cfg(not(windows))]
async fn wait_for_windows_process_id(process_id: u32) -> anyhow::Result<()> {
    anyhow::bail!("cannot wait for Windows process id {process_id} on this platform")
}

#[cfg(not(windows))]
async fn terminate_windows_process_id(process_id: u32) -> anyhow::Result<()> {
    anyhow::bail!("cannot terminate Windows process id {process_id} on this platform")
}

fn launch_status(
    status: &str,
    message: &str,
    debug_port: u16,
    helper_port: u16,
    app_dir: &Path,
) -> LaunchStatus {
    LaunchStatus {
        status: status.to_string(),
        message: message.to_string(),
        started_at_ms: now_ms(),
        debug_port: Some(debug_port),
        helper_port: Some(helper_port),
        codex_app: Some(app_dir.to_string_lossy().to_string()),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn command_line_arguments(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_windows_argument(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_windows_argument(arg: &str) -> String {
    if !arg.is_empty() && !arg.bytes().any(|byte| matches!(byte, b' ' | b'\t' | b'"')) {
        return arg.to_string();
    }
    let mut output = String::from("\"");
    let mut backslashes = 0;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                output.push_str(&"\\".repeat(backslashes * 2 + 1));
                output.push('"');
                backslashes = 0;
            }
            _ => {
                output.push_str(&"\\".repeat(backslashes));
                output.push(ch);
                backslashes = 0;
            }
        }
    }
    output.push_str(&"\\".repeat(backslashes * 2));
    output.push('"');
    output
}

#[cfg(not(windows))]
pub async fn activate_packaged_app(
    _app_user_model_id: &str,
    _arguments: &str,
) -> anyhow::Result<u32> {
    anyhow::bail!("Packaged app activation is only supported on Windows")
}

#[cfg(windows)]
pub async fn activate_packaged_app(
    app_user_model_id: &str,
    arguments: &str,
) -> anyhow::Result<u32> {
    let app_user_model_id = app_user_model_id.to_string();
    let arguments = arguments.to_string();
    tokio::task::spawn_blocking(move || {
        activate_packaged_app_blocking(&app_user_model_id, &arguments)
    })
    .await
    .context("packaged app activation task failed")?
}

#[cfg(windows)]
fn activate_packaged_app_blocking(app_user_model_id: &str, arguments: &str) -> anyhow::Result<u32> {
    use windows::Win32::System::Com::{
        CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::Shell::{ApplicationActivationManager, IApplicationActivationManager};
    use windows::core::HSTRING;

    unsafe {
        let coinit = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let should_uninitialize = coinit.is_ok();
        coinit.ok().or_else(|error| {
            const RPC_E_CHANGED_MODE: i32 = -2147417850;
            if error.code().0 == RPC_E_CHANGED_MODE {
                Ok(())
            } else {
                Err(error)
            }
        })?;

        let result: windows::core::Result<u32> = (|| {
            let manager: IApplicationActivationManager =
                CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)?;
            let process_id = manager.ActivateApplication(
                &HSTRING::from(app_user_model_id),
                &HSTRING::from(arguments),
                windows::Win32::UI::Shell::ACTIVATEOPTIONS(0),
            )?;
            Ok(process_id)
        })();

        if should_uninitialize {
            CoUninitialize();
        }
        result.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_launch_guard_stops_after_stable_ready_artifacts() {
        let artifacts = crate::computer_use_guard::GuardArtifacts {
            notify_exe: Some(PathBuf::from("codex-computer-use.exe")),
            marketplace_path: Some(PathBuf::from("openai-bundled")),
            sky_package_json: None,
            runtime_exports_needed: false,
        };

        assert!(!should_stop_post_launch_computer_use_guard(2, &artifacts));
        assert!(should_stop_post_launch_computer_use_guard(3, &artifacts));
    }

    #[test]
    fn post_launch_guard_keeps_retrying_until_artifacts_are_ready() {
        let missing_notify = crate::computer_use_guard::GuardArtifacts {
            notify_exe: None,
            marketplace_path: Some(PathBuf::from("openai-bundled")),
            sky_package_json: None,
            runtime_exports_needed: false,
        };
        let missing_marketplace = crate::computer_use_guard::GuardArtifacts {
            notify_exe: Some(PathBuf::from("codex-computer-use.exe")),
            marketplace_path: None,
            sky_package_json: None,
            runtime_exports_needed: false,
        };
        let missing_runtime_package = crate::computer_use_guard::GuardArtifacts {
            notify_exe: Some(PathBuf::from("codex-computer-use.exe")),
            marketplace_path: Some(PathBuf::from("openai-bundled")),
            sky_package_json: None,
            runtime_exports_needed: true,
        };

        assert!(!should_stop_post_launch_computer_use_guard(
            3,
            &missing_notify
        ));
        assert!(!should_stop_post_launch_computer_use_guard(
            3,
            &missing_marketplace
        ));
        assert!(!should_stop_post_launch_computer_use_guard(
            3,
            &missing_runtime_package
        ));
    }
}
