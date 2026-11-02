use std::collections::HashMap;
use std::collections::VecDeque;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, bail};
use base64::Engine;
use futures_util::stream::FuturesUnordered;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

pub const BRIDGE_BINDING_NAME: &str = "codexSessionDeleteV2";
const APP_SERVER_CLIENT_CAPTURE_REQUEST_PATH: &str = "codex-app-server-client-capture";
const CDP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CDP_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

pub type BridgeHandler = Arc<
    dyn Fn(String, Value) -> Pin<Box<dyn Future<Output = anyhow::Result<Value>> + Send>>
        + Send
        + Sync,
>;

static NEXT_MESSAGE_ID: AtomicU64 = AtomicU64::new(100);

/// Bridge 会话按注入目标分代。
///
/// 同一目标再次安装 Bridge 时，旧会话会在下一次消息循环中退出并关闭 socket，
/// 避免多份 CDP 会话同时应答同一个页面请求。不同目标互不影响。
static NEXT_BRIDGE_GENERATION: AtomicU64 = AtomicU64::new(1);
static CURRENT_BRIDGE_GENERATIONS: std::sync::LazyLock<std::sync::Mutex<HashMap<String, u64>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));
static APP_SERVER_CLIENT_CAPTURE_GENERATIONS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<u64>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

fn claim_capture_generation(
    active: &mut std::collections::HashSet<u64>,
    generation_id: u64,
) -> bool {
    active.insert(generation_id)
}

#[derive(Clone)]
struct BridgeGeneration {
    target: String,
    id: u64,
}

type PendingBridgeCall = Pin<Box<dyn Future<Output = CompletedBridgeCall> + Send>>;

struct CompletedBridgeCall {
    request_id: String,
    generation: Option<BridgeGeneration>,
    response: Result<Value, String>,
}

fn next_bridge_generation(target: &str) -> BridgeGeneration {
    BridgeGeneration {
        target: target.to_string(),
        id: NEXT_BRIDGE_GENERATION.fetch_add(1, Ordering::SeqCst),
    }
}

fn publish_bridge_generation(generation: &BridgeGeneration) -> bool {
    let mut generations = CURRENT_BRIDGE_GENERATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if generations
        .get(&generation.target)
        .is_some_and(|current| *current > generation.id)
    {
        return false;
    }
    generations.insert(generation.target.clone(), generation.id);
    true
}

fn bridge_generation_is_current(generation: &BridgeGeneration) -> bool {
    CURRENT_BRIDGE_GENERATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&generation.target)
        .is_some_and(|current| *current == generation.id)
}

fn release_bridge_generation(generation: &BridgeGeneration) {
    let mut generations = CURRENT_BRIDGE_GENERATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if generations
        .get(&generation.target)
        .is_some_and(|current| *current == generation.id)
    {
        generations.remove(&generation.target);
    }
}

pub fn build_bridge_script(binding_name: &str) -> String {
    format!(
        r#"
(() => {{
  window.__codexSessionDeleteCallbacks = new Map();
  window.__codexSessionDeleteSeq = 0;
  window.__codexSessionDeleteResolve = (id, result) => {{
    const callback = window.__codexSessionDeleteCallbacks.get(id);
    if (!callback) return;
    window.__codexSessionDeleteCallbacks.delete(id);
    callback.resolve(result);
  }};
  window.__codexSessionDeleteReject = (id, message) => {{
    const callback = window.__codexSessionDeleteCallbacks.get(id);
    if (!callback) return;
    window.__codexSessionDeleteCallbacks.delete(id);
    callback.resolve({{ status: "failed", message }});
  }};
  window.__codexSessionDeleteBridge = (path, payload) => new Promise((resolve) => {{
    const id = String(++window.__codexSessionDeleteSeq);
    window.__codexSessionDeleteCallbacks.set(id, {{ resolve }});
    window.{binding_name}(JSON.stringify({{ id, path, payload }}));
  }});
}})();
"#
    )
}

pub fn bridge_health_check_script() -> &'static str {
    r#"
(() => {
  const bridge = window.__codexSessionDeleteBridge;
  if (typeof bridge !== "function") return false;
  try {
    return Promise.race([
      Promise.resolve(bridge("/backend/status", {})).then((result) => !!result && result.status === "ok"),
      new Promise((resolve) => setTimeout(() => resolve(false), 2000)),
    ]);
  } catch (error) {
    return false;
  }
})()
"#
}

pub async fn evaluate_script(websocket_url: &str, script: &str) -> anyhow::Result<Value> {
    evaluate_script_with_await_promise(websocket_url, script, false).await
}

pub async fn evaluate_script_with_await_promise(
    websocket_url: &str,
    script: &str,
    await_promise: bool,
) -> anyhow::Result<Value> {
    let socket = connect_cdp_websocket(websocket_url).await?;
    let mut session = CdpSession::new(socket);
    let response = session
        .send_command(
            1,
            "Runtime.evaluate",
            runtime_evaluate_params_with_await_promise(script, await_promise),
        )
        .await?;
    ensure_runtime_evaluate_succeeded(response)
}

pub fn capture_screenshot_params() -> Value {
    json!({
        "format": "png",
        "fromSurface": true,
        "captureBeyondViewport": false,
    })
}

pub async fn send_cdp_command(
    websocket_url: &str,
    method: &str,
    params: Value,
) -> anyhow::Result<Value> {
    let socket = connect_cdp_websocket(websocket_url).await?;
    let mut session = CdpSession::new(socket);
    session
        .send_command(next_message_id(), method, params)
        .await
}

pub async fn capture_page_screenshot(
    websocket_url: &str,
    output_path: &Path,
) -> anyhow::Result<u64> {
    let response = send_cdp_command(
        websocket_url,
        "Page.captureScreenshot",
        capture_screenshot_params(),
    )
    .await?;
    let encoded = response
        .get("result")
        .and_then(|result| result.get("data"))
        .and_then(Value::as_str)
        .filter(|data| !data.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Page.captureScreenshot returned no image data"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("failed to decode screenshot PNG")?;
    if !bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]) {
        bail!("Page.captureScreenshot returned invalid PNG data");
    }
    crate::settings::atomic_write(output_path, &bytes)
        .with_context(|| format!("failed to save screenshot {}", output_path.display()))?;
    Ok(bytes.len() as u64)
}

