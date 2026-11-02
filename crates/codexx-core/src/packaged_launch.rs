use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};

pub const MAIN_WRAPPER_FILE_NAME: &str = "packaged-launch-main.cmd";
pub const AUMID_ACTIVATION_CALL_TIMEOUT: Duration = Duration::from_secs(20);
pub const AUMID_ACTIVATION_VERIFY_WINDOW: Duration = Duration::from_secs(15);
pub const AUMID_ACTIVATION_EXIT_GRACE: Duration = Duration::from_secs(2);
const SCRUBBED_CODEX_ENV_VARS: [&str; 3] =
    ["ELECTRON_RUN_AS_NODE", "ELECTRON_NO_ASAR", "NODE_OPTIONS"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AumidActivationVerdict {
    Ready,
    Pending,
    FallbackToPackageContext,
    Inconclusive,
}

pub fn observe_aumid_activation(
    cdp_ready: bool,
    process_alive: Option<bool>,
    elapsed: Duration,
    exited_for: Duration,
) -> AumidActivationVerdict {
    if cdp_ready {
        return AumidActivationVerdict::Ready;
    }
    if process_alive == Some(false) && exited_for >= AUMID_ACTIVATION_EXIT_GRACE {
        return AumidActivationVerdict::FallbackToPackageContext;
    }
    if elapsed >= AUMID_ACTIVATION_VERIFY_WINDOW {
        return AumidActivationVerdict::Inconclusive;
    }
    AumidActivationVerdict::Pending
}

pub fn split_app_user_model_id(value: &str) -> anyhow::Result<(String, String)> {
    let value = value.trim();
    let Some((package_family_name, app_id)) = value.split_once('!') else {
        bail!("AUMID 缺少 `!` 分隔符：{value}");
    };
    if package_family_name.is_empty() || app_id.is_empty() || value.matches('!').count() != 1 {
        bail!("AUMID 形状不合法：{value}");
    }
    Ok((package_family_name.to_string(), app_id.to_string()))
}

pub fn build_main_instance_wrapper_script(
    executable: &Path,
    args: &[String],
) -> anyhow::Result<String> {
    let executable = batch_path_literal(executable)?;
    let executable_dir = executable
        .rsplit_once('\\')
        .map(|(directory, _)| directory.to_string());
    let args = args
        .iter()
        .map(|arg| batch_argument_literal(arg))
        .collect::<anyhow::Result<Vec<_>>>()?;

    let mut script = String::from("@echo off\r\nsetlocal\r\n");
    for name in SCRUBBED_CODEX_ENV_VARS {
        script.push_str(&format!("set \"{name}=\"\r\n"));
    }
    if let Some(directory) = executable_dir {
        script.push_str(&format!("cd /d \"{directory}\"\r\n"));
    }
    script.push_str(&format!("start \"\" /b \"{executable}\""));
    for arg in args {
        script.push(' ');
        script.push_str(&arg);
    }
    script.push_str("\r\nendlocal\r\n");
    Ok(script)
}

pub fn prepare_main_instance_wrapper(
    state_dir: &Path,
    executable: &Path,
    args: &[String],
) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("创建启动状态目录失败：{}", state_dir.display()))?;
    let path = state_dir.join(MAIN_WRAPPER_FILE_NAME);
    let script = build_main_instance_wrapper_script(executable, args)?;
    std::fs::write(&path, script.as_bytes())
        .with_context(|| format!("写入包上下文 wrapper 失败：{}", path.display()))?;
    Ok(path)
}

pub fn activation_script(app_user_model_id: &str, wrapper_path: &Path) -> anyhow::Result<String> {
    let (package_family_name, app_id) = split_app_user_model_id(app_user_model_id)?;
    let package_family_name = powershell_literal(&package_family_name)?;
    let app_id = powershell_literal(&app_id)?;
    let wrapper_path = powershell_literal(&path_text(wrapper_path)?)?;
    Ok(format!(
        "$ErrorActionPreference = 'Stop'; Invoke-CommandInDesktopPackage -PackageFamilyName '{package_family_name}' -AppId '{app_id}' -Command 'cmd.exe' -Args '/d /c \"{wrapper_path}\"' -PreventBreakaway"
    ))
}

