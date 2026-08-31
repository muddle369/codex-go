use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::Serialize;
use toml_edit::{DocumentMut, Item, Table};

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginMaintenanceReport {
    pub config_path: String,
    pub config_exists: bool,
    pub config_valid: bool,
    pub plugin_count: usize,
    pub marketplace_count: usize,
    pub enabled_plugin_count: usize,
    pub issues: Vec<String>,
    pub backup_path: Option<String>,
}

pub fn inspect_plugin_configuration(home: &Path) -> anyhow::Result<PluginMaintenanceReport> {
    let config_path = home.join("config.toml");
    let mut report = PluginMaintenanceReport {
        config_path: config_path.to_string_lossy().into_owned(),
        config_exists: config_path.is_file(),
        ..Default::default()
    };
    if !report.config_exists {
        report.issues.push("未找到 config.toml，尚未建立插件配置。".to_string());
        return Ok(report);
    }

    let contents = std::fs::read_to_string(&config_path)
        .with_context(|| format!("读取 {} 失败", config_path.display()))?;
    let document = match contents.trim_start_matches('\u{feff}').parse::<DocumentMut>() {
        Ok(document) => document,
        Err(error) => {
            report.issues.push(format!("config.toml 无法解析：{error}"));
            return Ok(report);
        }
    };
    report.config_valid = true;
    inspect_context_table(&document, "plugins", &mut report, true, home);
    inspect_context_table(&document, "marketplaces", &mut report, false, home);
    Ok(report)
}

pub fn repair_plugin_configuration(home: &Path) -> anyhow::Result<PluginMaintenanceReport> {
    let config_path = home.join("config.toml");
    let contents = std::fs::read_to_string(&config_path)
        .with_context(|| format!("读取 {} 失败", config_path.display()))?;
    let mut document = contents
        .trim_start_matches('\u{feff}')
        .parse::<DocumentMut>()
        .map_err(|error| anyhow::anyhow!("config.toml 无法解析，未执行修复：{error}"))?;
    if let Some(plugins) = document.get_mut("plugins").and_then(Item::as_table_mut) {
        for (_, item) in plugins.iter_mut() {
            if let Some(table) = item.as_table_mut() {
                if table.get("enabled").is_none() {
                    table["enabled"] = toml_edit::value(true);
                }
            }
        }
    }
    let normalized = ensure_trailing_newline(document.to_string());
    let backup_path = create_backup(&config_path)?;
    let temporary_path = config_path.with_extension("toml.codexgo-repair.tmp");
    if let Err(error) = (|| -> anyhow::Result<()> {
        std::fs::write(&temporary_path, normalized.as_bytes())?;
        std::fs::rename(&temporary_path, &config_path)?;
        Ok(())
    })() {
        let _ = std::fs::remove_file(&temporary_path);
        let _ = std::fs::copy(&backup_path, &config_path);
        return Err(error).context("写入修复后的 config.toml 失败，已尝试恢复备份");
    }
    let mut report = inspect_plugin_configuration(home)?;
    report.backup_path = Some(backup_path.to_string_lossy().into_owned());
    Ok(report)
}

pub fn restore_latest_plugin_configuration_backup(home: &Path) -> anyhow::Result<PluginMaintenanceReport> {
    let config_path = home.join("config.toml");
    let backup_path = latest_backup(&config_path)
        .ok_or_else(|| anyhow::anyhow!("没有找到可恢复的 CodexGO 插件配置备份"))?;
    if config_path.is_file() {
        create_backup(&config_path)?;
    }
    let temporary_path = config_path.with_extension("toml.codexgo-restore.tmp");
    if let Err(error) = (|| -> anyhow::Result<()> {
        std::fs::copy(&backup_path, &temporary_path)?;
        std::fs::rename(&temporary_path, &config_path)?;
        Ok(())
    })() {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error).context("恢复插件配置备份失败");
    }
    let mut report = inspect_plugin_configuration(home)?;
    report.backup_path = Some(backup_path.to_string_lossy().into_owned());
    Ok(report)
}