pub async fn run_periodic_evaluations<F>(
    websocket_url: &str,
    period: Duration,
    mut next_expression: F,
) -> anyhow::Result<()>
where
    F: FnMut() -> anyhow::Result<Option<String>>,
{
    let socket = connect_cdp_websocket(websocket_url).await?;
    let mut session = CdpSession::new(socket);
    let mut interval = tokio::time::interval(period);
    loop {
        interval.tick().await;
        let Some(expression) = next_expression()? else {
            return Ok(());
        };
        let response = session
            .send_command(
                next_message_id(),
                "Runtime.evaluate",
                runtime_evaluate_params(&expression),
            )
            .await?;
        let response = ensure_runtime_evaluate_succeeded(response)?;
        if runtime_evaluate_result_is_false(&response) {
            bail!("periodic Runtime.evaluate reported unavailable capability");
        }
    }
}

pub async fn add_script_to_new_documents(
    websocket_url: &str,
    script: &str,
) -> anyhow::Result<Value> {
    let socket = connect_cdp_websocket(websocket_url).await?;
    let mut session = CdpSession::new(socket);
    session
        .send_command(
            1,
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": script }),
        )
        .await
}

pub async fn install_bridge(
    websocket_url: &str,
    binding_name: &str,
    handler: BridgeHandler,
    new_document_scripts: &[String],
) -> anyhow::Result<()> {
    let socket = connect_cdp_websocket(websocket_url).await?;
    let generation = next_bridge_generation(websocket_url);
    let capture_websocket_url = websocket_url.to_string();
    let capture_generation = generation.clone();
    let downstream_handler = handler.clone();
    let handler: BridgeHandler = Arc::new(move |path, payload| {
        if path == APP_SERVER_CLIENT_CAPTURE_REQUEST_PATH {
            start_app_server_client_capture(&capture_websocket_url, capture_generation.clone());
            return Box::pin(async { Ok(json!({ "status": "started" })) });
        }
        downstream_handler(path, payload)
    });
    let mut session = CdpSession::new(socket).with_handler(handler);
    session = session.with_generation(generation.clone());

    session.send_command(1, "Runtime.enable", json!({})).await?;
    session
        .send_command(2, "Runtime.removeBinding", json!({ "name": binding_name }))
        .await?;
    session
        .send_command(3, "Runtime.addBinding", json!({ "name": binding_name }))
        .await?;

    let bridge_script = build_bridge_script(binding_name);
    session
        .send_command(
            4,
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": bridge_script }),
        )
        .await?;
    session
        .send_command(
            5,
            "Runtime.evaluate",
            runtime_evaluate_params(&bridge_script),
        )
        .await?;

    for script in new_document_scripts {
        let message_id = next_message_id();
        session
            .send_command(
                message_id,
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": script }),
            )
            .await?;
        let message_id = next_message_id();
        session
            .send_command(
                message_id,
                "Runtime.evaluate",
                runtime_evaluate_params(script),
            )
            .await?;
    }

    if !publish_bridge_generation(&generation) {
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "bridge.generation_superseded_before_publish",
            json!({ "generation": generation.id }),
        );
        session.close().await;
        return Ok(());
    }
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "bridge.generation_published",
        json!({ "generation": generation.id }),
    );

    start_app_server_client_capture(websocket_url, generation.clone());

    let mut pending_calls = FuturesUnordered::new();
    session.enqueue_binding_calls(&mut pending_calls);
    tokio::spawn(async move {
        loop {
            if !bridge_generation_is_current(&generation) {
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "bridge.generation_superseded",
                    json!({ "generation": generation.id }),
                );
                break;
            }

            session.enqueue_binding_calls(&mut pending_calls);
            tokio::select! {
                completed = pending_calls.next(), if !pending_calls.is_empty() => {
                    let Some(completed) = completed else {
                        continue;
                    };
                    if session.finish_binding_call(completed).await.is_err() {
                        break;
                    }
                }
                message = session.next_message() => {
                    match message {
                        Ok(Some(_)) => {}
                        Ok(None) | Err(_) => break,
                    }
                }
            }
        }
        session.close().await;
        release_bridge_generation(&generation);
    });

    Ok(())
}

pub fn runtime_evaluate_params(script: &str) -> Value {
    runtime_evaluate_params_with_await_promise(script, false)
}

pub fn runtime_evaluate_params_with_await_promise(script: &str, await_promise: bool) -> Value {
    json!({
        "expression": script,
        "awaitPromise": await_promise,
        "allowUnsafeEvalBlockedByCSP": true,
    })
}

#[cfg(test)]
mod app_server_capture_tests {
    use super::*;
    use std::task::{Context, Poll};

    struct CaptureTestSocket {
        incoming: VecDeque<Message>,
        sent: Vec<Value>,
        mixed_breakpoints: bool,
        fail_evaluation: bool,
    }

    impl CaptureTestSocket {
        fn new(mixed_breakpoints: bool, fail_evaluation: bool) -> Self {
            Self {
                incoming: VecDeque::new(),
                sent: Vec::new(),
                mixed_breakpoints,
                fail_evaluation,
            }
        }

        fn queue(&mut self, value: Value) {
            self.incoming
                .push_back(Message::Text(value.to_string().into()));
        }
    }

    impl futures_util::Stream for CaptureTestSocket {
        type Item = Result<Message, tokio_tungstenite::tungstenite::Error>;

        fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(self.incoming.pop_front().map(Ok))
        }
    }

    impl futures_util::Sink<Message> for CaptureTestSocket {
        type Error = tokio_tungstenite::tungstenite::Error;

        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            let Message::Text(text) = item else {
                return Ok(());
            };
            let command: Value = serde_json::from_str(text.as_str()).unwrap();
            self.sent.push(command.clone());
            let id = command["id"].as_u64().unwrap();
            match command["method"].as_str().unwrap() {
                "Debugger.enable" => self.queue(json!({ "id": id, "result": {} })),
                "Debugger.setBreakpointByUrl" => {
                    let hits = if self.mixed_breakpoints {
                        json!(["capture-1", "user-1"])
                    } else {
                        json!(["capture-1"])
                    };
                    self.queue(json!({
                        "method": "Debugger.paused",
                        "params": {
                            "hitBreakpoints": hits,
                            "callFrames": [{ "callFrameId": "frame-1" }]
                        }
                    }));
                    self.queue(json!({ "id": id, "result": { "breakpointId": "capture-1" } }));
                }
                "Debugger.evaluateOnCallFrame" => {
                    if self.fail_evaluation {
                        self.queue(json!({ "id": id, "result": { "exceptionDetails": {} } }));
                    } else {
                        self.queue(json!({ "id": id, "result": { "result": { "value": true } } }));
                    }
                }
                "Debugger.resume" => {
                    self.queue(json!({ "method": "Debugger.resumed", "params": {} }));
                    self.queue(json!({ "id": id, "result": {} }));
                }
                "Debugger.removeBreakpoint" | "Debugger.disable" => {
                    self.queue(json!({ "id": id, "result": {} }));
                }
                _ => self.queue(json!({ "id": id, "result": {} })),
            }
            Ok(())
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    #[test]
    fn parses_app_server_capture_location_from_renderer_json() {
        let location = json!({
            "urlRegex": "app://-/assets/app-shared-abc.js",
            "lineNumber": 12,
            "columnNumber": 34,
        });
        let encoded = Value::String(location.to_string());

        assert_eq!(
            parse_app_server_client_capture_location(&encoded),
            Some(("app://-/assets/app-shared-abc.js".to_string(), 12, 34))
        );
    }

    #[test]
    fn rejects_invalid_app_server_capture_locations() {
        for location in [
            Value::Null,
            Value::String("not json".to_string()),
            Value::String(
                json!({ "urlRegex": "", "lineNumber": 1, "columnNumber": 2 }).to_string(),
            ),
            Value::String(
                json!({ "urlRegex": "bundle.js", "lineNumber": -1, "columnNumber": 2 }).to_string(),
            ),
            Value::String(
                json!({ "urlRegex": "bundle.js", "lineNumber": u64::MAX, "columnNumber": 2 })
                    .to_string(),
            ),
        ] {
            assert_eq!(parse_app_server_client_capture_location(&location), None);
        }
    }

    #[test]
    fn capture_generation_is_claimed_only_once() {
        let mut active = std::collections::HashSet::new();

        assert!(claim_capture_generation(&mut active, 42));
        assert!(!claim_capture_generation(&mut active, 42));
    }

    #[test]
    fn capture_pause_is_owned_only_when_our_breakpoint_is_the_only_hit() {
        let owned = json!({
            "method": "Debugger.paused",
            "params": { "hitBreakpoints": ["capture-1"] }
        });
        let mixed = json!({
            "method": "Debugger.paused",
            "params": { "hitBreakpoints": ["capture-1", "user-1"] }
        });

        assert!(capture_owns_pause(&owned, Some("capture-1")));
        assert!(!capture_owns_pause(&mixed, Some("capture-1")));
        assert!(!capture_owns_pause(&owned, Some("other")));
        assert!(!capture_owns_pause(
            &json!({ "method": "Debugger.resumed" }),
            Some("capture-1")
        ));
    }

    async fn capture_with_test_socket(
        mixed_breakpoints: bool,
        fail_evaluation: bool,
    ) -> (anyhow::Result<bool>, Vec<Value>) {
        let test_id = NEXT_BRIDGE_GENERATION.fetch_add(1, Ordering::SeqCst);
        let generation = next_bridge_generation(&format!("capture-test-{test_id}"));
        assert!(publish_bridge_generation(&generation));
        let socket = CaptureTestSocket::new(mixed_breakpoints, fail_evaluation);
        let mut session = CdpSession::new(socket).with_generation(generation.clone());
        let result = capture_app_server_client_at_location(
            &mut session,
            &generation,
            ("app://-/assets/app-shared-test.js".to_string(), 3, 12),
            Duration::from_millis(50),
            Duration::from_millis(100),
            Duration::from_millis(100),
        )
        .await;
        let sent = std::mem::take(&mut session.socket.sent);
        release_bridge_generation(&generation);
        (result, sent)
    }

    #[tokio::test]
    async fn capture_resumes_only_its_pause_and_always_cleans_up() {
        let (result, sent) = capture_with_test_socket(false, false).await;
        assert_eq!(result.unwrap(), true);
        let methods = sent
            .iter()
            .filter_map(|command| command["method"].as_str())
            .collect::<Vec<_>>();
        assert!(methods.contains(&"Debugger.resume"));
        assert!(methods.contains(&"Debugger.removeBreakpoint"));
        assert!(methods.contains(&"Debugger.disable"));
    }

    #[tokio::test]
    async fn capture_resumes_its_pause_even_when_call_frame_evaluation_fails() {
        let (result, sent) = capture_with_test_socket(false, true).await;
        assert!(result.is_err());
        let methods = sent
            .iter()
            .filter_map(|command| command["method"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            methods
                .iter()
                .filter(|method| **method == "Debugger.resume")
                .count(),
            1
        );
        assert!(methods.contains(&"Debugger.removeBreakpoint"));
        assert!(methods.contains(&"Debugger.disable"));
    }

    #[tokio::test]
    async fn capture_does_not_resume_a_pause_shared_with_another_breakpoint() {
        let (result, sent) = capture_with_test_socket(true, false).await;
        assert!(!result.unwrap());
        let methods = sent
            .iter()
            .filter_map(|command| command["method"].as_str())
            .collect::<Vec<_>>();
        assert!(!methods.contains(&"Debugger.resume"));
        assert!(methods.contains(&"Debugger.removeBreakpoint"));
        assert!(methods.contains(&"Debugger.disable"));
    }
}

pub fn resolve_bridge_expression(request_id: &str, result: &Value) -> anyhow::Result<String> {
    Ok(format!(
        "window.__codexSessionDeleteResolve({}, {})",
        serde_json::to_string(request_id)?,
        serde_json::to_string(result)?,
    ))
}

pub fn reject_bridge_expression(request_id: &str, message: &str) -> anyhow::Result<String> {
    Ok(format!(
        "window.__codexSessionDeleteReject({}, {})",
        serde_json::to_string(request_id)?,
        serde_json::to_string(message)?,
    ))
}

async fn connect_cdp_websocket(
    websocket_url: &str,
) -> anyhow::Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    let parsed = reqwest::Url::parse(websocket_url).context("invalid CDP WebSocket URL")?;
    let port = parsed
        .port()
        .ok_or_else(|| anyhow::anyhow!("CDP WebSocket URL must include an explicit port"))?;
    crate::cdp::validate_cdp_websocket_url(websocket_url, port)?;
    let (socket, _) = tokio::time::timeout(CDP_CONNECT_TIMEOUT, connect_async(websocket_url))
        .await
        .with_context(|| {
            format!(
                "timed out connecting CDP websocket after {}s",
                CDP_CONNECT_TIMEOUT.as_secs()
            )
        })?
        .context("failed to connect CDP websocket")?;

    Ok(socket)
}

