use std::time::Duration;

use anyhow::Context;
use serde_json::json;

pub const SANITIZE_LOCAL_STORAGE_SCRIPT: &str = r#"(() => {
  const key = '__codexDailyTokenUsageV1';
  const raw = localStorage.getItem(key);
  if (!raw) return { changed: false, cleanedTurns: 0 };
  let data;
  try { data = JSON.parse(raw); } catch (_) { return { changed: false, cleanedTurns: 0 }; }
  let changed = false;
  let cleanedTurns = 0;
  for (const day of Object.values(data.days || {})) {
    for (const turn of Object.values(day?.turns || {})) {
      if (typeof turn?.model !== 'string') continue;
      const next = turn.model.replace(/\[[^\]]+\]$/, '');
      if (next !== turn.model) { turn.model = next; changed = true; cleanedTurns += 1; }
    }
  }
  if (changed) localStorage.setItem(key, JSON.stringify(data));
  return { changed, cleanedTurns };
})()"#;

pub async fn sanitize_local_storage_model_suffixes(debug_port: u16) -> anyhow::Result<bool> {
    let target = crate::cdp::pick_page_target(&crate::cdp::list_targets(debug_port).await?)?;
    let websocket_url = target
        .web_socket_debugger_url
        .as_deref()
        .context("CDP target missing websocket URL")?;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        crate::bridge::evaluate_script_with_await_promise(
            websocket_url,
            SANITIZE_LOCAL_STORAGE_SCRIPT,
            false,
        ),
    )
    .await
    .context("localStorage cleanup timed out")??;
    let changed = result
        .get("result")
        .and_then(|value| value.get("value"))
        .and_then(|value| value.get("changed"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "codex_local_storage.sanitize_model_suffixes",
        json!({"debug_port": debug_port, "changed": changed}),
    );
    Ok(changed)
}

pub async fn sanitize_local_storage_model_suffixes_nonfatal(debug_port: u16) {
    if let Err(error) = sanitize_local_storage_model_suffixes(debug_port).await {
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "codex_local_storage.sanitize_model_suffixes_failed",
            json!({"debug_port": debug_port, "error": error.to_string()}),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::SANITIZE_LOCAL_STORAGE_SCRIPT;

    #[test]
    fn cleanup_script_targets_codex_daily_usage() {
        assert!(SANITIZE_LOCAL_STORAGE_SCRIPT.contains("__codexDailyTokenUsageV1"));
        assert!(SANITIZE_LOCAL_STORAGE_SCRIPT.contains("localStorage.setItem"));
    }
}
