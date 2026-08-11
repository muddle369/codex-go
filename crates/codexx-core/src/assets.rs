use base64::Engine;
use serde_json::{Value, json};
use std::path::Path;

use crate::settings::BackendSettings;

const RENDERER_SCRIPT: &str = include_str!("../../../assets/inject/renderer-inject.js");
const AUDIO_TRANSCRIPTION_SCRIPT: &str =
    include_str!("../../../assets/inject/audio-transcription-inject.js");
#[cfg(windows)]
const DREAM_SKIN_CSS: &str =
    include_str!("../../../assets/inject/upstream/dream-skin/windows/dream-skin.css");
#[cfg(not(windows))]
const DREAM_SKIN_CSS: &str =
    include_str!("../../../assets/inject/upstream/dream-skin/macos/dream-skin.css");
#[cfg(windows)]
const DREAM_SKIN_RENDERER: &str =
    include_str!("../../../assets/inject/upstream/dream-skin/windows/renderer-inject.js");
#[cfg(not(windows))]
const DREAM_SKIN_RENDERER: &str =
    include_str!("../../../assets/inject/upstream/dream-skin/macos/renderer-inject.js");
#[cfg(windows)]
const CIDALA_SKIN_CSS: &str =
    include_str!("../../../assets/inject/upstream/cidala-tiger/windows/dream-skin.css");
#[cfg(not(windows))]
const CIDALA_SKIN_CSS: &str =
    include_str!("../../../assets/inject/upstream/cidala-tiger/macos/dream-skin.css");
#[cfg(windows)]
const CIDALA_SKIN_RENDERER: &str =
    include_str!("../../../assets/inject/upstream/cidala-tiger/windows/renderer-inject.js");
#[cfg(not(windows))]
const CIDALA_SKIN_RENDERER: &str =
    include_str!("../../../assets/inject/upstream/cidala-tiger/macos/renderer-inject.js");
const SNOW_SKIN_CSS: &str =
    include_str!("../../../assets/inject/upstream/snow-skin/dream-skin.css");
const SNOW_SKIN_RENDERER: &str =
    include_str!("../../../assets/inject/upstream/snow-skin/renderer-inject.js");
const GLASS_VISION_CSS: &str =
    include_str!("../../../assets/inject/upstream/glass-vision/glass-vision.css");
const GLASS_VISION_RENDERER: &str =
    include_str!("../../../assets/inject/upstream/glass-vision/renderer-inject.js");
const SPONSOR_ALIPAY: &[u8] = include_bytes!("../../../assets/images/feng-alipay.JPG");
const SPONSOR_WECHAT: &[u8] = include_bytes!("../../../assets/images/feng-wechat.JPG");
pub const DIAGNOSTIC_BUILD_ID: &str = "diag-20260518-1";

pub fn renderer_script() -> &'static str {
    RENDERER_SCRIPT
}

pub fn sponsor_image_data_uris() -> Value {
    json!({
        "alipay": image_data_uri("image/jpeg", SPONSOR_ALIPAY),
        "wechat": image_data_uri("image/jpeg", SPONSOR_WECHAT),
    })
}

pub fn injection_script(helper_port: u16) -> String {
    injection_script_with_settings(helper_port, &BackendSettings::default())
}

pub fn audio_transcription_injection_script(helper_port: u16) -> String {
    let endpoint = format!("http://127.0.0.1:{helper_port}/v1/audio/transcriptions");
    AUDIO_TRANSCRIPTION_SCRIPT.replace(
        "__CODEXGO_AUDIO_TRANSCRIPTION_ENDPOINT__",
        &serde_json::to_string(&endpoint).expect("audio transcription endpoint should serialize"),
    )
}