pub fn app_server_client_capture_condition() -> &'static str {
    "!window.__codexPlusAppServerClientClass"
}

pub fn app_server_client_capture_probe_script() -> &'static str {
    "JSON.stringify(window.__codexPlusAppServerClientCapture || null)"
}

pub fn parse_app_server_client_capture_location(value: &Value) -> Option<(String, u32, u32)> {
    let text = value.as_str()?;
    let parsed: Value = serde_json::from_str(text).ok()?;
    let url_regex = parsed.get("urlRegex")?.as_str()?.to_string();
    if url_regex.is_empty() {
        return None;
    }
    let line_number = parsed.get("lineNumber")?.as_u64()?;
    let column_number = parsed.get("columnNumber")?.as_u64()?;
    if line_number > u32::MAX as u64 || column_number > u32::MAX as u64 {
        return None;
    }
    Some((url_regex, line_number as u32, column_number as u32))
}

const APP_SERVER_CLIENT_CAPTURE_LOCATION_TIMEOUT: Duration = Duration::from_secs(90);
const APP_SERVER_CLIENT_CAPTURE_LOCATION_POLL: Duration = Duration::from_secs(2);
const APP_SERVER_CLIENT_CAPTURE_WATCH_CAP: Duration = Duration::from_secs(8 * 60 * 60);
const APP_SERVER_CLIENT_CAPTURE_ENABLE_TIMEOUT: Duration = Duration::from_secs(15);
const APP_SERVER_CLIENT_CAPTURE_ENABLE_ATTEMPTS: usize = 3;
const APP_SERVER_CLIENT_CAPTURE_ENABLE_RETRY_DELAY: Duration = Duration::from_secs(2);
const APP_SERVER_CLIENT_CAPTURE_ON_CALL_FRAME: &str = "(()=>{try{const ctor=this.constructor;if(typeof ctor!=='function'||ctor===Object||ctor===Function)return false;if(typeof ctor.prototype?.sendRequest!=='function'&&typeof this.sendRequest!=='function')return false;window.__codexPlusAppServerClientClass=ctor;window.__codexPlusInstallAppServerClientPrototypePatch?.();return window.__codexPlusAppServerClientClass===ctor}catch{return false}})()";
const APP_SERVER_CLIENT_CAPTURE_COMMAND_TIMEOUT: Duration = Duration::from_secs(1);
const APP_SERVER_CLIENT_CAPTURE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

async fn wait_for_stale_bridge_generation(generation: &BridgeGeneration) {
    while bridge_generation_is_current(generation) {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn enable_app_server_client_capture_debugger<S>(
    session: &mut CdpSession<S>,
    generation: &BridgeGeneration,
    timeout: Duration,
    retry_delay: Duration,
) -> anyhow::Result<bool>
where
    S: SinkExt<Message>
        + StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin
        + Send,
    <S as futures_util::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    for attempt in 1..=APP_SERVER_CLIENT_CAPTURE_ENABLE_ATTEMPTS {
        if !bridge_generation_is_current(generation) {
            return Ok(false);
        }
        let result = tokio::select! {
            result = session.send_command_with_timeout(
                next_message_id(), "Debugger.enable", json!({}), timeout
            ) => result,
            _ = wait_for_stale_bridge_generation(generation) => return Ok(false),
        };
        match result {
            Ok(_) => return Ok(bridge_generation_is_current(generation)),
            Err(error) => {
                if attempt == APP_SERVER_CLIENT_CAPTURE_ENABLE_ATTEMPTS
                    || !error.is::<tokio::time::error::Elapsed>()
                {
                    return Err(error);
                }
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "bridge.app_server_client_capture_enable_retry",
                    json!({ "attempt": attempt, "timeout_ms": timeout.as_millis() }),
                );
                tokio::select! {
                    _ = tokio::time::sleep(retry_delay) => {},
                    _ = wait_for_stale_bridge_generation(generation) => return Ok(false),
                }
            }
        }
    }
    Ok(false)
}

fn spawn_app_server_client_capture(websocket_url: &str, generation: BridgeGeneration) {
    let websocket_url = websocket_url.to_string();
    let generation_id = generation.id;
    tokio::spawn(async move {
        if let Err(error) = run_app_server_client_capture(&websocket_url, generation).await {
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "bridge.app_server_client_capture_failed",
                json!({ "message": format!("{error:#}") }),
            );
        }
        APP_SERVER_CLIENT_CAPTURE_GENERATIONS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&generation_id);
    });
}

fn start_app_server_client_capture(websocket_url: &str, generation: BridgeGeneration) {
    let should_start = {
        let mut active = APP_SERVER_CLIENT_CAPTURE_GENERATIONS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        claim_capture_generation(&mut active, generation.id)
    };
    if !should_start {
        return;
    }
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "bridge.app_server_client_capture_started",
        json!({ "generation": generation.id, "mode": "proactive" }),
    );
    spawn_app_server_client_capture(websocket_url, generation);
}