fn inspect_context_table(
    document: &DocumentMut,
    table_name: &str,
    report: &mut PluginMaintenanceReport,
    is_plugin_table: bool,
    home: &Path,
) {
    let Some(item) = document.get(table_name) else {
        return;
    };
    let Some(table) = item.as_table() else {
        report.issues.push(format!("[{table_name}] 不是有效的 TOML 表。"));
        return;
    };
    if is_plugin_table {
        report.plugin_count = table.len();
    } else {
        report.marketplace_count = table.len();
    }
    for (id, item) in table.iter() {
        let Some(entry) = item.as_table() else {
            report.issues.push(format!("{table_name}.{id} 不是有效的 TOML 表。"));
            continue;
        };
        if is_plugin_table {
            if entry.get("enabled").is_none() {
                report.issues.push(format!("插件 {id} 缺少 enabled 状态，修复时会默认设为 true。"));
            }
            if entry.get("enabled").is_some_and(|value| value.as_bool().is_none()) {
                report.issues.push(format!("插件 {id} 的 enabled 不是布尔值。"));
            }
            if entry.get("enabled").and_then(Item::as_bool).unwrap_or(true) {
                report.enabled_plugin_count += 1;
            }
        } else {
            inspect_marketplace_source(id, entry, report, home);
        }
    }
}

fn latest_backup(config_path: &Path) -> Option<PathBuf> {
    let parent = config_path.parent()?;
    let mut backups = std::fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("config.toml.codexgo-backup-"))
        })
        .collect::<Vec<_>>();
    backups.sort();
    backups.pop()
}

fn inspect_marketplace_source(id: &str, table: &Table, report: &mut PluginMaintenanceReport, home: &Path) {
    let source_type = table.get("source_type").and_then(Item::as_str).unwrap_or_default();
    let source = table.get("source").and_then(Item::as_str).unwrap_or_default();
    if source_type == "local" && source.trim().is_empty() {
        report.issues.push(format!("市场 {id} 声明为 local，但没有 source 路径。"));
    } else if source_type == "local" && !is_remote_source(source) && !resolve_local_source(home, source).exists() {
        report.issues.push(format!("市场 {id} 的本地目录不存在：{source}"));
    }
}

fn resolve_local_source(home: &Path, source: &str) -> PathBuf {
    let path = PathBuf::from(source);
    if path.is_absolute() { path } else { home.join(path) }
}

fn is_remote_source(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.starts_with("http://") || value.starts_with("https://") || value.starts_with("remote:")
}

fn create_backup(path: &Path) -> anyhow::Result<PathBuf> {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let backup = path.with_file_name(format!("config.toml.codexgo-backup-{timestamp}"));
    std::fs::copy(path, &backup)
        .with_context(|| format!("创建 {} 备份失败", path.display()))?;
    Ok(backup)
}

fn ensure_trailing_newline(mut value: String) -> String {
    while value.ends_with("\n\n") {
        value.pop();
    }
    if !value.ends_with('\n') {
        value.push('\n');
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_plugins_and_missing_local_marketplace() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("config.toml"),
            r#"[plugins.browser]
enabled = true

[plugins.disabled]
enabled = false

[marketplaces.local]
source_type = "local"
source = "/path/that/does/not/exist"
"#,
        )
        .unwrap();
        let report = inspect_plugin_configuration(temp.path()).unwrap();
        assert!(report.config_valid);
        assert_eq!(report.plugin_count, 2);
        assert_eq!(report.enabled_plugin_count, 1);
        assert_eq!(report.marketplace_count, 1);
        assert_eq!(report.issues.len(), 1);
    }

    #[test]
    fn repair_creates_backup_and_keeps_content() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.toml");
        std::fs::write(&config, "[plugins.browser]\nenabled = true").unwrap();
        let report = repair_plugin_configuration(temp.path()).unwrap();
        assert!(report.backup_path.is_some());
        assert!(Path::new(report.backup_path.as_ref().unwrap()).is_file());
        assert_eq!(std::fs::read_to_string(config).unwrap(), "[plugins.browser]\nenabled = true\n");
    }

    #[test]
    fn repair_adds_missing_enabled_state() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.toml");
        std::fs::write(&config, "[plugins.browser]\n").unwrap();
        let before = inspect_plugin_configuration(temp.path()).unwrap();
        assert_eq!(before.issues.len(), 1);
        let after = repair_plugin_configuration(temp.path()).unwrap();
        assert!(after.issues.is_empty());
        assert!(std::fs::read_to_string(config).unwrap().contains("enabled = true"));
    }

    #[test]
    fn restore_uses_latest_backup() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.toml");
        std::fs::write(&config, "[plugins.current]\nenabled = true\n").unwrap();
        std::fs::write(temp.path().join("config.toml.codexgo-backup-1"), "[plugins.old]\nenabled = true\n").unwrap();
        let report = restore_latest_plugin_configuration_backup(temp.path()).unwrap();
        assert_eq!(report.plugin_count, 1);
        assert!(std::fs::read_to_string(config).unwrap().contains("plugins.old"));
    }
}