pub fn injection_script_with_settings(helper_port: u16, settings: &BackendSettings) -> String {
    let helper_url = format!("http://127.0.0.1:{helper_port}");
    let sponsor_images = sponsor_image_data_uris();
    let image_overlay = image_overlay_config(helper_port, settings);
    let dream_skin = dream_skin_config(settings);
    let companion = composer_companion_config(settings);
    let stepwise = stepwise_config(settings);
    let dream_skin_runtime = dream_skin_runtime_script(settings);
    format!(
        "window.__CODEX_SESSION_DELETE_HELPER__ = {};\nwindow.__CODEX_PLUS_SPONSOR_IMAGES__ = {};\nwindow.__CODEX_PLUS_VERSION__ = {};\nwindow.__CODEX_PLUS_BUILD__ = {};\nwindow.__CODEX_PLUS_IMAGE_OVERLAY__ = {};\nwindow.__CODEX_PLUS_DREAM_SKIN__ = {};\nwindow.__CODEX_PLUS_COMPOSER_COMPANION__ = {};\nwindow.__CODEX_PLUS_STEPWISE__ = {};\nwindow.__CODEX_PLUS_PASTE_FIX__ = {};\n{}\n{}",
        serde_json::to_string(&helper_url).expect("helper URL should serialize"),
        serde_json::to_string(&sponsor_images).expect("sponsor images should serialize"),
        serde_json::to_string(crate::version::VERSION).expect("version should serialize"),
        serde_json::to_string(DIAGNOSTIC_BUILD_ID).expect("build id should serialize"),
        serde_json::to_string(&image_overlay).expect("image overlay config should serialize"),
        serde_json::to_string(&dream_skin).expect("dream skin config should serialize"),
        serde_json::to_string(&companion).expect("companion config should serialize"),
        serde_json::to_string(&stepwise).expect("stepwise config should serialize"),
        serde_json::to_string(&json!({"enabled": settings.codex_app_paste_fix}))
            .expect("paste fix config should serialize"),
        renderer_script(),
        dream_skin_runtime,
    )
}

fn dream_skin_config(settings: &BackendSettings) -> Value {
    let background = if settings.codex_app_dream_skin_enabled {
        image_file_data_uri(Path::new(
            settings.codex_app_dream_skin_background_path.trim(),
        ))
        .unwrap_or_default()
    } else {
        String::new()
    };
    json!({
        "enabled": settings.codex_app_dream_skin_enabled,
        "theme": settings.codex_app_dream_skin_theme,
        "accent": settings.codex_app_dream_skin_accent,
        "themeConfig": settings.codex_app_dream_skin_theme_config,
        "backgroundDataUrl": background,
    })
}