async fn run_app_server_client_capture(
    websocket_url: &str,
    generation: BridgeGeneration,
) -> anyhow::Result<()> {
    let settings = crate::settings::SettingsStore::new(crate::paths::default_settings_path())
        .load()
        .unwrap_or_default();
    if !settings.codex_app_model_whitelist_unlock && !settings.codex_app_session_delete {
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "bridge.app_server_client_capture_skipped",
            json!({ "reason": "capture_features_disabled" }),
        );
        return Ok(());
    }
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "bridge.app_server_client_capture_debugger_connecting",
        json!({ "generation": generation.id }),
    );
    let socket = match connect_cdp_websocket(websocket_url).await {
        Ok(socket) => socket,
        Err(_) => {
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "bridge.app_server_client_capture_debugger_connect_failed",
                json!({ "generation": generation.id }),
            );
            anyhow::bail!("failed to connect capture debugger")
        }
    };
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "bridge.app_server_client_capture_debugger_connected",
        json!({ "generation": generation.id }),
    );
    let mut session = CdpSession::new(socket);
    let mut location_poll_count = 0usize;
    let location_wait_started = tokio::time::Instant::now();
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "bridge.app_server_client_capture_location_wait_started",
        json!({
            "generation": generation.id,
            "timeout_ms": APP_SERVER_CLIENT_CAPTURE_LOCATION_TIMEOUT.as_millis(),
        }),
    );
    let location_wait = async {
        loop {
            if !bridge_generation_is_current(&generation) {
                return None;
            }
            location_poll_count += 1;
            let Ok(response) = session
                .send_command(
                    next_message_id(),
                    "Runtime.evaluate",
                    runtime_evaluate_params(app_server_client_capture_probe_script()),
                )
                .await
            else {
                tokio::time::sleep(APP_SERVER_CLIENT_CAPTURE_LOCATION_POLL).await;
                continue;
            };
            if let Some(value) = response.pointer("/result/result/value") {
                if let Some(location) = parse_app_server_client_capture_location(value) {
                    return Some(location);
                }
            }
            tokio::time::sleep(APP_SERVER_CLIENT_CAPTURE_LOCATION_POLL).await;
        }
    };
    let (location, location_wait_reason) = tokio::select! {
        result = tokio::time::timeout(APP_SERVER_CLIENT_CAPTURE_LOCATION_TIMEOUT, location_wait) => match result {
            Ok(Some(location)) => (Some(location), "located"),
            Ok(None) => (None, "generation_replaced"),
            Err(_) => (None, "timeout"),
        },
        _ = wait_for_stale_bridge_generation(&generation) => (None, "generation_replaced"),
    };
    let location_wait_elapsed_ms = location_wait_started.elapsed().as_millis();
    let Some(location) = location else {
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "bridge.app_server_client_capture_skipped",
            json!({ "reason": location_wait_reason }),
        );
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "bridge.app_server_client_capture_location_wait_failed",
            json!({
                "reason": location_wait_reason,
                "generation": generation.id,
                "pollCount": location_poll_count,
                "elapsedMs": location_wait_elapsed_ms,
            }),
        );
        let _ =
            tokio::time::timeout(APP_SERVER_CLIENT_CAPTURE_CLEANUP_TIMEOUT, session.close()).await;
        return Ok(());
    };
    let _ = crate::diagnostic_log::append_diagnostic_log(
        "bridge.app_server_client_capture_location_found",
        json!({
            "generation": generation.id,
            "lineNumber": location.1,
            "columnNumber": location.2,
            "pollCount": location_poll_count,
            "elapsedMs": location_wait_elapsed_ms,
        }),
    );
    capture_app_server_client_at_location(
        &mut session,
        &generation,
        location,
        APP_SERVER_CLIENT_CAPTURE_COMMAND_TIMEOUT,
        APP_SERVER_CLIENT_CAPTURE_WATCH_CAP,
        APP_SERVER_CLIENT_CAPTURE_CLEANUP_TIMEOUT,
    )
    .await
    .map(|_| ())
}

#[derive(Default)]
struct CaptureDebuggerState {
    breakpoint_id: Option<String>,
}

fn capture_owns_pause(message: &Value, breakpoint_id: Option<&str>) -> bool {
    let Some(breakpoint_id) = breakpoint_id else {
        return false;
    };
    if message.get("method").and_then(Value::as_str) != Some("Debugger.paused") {
        return false;
    }
    let Some(hits) = message
        .pointer("/params/hitBreakpoints")
        .and_then(Value::as_array)
    else {
        return false;
    };
    !hits.is_empty() && hits.iter().all(|hit| hit.as_str() == Some(breakpoint_id))
}

fn breakpoint_fingerprint(id: &str) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(id.as_bytes()).into()
}

fn paused_hit_fingerprint(message: &Value) -> Option<[u8; 32]> {
    let hits = message.pointer("/params/hitBreakpoints")?.as_array()?;
    let first = hits.first()?.as_str()?;
    hits.iter()
        .all(|hit| hit.as_str() == Some(first))
        .then(|| breakpoint_fingerprint(first))
}

