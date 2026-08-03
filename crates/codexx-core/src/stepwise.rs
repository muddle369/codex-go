use std::time::Duration;

use anyhow::Context;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::settings::BackendSettings;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepwiseRequest {
    #[serde(default)]
    pub last_user_message: String,
    #[serde(default)]
    pub last_assistant_message: String,
    #[serde(default)]
    pub thread_title: String,
    #[serde(default)]
    pub page_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepwiseItem {
    pub label: String,
    pub prompt: String,
}

pub fn public_settings(settings: &BackendSettings) -> Value {
    json!({
        "enabled": settings.codex_app_stepwise_enabled,
        "directSend": settings.codex_app_stepwise_direct_send,
        "baseUrlConfigured": !settings.codex_app_stepwise_base_url.trim().is_empty(),
        "apiKeyConfigured": !stepwise_api_key(settings).is_empty(),
        "model": settings.codex_app_stepwise_model,
        "maxItems": settings.codex_app_stepwise_max_items,
        "maxInputChars": settings.codex_app_stepwise_max_input_chars,
        "maxOutputTokens": settings.codex_app_stepwise_max_output_tokens,
        "timeoutMs": settings.codex_app_stepwise_timeout_ms,
    })
}

pub async fn generate(
    request: StepwiseRequest,
    settings: &BackendSettings,
) -> anyhow::Result<Value> {
    if !settings.codex_app_stepwise_enabled {
        return Ok(json!({"status":"ok","disabled":true,"items":[]}));
    }
    let base_url = settings
        .codex_app_stepwise_base_url
        .trim()
        .trim_end_matches('/');
    let model = settings.codex_app_stepwise_model.trim();
    let api_key = stepwise_api_key(settings);
    if base_url.is_empty() || model.is_empty() || api_key.is_empty() {
        return Ok(
            json!({"status":"failed","items":[],"error":"Stepwise Base URL、模型或 API Key 未配置"}),
        );
    }
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {api_key}"))?,
    );
    let response = reqwest::Client::new()
        .post(format!("{base_url}/chat/completions"))
        .headers(headers)
        .timeout(Duration::from_millis(
            settings.codex_app_stepwise_timeout_ms,
        ))
        .json(&json!({
            "model": model,
            "messages": build_messages(&request, settings),
            "temperature": 0.2,
            "max_tokens": settings.codex_app_stepwise_max_output_tokens,
            "response_format": {"type":"json_object"}
        }))
        .send()
        .await
        .context("Stepwise 请求失败")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Ok(
            json!({"status":"failed","items":[],"error":format!("上游 HTTP {}：{}", status.as_u16(), body.chars().take(240).collect::<String>())}),
        );
    }
    let payload: Value = serde_json::from_str(&body).context("Stepwise 返回不是合法 JSON")?;
    Ok(
        json!({"status":"ok","items":extract_items(&payload, settings.codex_app_stepwise_max_items)}),
    )
}

pub async fn test_connection(settings: &BackendSettings) -> anyhow::Result<Value> {
    generate(
        StepwiseRequest {
            last_user_message: "测试连接".to_string(),
            last_assistant_message: "请返回一条简短的下一步建议".to_string(),
            ..Default::default()
        },
        settings,
    )
    .await
}

fn build_messages(request: &StepwiseRequest, settings: &BackendSettings) -> Vec<Value> {
    let limit = settings.codex_app_stepwise_max_input_chars as usize;
    let user = shorten(&request.last_user_message, limit / 3);
    let assistant = shorten(&request.last_assistant_message, limit / 2);
    vec![
        json!({"role":"system","content":format!("Return strict JSON only: {{\"items\":[{{\"prompt\":\"...\",\"label\":\"...\"}}]}}. Generate at most {} concise next actions in the user's language.", settings.codex_app_stepwise_max_items)}),
        json!({"role":"user","content":json!({"lastUserMessage":user,"lastAssistantMessage":assistant,"threadTitle":shorten(&request.thread_title,240),"pageUrl":shorten(&request.page_url,240)}).to_string()}),
    ]
}

fn extract_items(payload: &Value, max_items: u8) -> Vec<StepwiseItem> {
    let candidates = [
        payload
            .get("choices")
            .and_then(|v| v.get(0))
            .and_then(|v| v.get("message"))
            .and_then(|v| v.get("content")),
        payload.get("output"),
        payload.get("result"),
        Some(payload),
    ];
    for candidate in candidates.into_iter().flatten() {
        let parsed = candidate
            .as_str()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .unwrap_or_else(|| candidate.clone());
        let items = parsed
            .get("items")
            .or_else(|| parsed.get("suggestions"))
            .or_else(|| parsed.get("actions"))
            .and_then(Value::as_array);
        if let Some(items) = items {
            let mut result = Vec::new();
            for item in items {
                let prompt = item
                    .get("prompt")
                    .or_else(|| item.get("text"))
                    .or_else(|| item.get("action"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if prompt.is_empty()
                    || result
                        .iter()
                        .any(|existing: &StepwiseItem| existing.prompt == prompt)
                {
                    continue;
                }
                result.push(StepwiseItem {
                    label: item
                        .get("label")
                        .or_else(|| item.get("title"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                    prompt: shorten(prompt, 420),
                });
                if result.len() >= max_items as usize {
                    break;
                }
            }
            return result;
        }
    }
    Vec::new()
}

fn stepwise_api_key(settings: &BackendSettings) -> String {
    let direct = settings.codex_app_stepwise_api_key.trim();
    if !direct.is_empty() {
        return direct.to_string();
    }
    std::env::var(settings.codex_app_stepwise_api_key_env.trim())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn shorten(value: &str, max: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max {
        return value.to_string();
    }
    value.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::extract_items;
    use serde_json::json;

    #[test]
    fn extracts_and_limits_stepwise_items() {
        let items = extract_items(
            &json!({"items":[{"prompt":"A"},{"prompt":"A"},{"prompt":"B"}]}),
            2,
        );
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].prompt, "A");
    }
}