fn dream_skin_runtime_script(settings: &BackendSettings) -> String {
    if !settings.codex_app_dream_skin_enabled {
        return String::new();
    }
    let dream_skin = dream_skin_config(settings);
    let art = dream_skin
        .get("backgroundDataUrl")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut theme = settings.codex_app_dream_skin_theme_config.clone();
    if !theme.is_object() || theme.as_object().is_some_and(|object| object.is_empty()) {
        theme = json!({
            "schemaVersion": 1,
            "id": settings.codex_app_dream_skin_theme,
            "name": settings.codex_app_dream_skin_theme,
            "colors": {"accent": settings.codex_app_dream_skin_accent}
        });
    }
    let style_preset = theme
        .get("stylePreset")
        .or_else(|| theme.get("style_preset"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (renderer, css) = match style_preset {
        "codex-snow" => (SNOW_SKIN_RENDERER, SNOW_SKIN_CSS),
        "glass-vision" => (GLASS_VISION_RENDERER, GLASS_VISION_CSS),
        "midnight-aurora" | "amber-dusk" | "forest-mist" | "cyber-neon" | "sakura-dawn" => {
            (CIDALA_SKIN_RENDERER, CIDALA_SKIN_CSS)
        }
        _ => (DREAM_SKIN_RENDERER, DREAM_SKIN_CSS),
    };
    let css_json = serde_json::to_string(css).unwrap();
    let art_json = serde_json::to_string(art).unwrap();
    let theme_json = serde_json::to_string(&theme).unwrap();
    let style_revision = serde_json::to_string(&format!("codexgo-{}", css.len())).unwrap();
    let payload_revision =
        serde_json::to_string(&format!("codexgo-{}", theme.to_string().len())).unwrap();
    let payload = renderer
        .replace("__DREAM_CSS_JSON__", &css_json)
        .replace("__DREAM_ART_JSON__", &art_json)
        .replace(
            "__DREAM_VERSION_JSON__",
            &serde_json::to_string(crate::version::VERSION).unwrap(),
        )
        .replace("__GLASS_VISION_CSS_JSON__", &css_json)
        .replace("__GLASS_VISION_ART_JSON__", &art_json)
        .replace("__DREAM_SKIN_CSS_JSON__", &css_json)
        .replace("__DREAM_SKIN_ART_JSON__", &art_json)
        .replace("__DREAM_THEME_JSON__", &theme_json)
        .replace("__DREAM_SKIN_THEME_JSON__", &theme_json)
        .replace(
            "__DREAM_SKIN_VERSION_JSON__",
            &serde_json::to_string(crate::version::VERSION).unwrap(),
        )
        .replace("__DREAM_SKIN_STYLE_REVISION_JSON__", &style_revision)
        .replace("__DREAM_SKIN_PAYLOAD_REVISION_JSON__", &payload_revision);
    if payload.contains("__DREAM_") || payload.contains("__GLASS_VISION_") {
        return String::new();
    }
    payload
}

fn composer_companion_config(settings: &BackendSettings) -> Value {
    let data_url = if settings.codex_app_composer_companion_enabled {
        image_file_data_uri(Path::new(settings.codex_app_composer_companion_path.trim()))
            .unwrap_or_default()
    } else {
        String::new()
    };
    json!({
        "enabled": settings.codex_app_composer_companion_enabled && !data_url.is_empty(),
        "dataUrl": data_url,
        "width": settings.codex_app_composer_companion_width,
        "side": settings.codex_app_composer_companion_side,
        "offsetX": settings.codex_app_composer_companion_offset_x,
        "offsetY": settings.codex_app_composer_companion_offset_y,
    })
}

fn stepwise_config(settings: &BackendSettings) -> Value {
    json!({
        "enabled": settings.codex_app_stepwise_enabled,
        "directSend": settings.codex_app_stepwise_direct_send,
        "maxItems": settings.codex_app_stepwise_max_items,
    })
}

pub fn image_overlay_config(helper_port: u16, settings: &BackendSettings) -> Value {
    let has_path = !settings.codex_app_image_overlay_path.trim().is_empty();
    let enabled = settings.codex_app_image_overlay_enabled && has_path;
    let data_url = if enabled {
        image_file_data_uri(Path::new(settings.codex_app_image_overlay_path.trim()))
            .unwrap_or_default()
    } else {
        String::new()
    };
    json!({
        "enabled": enabled && !data_url.is_empty(),
        "opacity": f64::from(settings.codex_app_image_overlay_opacity.clamp(1, 100)) / 100.0,
        "dataUrl": data_url,
        "imageUrl": if enabled {
            format!("http://127.0.0.1:{helper_port}/overlay/image")
        } else {
            String::new()
        },
    })
}

fn image_data_uri(mime_type: &str, bytes: &[u8]) -> String {
    format!(
        "data:{mime_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

fn image_file_data_uri(path: &Path) -> Option<String> {
    let mime_type = image_content_type(path)?;
    let bytes = std::fs::read(path).ok()?;
    Some(image_data_uri(mime_type, &bytes))
}

fn image_content_type(path: &Path) -> Option<&'static str> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injection_includes_resolved_dream_skin_runtime() {
        let settings = BackendSettings {
            codex_app_dream_skin_enabled: true,
            codex_app_dream_skin_theme: "Test Theme".to_string(),
            codex_app_dream_skin_theme_config: json!({
                "id": "test-theme",
                "name": "Test Theme",
                "stylePreset": "midnight-aurora",
                "colors": {"accent": "#8b7cff"}
            }),
            ..BackendSettings::default()
        };
        let script = injection_script_with_settings(49152, &settings);
        assert!(script.contains("codex-dream-skin"));
        assert!(script.contains("test-theme"));
        assert!(script.contains(crate::version::VERSION));
        assert!(!script.contains("version: \"1.2.0\""));
        assert!(!script.contains("__DREAM_THEME_JSON__"));
        assert!(!script.contains("__DREAM_SKIN_CSS_JSON__"));
        assert!(!script.contains("__DREAM_SKIN_THEME_JSON__"));
    }

    #[test]
    fn windows_dream_skin_renderers_use_version_placeholder() {
        let renderers = [
            include_str!("../../../assets/inject/upstream/dream-skin/windows/renderer-inject.js"),
            include_str!("../../../assets/inject/upstream/cidala-tiger/windows/renderer-inject.js"),
        ];
        for renderer in renderers {
            assert!(renderer.contains("__DREAM_SKIN_VERSION_JSON__"));
            assert!(!renderer.contains("version: \"1.2.0\""));
        }
    }

    #[test]
    fn audio_transcription_injection_only_redirects_to_local_helper() {
        let script = audio_transcription_injection_script(58321);

        assert!(script.contains("/transcribe"));
        assert!(script.contains("http://127.0.0.1:58321/v1/audio/transcriptions"));
        assert!(script.contains("x-codex-base64"));
        assert!(script.contains("x-codexgo-audio-language"));
        assert!(script.contains("x-openai-attach-auth"));
        assert!(!script.contains("codex-dream-skin"));
        assert!(!script.contains("codex-plus-menu"));
        assert!(!script.contains("CodexGO"));
    }

    #[test]
    fn renderer_injection_throttles_failed_model_patch_and_avoids_duplicate_diagnostics() {
        let script = injection_script(58321);

        assert!(script.contains("__codexPlusAppServerModelRequestPatchRetryAt"));
        assert!(script.contains("__codexPlusAppServerModelRequestPatchInFlight"));
        assert!(script.contains("return window.__codexSessionDeleteBridge"));
    }
}