async fn capture_app_server_client_at_location<S>(
    session: &mut CdpSession<S>,
    generation: &BridgeGeneration,
    location: (String, u32, u32),
    command_timeout: Duration,
    watch_cap: Duration,
    cleanup_timeout: Duration,
) -> anyhow::Result<bool>
where
    S: SinkExt<Message>
        + StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin
        + Send,
    <S as futures_util::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    let mut state = CaptureDebuggerState::default();
    let result: anyhow::Result<bool> = async {
        if !enable_app_server_client_capture_debugger(
            session, generation, APP_SERVER_CLIENT_CAPTURE_ENABLE_TIMEOUT,
            APP_SERVER_CLIENT_CAPTURE_ENABLE_RETRY_DELAY,
        ).await? {
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "bridge.app_server_client_capture_debugger_enable_skipped",
                json!({ "reason": "generation_replaced" }),
            );
            return Ok(false);
        }
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "bridge.app_server_client_capture_debugger_enabled",
            json!({}),
        );
        let (url_regex, line_number, column_number) = location;
        let breakpoint = match session.send_command_with_timeout(
            next_message_id(), "Debugger.setBreakpointByUrl",
            json!({ "lineNumber": line_number, "columnNumber": column_number, "urlRegex": url_regex,
                "condition": app_server_client_capture_condition() }), command_timeout,
        ).await {
            Ok(breakpoint) => breakpoint,
            Err(_) => {
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "bridge.app_server_client_capture_breakpoint_set_failed",
                    json!({ "lineNumber": line_number, "columnNumber": column_number }),
                );
                anyhow::bail!("failed to set app-server capture breakpoint")
            }
        };
        state.breakpoint_id = breakpoint.pointer("/result/breakpointId")
            .and_then(Value::as_str).map(str::to_string);
        if state.breakpoint_id.is_none() {
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "bridge.app_server_client_capture_breakpoint_set_failed",
                json!({ "reason": "breakpoint_id_missing", "lineNumber": line_number, "columnNumber": column_number }),
            );
            anyhow::bail!("capture breakpoint response did not contain an id")
        }
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "bridge.app_server_client_capture_armed",
            json!({
                "lineNumber": line_number,
                "columnNumber": column_number,
            }),
        );
        let watch_deadline = tokio::time::Instant::now() + watch_cap;
        let mut captured_once = false;
        loop {
            if !bridge_generation_is_current(generation) { break; }
            let message = tokio::select! {
                message = tokio::time::timeout_at(watch_deadline, session.next_message()) => message,
                _ = wait_for_stale_bridge_generation(generation) => break,
            };
            let Ok(message) = message else { break; };
            let Some(message) = message? else { break; };
            if !capture_owns_pause(&message, state.breakpoint_id.as_deref())
                || !session.current_pause_is_owned(state.breakpoint_id.as_deref()) { continue; }
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "bridge.app_server_client_capture_breakpoint_hit",
                json!({
                    "reason": message.pointer("/params/reason").and_then(Value::as_str).unwrap_or("unknown"),
                    "callFrameCount": message.pointer("/params/callFrames").and_then(Value::as_array).map_or(0, Vec::len),
                }),
            );
            let evaluation: anyhow::Result<()> = async {
                let frame = message.pointer("/params/callFrames/0/callFrameId")
                    .and_then(Value::as_str).context("owned capture pause has no call frame")?;
                let reply = session.send_command_with_timeout(
                    next_message_id(), "Debugger.evaluateOnCallFrame",
                    json!({ "callFrameId": frame, "expression": APP_SERVER_CLIENT_CAPTURE_ON_CALL_FRAME,
                        "returnByValue": true }), command_timeout,
                ).await?;
                anyhow::ensure!(reply.pointer("/result/exceptionDetails").is_none(), "capture frame evaluation raised an exception");
                anyhow::ensure!(reply.pointer("/result/result/value").and_then(Value::as_bool) == Some(true),
                    "capture frame evaluation did not publish the app-server client class");
                Ok(())
            }.await;
            anyhow::ensure!(session.current_pause_is_owned(state.breakpoint_id.as_deref()),
                "owned capture pause was resumed or replaced by another debugger");
            let resume_epoch = session.pause_epoch;
            session.send_command_with_timeout(
                next_message_id(), "Debugger.resume", json!({}), command_timeout,
            ).await.context("failed to resume owned app-server capture pause")?;
            session.clear_pause_if_epoch(resume_epoch);
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "bridge.app_server_client_capture_breakpoint_resumed",
                json!({ "captured": evaluation.is_ok() }),
            );
            evaluation?;
            if !captured_once {
                captured_once = true;
                let _ = crate::diagnostic_log::append_diagnostic_log("bridge.app_server_client_captured", json!({}));
            }
        }
        Ok(captured_once)
    }.await;
    let cleanup = tokio::time::timeout(
        cleanup_timeout,
        cleanup_capture_debugger(
            session,
            &mut state,
            command_timeout.min(cleanup_timeout / 4),
        ),
    )
    .await
    .context("capture debugger cleanup timed out")
    .and_then(|result| result);
    let closed = tokio::time::timeout(cleanup_timeout, session.socket.close())
        .await
        .context("capture debugger socket close timed out")
        .and_then(|result| result.context("failed to close capture debugger socket"));
    let cleanup = match (cleanup, closed) {
        (Err(cleanup), Err(closed)) => {
            Err(cleanup.context(format!("socket close also failed: {closed:#}")))
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    };
    match (result, cleanup) {
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("capture cleanup also failed: {cleanup:#}")))
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(captured), Ok(())) => Ok(captured),
    }
}

async fn cleanup_capture_debugger<S>(
    session: &mut CdpSession<S>,
    state: &mut CaptureDebuggerState,
    timeout: Duration,
) -> anyhow::Result<()>
where
    S: SinkExt<Message>
        + StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin
        + Send,
    <S as futures_util::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    let mut errors = Vec::new();
    if let Some(id) = state.breakpoint_id.as_deref() {
        if let Err(error) = session
            .send_command_with_timeout(
                next_message_id(),
                "Debugger.removeBreakpoint",
                json!({ "breakpointId": id }),
                timeout,
            )
            .await
        {
            errors.push(format!("{error:#}"));
        }
    }
    if session.current_pause_is_owned(state.breakpoint_id.as_deref()) {
        let resume_epoch = session.pause_epoch;
        if let Err(error) = session
            .send_command_with_timeout(next_message_id(), "Debugger.resume", json!({}), timeout)
            .await
        {
            errors.push(format!("{error:#}"));
        } else {
            session.clear_pause_if_epoch(resume_epoch);
        }
    }
    if let Err(error) = session
        .send_command_with_timeout(next_message_id(), "Debugger.disable", json!({}), timeout)
        .await
    {
        errors.push(format!("{error:#}"));
    }
    anyhow::ensure!(
        errors.is_empty(),
        "capture debugger cleanup failed: {}",
        errors.join("; ")
    );
    Ok(())
}