pub async fn activate_in_package_context(
    app_user_model_id: &str,
    wrapper_path: &Path,
) -> anyhow::Result<()> {
    let script = activation_script(app_user_model_id, wrapper_path)?;

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};

        let powershell = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .map(|root| root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
            .unwrap_or_else(|| PathBuf::from("powershell.exe"));
        let task = tokio::task::spawn_blocking(move || {
            let output = Command::new(&powershell)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    &script,
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(crate::windows_integration::CREATE_NO_WINDOW)
                .env_remove("ELECTRON_RUN_AS_NODE")
                .env_remove("ELECTRON_NO_ASAR")
                .env_remove("NODE_OPTIONS")
                .output()
                .with_context(|| {
                    format!("启动包上下文 PowerShell 失败：{}", powershell.display())
                })?;
            if !output.status.success() {
                bail!(
                    "Invoke-CommandInDesktopPackage 失败（{}）：{}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            let _ = output;
            Ok::<(), anyhow::Error>(())
        });
        return tokio::time::timeout(AUMID_ACTIVATION_CALL_TIMEOUT, task)
            .await
            .context("包上下文激活超时")?
            .context("包上下文激活任务异常退出")?;
    }

    #[cfg(not(windows))]
    {
        let _ = script;
        bail!("包上下文激活仅在 Windows 上可用");
    }
}

pub fn wait_for_instance_ready(debug_port: u16, timeout: Duration) -> bool {
    if debug_port == 0 {
        return false;
    }
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if crate::cdp::endpoint_available(debug_port) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn batch_path_literal(path: &Path) -> anyhow::Result<String> {
    let text = path_text(path)?;
    Ok(text.replace('%', "%%"))
}

fn batch_argument_literal(value: &str) -> anyhow::Result<String> {
    if value.contains(['"', '\r', '\n', '\0']) {
        bail!("启动参数包含无法写入 wrapper 的字符");
    }
    let value = value.replace('%', "%%");
    let needs_quotes = value.is_empty()
        || value.chars().any(|character| {
            character.is_whitespace()
                || matches!(character, '&' | '|' | '<' | '>' | '^' | '(' | ')')
        });
    Ok(if needs_quotes {
        format!("\"{value}\"")
    } else {
        value
    })
}

fn path_text(path: &Path) -> anyhow::Result<String> {
    let text = path.to_string_lossy();
    if text.is_empty() || text.contains(['"', '\r', '\n', '\0']) {
        bail!("路径包含无法写入 wrapper 的字符");
    }
    Ok(text.into_owned())
}

fn powershell_literal(value: &str) -> anyhow::Result<String> {
    if value.contains(['\r', '\n', '\0']) {
        bail!("PowerShell 参数包含非法字符");
    }
    Ok(value.replace('"', "\"\"").replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_valid_aumid() {
        assert_eq!(
            split_app_user_model_id("OpenAI.Codex_abc!App").unwrap(),
            ("OpenAI.Codex_abc".to_string(), "App".to_string())
        );
    }

    #[test]
    fn rejects_malformed_aumid() {
        assert!(split_app_user_model_id("OpenAI.Codex_abc").is_err());
        assert!(split_app_user_model_id("OpenAI.Codex_abc!App!Extra").is_err());
    }

    #[test]
    fn only_confirmed_early_process_exit_triggers_package_context_fallback() {
        assert_eq!(
            observe_aumid_activation(
                false,
                Some(false),
                Duration::from_secs(3),
                AUMID_ACTIVATION_EXIT_GRACE,
            ),
            AumidActivationVerdict::FallbackToPackageContext
        );
        assert_eq!(
            observe_aumid_activation(false, None, AUMID_ACTIVATION_VERIFY_WINDOW, Duration::ZERO,),
            AumidActivationVerdict::Inconclusive
        );
        assert_eq!(
            observe_aumid_activation(
                false,
                Some(true),
                AUMID_ACTIVATION_VERIFY_WINDOW,
                Duration::ZERO,
            ),
            AumidActivationVerdict::Inconclusive
        );
    }
}