struct CdpSession<S> {
    socket: S,
    responses: HashMap<u64, Value>,
    binding_calls: VecDeque<Value>,
    paused_events: VecDeque<(Value, usize, u64)>,
    paused_event_bytes: usize,
    pause_epoch: u64,
    current_pause_hit: Option<[u8; 32]>,
    handler: Option<BridgeHandler>,
    generation: Option<BridgeGeneration>,
}

impl<S> CdpSession<S>
where
    S: SinkExt<Message>
        + StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin
        + Send,
    <S as futures_util::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    fn new(socket: S) -> Self {
        Self {
            socket,
            responses: HashMap::new(),
            binding_calls: VecDeque::new(),
            paused_events: VecDeque::new(),
            paused_event_bytes: 0,
            pause_epoch: 0,
            current_pause_hit: None,
            handler: None,
            generation: None,
        }
    }

    fn with_handler(mut self, handler: BridgeHandler) -> Self {
        self.handler = Some(handler);
        self
    }

    fn with_generation(mut self, generation: BridgeGeneration) -> Self {
        self.generation = Some(generation);
        self
    }

    fn is_current(&self) -> bool {
        self.generation
            .as_ref()
            .is_none_or(bridge_generation_is_current)
    }

    fn current_pause_is_owned(&self, breakpoint_id: Option<&str>) -> bool {
        breakpoint_id.is_some_and(|id| self.current_pause_hit == Some(breakpoint_fingerprint(id)))
    }

    fn clear_pause_if_epoch(&mut self, epoch: u64) {
        if self.pause_epoch == epoch {
            self.pause_epoch = self.pause_epoch.wrapping_add(1);
            self.current_pause_hit = None;
            self.paused_events.clear();
            self.paused_event_bytes = 0;
        }
    }

    async fn close(&mut self) {
        let _ = self.socket.send(Message::Close(None)).await;
        let _ = self.socket.close().await;
    }

    async fn send_command(
        &mut self,
        message_id: u64,
        method: &str,
        params: Value,
    ) -> anyhow::Result<Value> {
        self.send_command_with_timeout(message_id, method, params, CDP_COMMAND_TIMEOUT)
            .await
    }

    async fn send_command_with_timeout(
        &mut self,
        message_id: u64,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        tokio::time::timeout(timeout, async {
            self.socket
                .send(Message::Text(
                    json!({ "id": message_id, "method": method, "params": params })
                        .to_string()
                        .into(),
                ))
                .await
                .with_context(|| format!("failed to send CDP command {method} id {message_id}"))?;
            self.wait_for_id(message_id, method.to_string()).await
        })
        .await
        .with_context(|| {
            format!(
                "timed out waiting for CDP command {method} id {message_id} response after {}s",
                timeout.as_secs()
            )
        })?
    }

    async fn send_command_without_wait(
        &mut self,
        message_id: u64,
        method: &str,
        params: Value,
    ) -> anyhow::Result<()> {
        self.socket
            .send(Message::Text(
                json!({
                    "id": message_id,
                    "method": method,
                    "params": params,
                })
                .to_string()
                .into(),
            ))
            .await
            .with_context(|| format!("failed to send CDP command {method} id {message_id}"))?;
        Ok(())
    }

    async fn wait_for_id(&mut self, message_id: u64, method: String) -> anyhow::Result<Value> {
        loop {
            if let Some(response) = self.responses.remove(&message_id) {
                return command_result(response, &method, message_id);
            }

            let Some(message) = self.read_socket_message().await? else {
                bail!("CDP websocket closed before response for {method} id {message_id}");
            };

            if let Some(response_id) = message.get("id").and_then(Value::as_u64) {
                if response_id == message_id {
                    return command_result(message, &method, message_id);
                }
                self.responses.insert(response_id, message);
            } else if message.get("method").and_then(Value::as_str) == Some("Debugger.paused") {
                let bytes = message.to_string().len();
                anyhow::ensure!(
                    self.paused_events.len() < 8 && bytes <= 1024 * 1024 - self.paused_event_bytes,
                    "CDP paused event queue exceeded its bounded capacity"
                );
                self.paused_event_bytes += bytes;
                self.paused_events
                    .push_back((message, bytes, self.pause_epoch));
            }
        }
    }

    async fn next_message(&mut self) -> anyhow::Result<Option<Value>> {
        while let Some((message, bytes, epoch)) = self.paused_events.pop_front() {
            self.paused_event_bytes -= bytes;
            if epoch == self.pause_epoch {
                return Ok(Some(message));
            }
        }
        self.read_socket_message().await
    }

    async fn read_socket_message(&mut self) -> anyhow::Result<Option<Value>> {
        let Some(message) = self.socket.next().await else {
            return Ok(None);
        };
        let message = message.context("failed to read CDP websocket message")?;
        let Message::Text(text) = message else {
            return Ok(Some(json!({})));
        };
        let value: Value = serde_json::from_str(&text).context("failed to parse CDP message")?;

        match value.get("method").and_then(Value::as_str) {
            Some("Debugger.paused") => {
                self.pause_epoch = self.pause_epoch.wrapping_add(1);
                self.current_pause_hit = paused_hit_fingerprint(&value);
            }
            Some("Debugger.resumed") => self.clear_pause_if_epoch(self.pause_epoch),
            _ => {}
        }

        if value.get("method").and_then(Value::as_str) == Some("Runtime.bindingCalled") {
            self.binding_calls.push_back(value.clone());
        }

        Ok(Some(value))
    }

    fn enqueue_binding_calls(&mut self, pending_calls: &mut FuturesUnordered<PendingBridgeCall>) {
        while let Some(message) = self.binding_calls.pop_front() {
            self.enqueue_binding_call(message, pending_calls);
        }
    }

    fn enqueue_binding_call(
        &mut self,
        message: Value,
        pending_calls: &mut FuturesUnordered<PendingBridgeCall>,
    ) {
        let Some(handler) = self.handler.clone() else {
            return;
        };

        let Some(payload_text) = message
            .get("params")
            .and_then(|params| params.get("payload"))
            .and_then(Value::as_str)
        else {
            return;
        };

        let parsed: Value = match serde_json::from_str(payload_text) {
            Ok(parsed) => parsed,
            Err(error) => {
                let Some(request_id) = extract_string_field(payload_text, "id") else {
                    return;
                };
                self.enqueue_completed_binding_call(
                    request_id,
                    Err(format!("failed to parse bridge payload: {error}")),
                    pending_calls,
                );
                return;
            }
        };
        let Some(request_id) = parsed.get("id").and_then(Value::as_str).map(str::to_string) else {
            return;
        };
        if !self.is_current() {
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "bridge.stale_request_dropped",
                json!({
                    "request_id": request_id,
                    "generation": self.generation.as_ref().map(|generation| generation.id)
                }),
            );
            return;
        }
        let path = parsed
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let payload = parsed.get("payload").cloned().unwrap_or_else(|| json!({}));
        let generation = self.generation.clone();

        pending_calls.push(Box::pin(async move {
            CompletedBridgeCall {
                request_id,
                generation,
                response: handler(path, payload)
                    .await
                    .map_err(|error| error.to_string()),
            }
        }));
    }

    fn enqueue_completed_binding_call(
        &self,
        request_id: String,
        response: Result<Value, String>,
        pending_calls: &mut FuturesUnordered<PendingBridgeCall>,
    ) {
        let generation = self.generation.clone();
        pending_calls.push(Box::pin(async move {
            CompletedBridgeCall {
                request_id,
                generation,
                response,
            }
        }));
    }

    async fn finish_binding_call(&mut self, completed: CompletedBridgeCall) -> anyhow::Result<()> {
        if completed
            .generation
            .as_ref()
            .is_some_and(|generation| !bridge_generation_is_current(generation))
        {
            let _ = crate::diagnostic_log::append_diagnostic_log(
                "bridge.stale_response_dropped",
                json!({
                    "request_id": completed.request_id,
                    "generation": completed.generation.as_ref().map(|generation| generation.id)
                }),
            );
            return Ok(());
        }

        match completed.response {
            Ok(result) => {
                self.resolve_bridge_request(&completed.request_id, &result)
                    .await
            }
            Err(message) => {
                self.reject_bridge_request(&completed.request_id, &message)
                    .await
            }
        }
    }

    async fn resolve_bridge_request(
        &mut self,
        request_id: &str,
        result: &Value,
    ) -> anyhow::Result<()> {
        let expression = resolve_bridge_expression(request_id, result)?;
        let message_id = next_message_id();
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "bridge.resolve_start",
            json!({
                "request_id": request_id,
                "message_id": message_id,
                "result_status": result.get("status").and_then(Value::as_str).unwrap_or("")
            }),
        );
        let sent = self
            .send_command_without_wait(
                message_id,
                "Runtime.evaluate",
                runtime_evaluate_params(&expression),
            )
            .await;
        match &sent {
            Ok(_) => {
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "bridge.resolve_ok",
                    json!({
                        "request_id": request_id,
                        "message_id": message_id
                    }),
                );
            }
            Err(error) => {
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "bridge.resolve_failed",
                    json!({
                        "request_id": request_id,
                        "message_id": message_id,
                        "message": error.to_string()
                    }),
                );
            }
        }
        sent.map(|_| ())
    }

    async fn reject_bridge_request(
        &mut self,
        request_id: &str,
        message: &str,
    ) -> anyhow::Result<()> {
        let expression = reject_bridge_expression(request_id, message)?;
        let message_id = next_message_id();
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "bridge.reject_start",
            json!({
                "request_id": request_id,
                "message_id": message_id,
                "message": message
            }),
        );
        let sent = self
            .send_command_without_wait(
                message_id,
                "Runtime.evaluate",
                runtime_evaluate_params(&expression),
            )
            .await;
        match &sent {
            Ok(_) => {
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "bridge.reject_ok",
                    json!({
                        "request_id": request_id,
                        "message_id": message_id
                    }),
                );
            }
            Err(error) => {
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    "bridge.reject_failed",
                    json!({
                        "request_id": request_id,
                        "message_id": message_id,
                        "error": error.to_string()
                    }),
                );
            }
        }
        sent.map(|_| ())
    }
}

fn command_result(response: Value, method: &str, message_id: u64) -> anyhow::Result<Value> {
    if let Some(error) = response.get("error") {
        bail!("CDP command {method} id {message_id} failed: {error}");
    }
    Ok(response)
}

fn ensure_runtime_evaluate_succeeded(response: Value) -> anyhow::Result<Value> {
    if let Some(exception) = response
        .get("result")
        .and_then(|result| result.get("exceptionDetails"))
    {
        bail!("Runtime.evaluate raised an exception: {exception}");
    }
    Ok(response)
}

fn runtime_evaluate_result_is_false(response: &Value) -> bool {
    response
        .get("result")
        .and_then(|result| result.get("result"))
        .and_then(|result| result.get("value"))
        .is_some_and(|value| value == false)
}

fn extract_string_field(input: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let mut index = input.find(&needle)? + needle.len();
    let bytes = input.as_bytes();

    while matches!(bytes.get(index), Some(b' ' | b'\n' | b'\r' | b'\t')) {
        index += 1;
    }
    if bytes.get(index) != Some(&b':') {
        return None;
    }
    index += 1;
    while matches!(bytes.get(index), Some(b' ' | b'\n' | b'\r' | b'\t')) {
        index += 1;
    }
    if bytes.get(index) != Some(&b'"') {
        return None;
    }
    index += 1;

    let mut output = String::new();
    let mut escaped = false;
    for ch in input[index..].chars() {
        if escaped {
            output.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(output),
            _ => output.push(ch),
        }
    }

    None
}

fn next_message_id() -> u64 {
    NEXT_MESSAGE_ID.fetch_add(1, Ordering::Relaxed) + 1
}
