(() => {
  // Codex++ 会把本文件注入 Codex 渲染页；所有读取都只能依赖页面里已经暴露的运行态信号。
  const INSTALL_KEY = "__codexContextMeterInstalled";
  const API_KEY = "__codexContextMeter";
  const STYLE_ID = "codex-context-meter-style";
  const ROOT_ID = "codex-context-meter";
  const CAPTURE_STATE_KEY = "__codexContextMeterCaptureState";
  const SCRIPT_VERSION = 33;
  const UPDATE_INTERVAL_MS = 5000;
  const SLOW_SCAN_INTERVAL_MS = 30000;
  const SWITCH_RETRY_WINDOW_MS = 8000;
  const SWITCH_RETRY_INTERVAL_MS = 700;
  const NAVIGATION_PENDING_MS = 1500;
  const CAPTURE_UPDATE_DELAY_MS = 30;
  const MUTATION_UPDATE_DELAY_MS = 500;
  const ACTIVE_CONVERSATION_LOOKUP_CACHE_MS = 250;
  const APP_SIGNAL_READING_CACHE_MS = 120;
  const APP_SIGNAL_IMPORT_GRACE_MS = 600;
  // 以下限额只约束兜底扫描；主路径读 app signal / 已缓存读数，不受这些值影响。
  const EXPENSIVE_FALLBACK_INTERVAL_MS = 2500;
  const REACT_HOST_SCAN_LIMIT = 180;
  const STATUS_TEXT_NODE_SCAN_LIMIT = 1200;
  const DOM_ATTRIBUTE_SCAN_LIMIT = 500;
  const DOM_TEXT_PART_LIMIT = 120;
  const WINDOW_KEY_CACHE_MS = 10000;
  const MAX_TEXT_LENGTH = 120000;
  const MAX_CAPTURE_TEXT_LENGTH = 2000000;
  const CAPTURE_TEXT_HINT_RE = /context|token|usage|latestTokenUsageInfo|window|budget|remaining|上下文|令牌|使用|窗口/i;
  const THREAD_CONTENT_SELECTOR = [
    '[data-app-shell-main-content-layout*="thread"]',
    '[class*="thread-edge"]',
    '[class*="transcript"]',
    '[data-testid*="thread"], [data-test-id*="thread"]',
    '[data-testid*="conversation"], [data-test-id*="conversation"]',
  ].join(",");
  const REACT_STATE_HOST_SELECTOR = [
    "[data-app-action-sidebar-thread-id]",
    "[data-testid*='thread' i]",
    "[data-testid*='conversation' i]",
    "[data-testid*='message' i]",
    "[data-message-author-role]",
    "main",
    "article",
  ].join(",");
  const CONVERSATION_CONTENT_SELECTOR = [
    `[data-thread-find-target]`,
    `[data-message-author-role]`,
    `[data-testid*="conversation" i]`,
    `[data-testid*="message" i]`,
    `article`,
  ].join(",");
  const MESSAGE_MUTATION_SELECTOR = [
    `[data-thread-find-target]`,
    `[data-message-author-role]`,
    `[data-testid*="message" i]`,
    `article`,
  ].join(",");
  const PREFERRED_STATUS_KEYS = [
    "contextUsage",
    "context_usage",
    "tokenUsage",
    "token_usage",
    "usage",
    "data",
    "props",
    "memoizedState",
    "memoizedProps",
    "pendingProps",
    "updateQueue",
    "dependencies",
    "alternate",
    "return",
    "child",
    "sibling",
    "stateNode",
    "current",
    "value",
    "store",
    "atom",
    "atoms",
    "map",
    "cache",
  ];
  const PREFERRED_STATUS_KEY_SET = new Set(PREFERRED_STATUS_KEYS);
  const STATUS_TREE_KEY_RE = /context|usage|status|thread|conversation|token|query|data|props|memoized|pending|return|child|sibling|state|value|current|store|atom|map|cache/i;
  const APP_SIGNAL_SCOPE_KEY_RE = /memoized|pending|dependencies|firstContext|context|value|current|return|child|sibling|state|store|node|chain|scope|provider|props|query|cache/i;
  const VALUE_TEXT_KEY_RE = /context|token|tokens|usage|window|budget|remaining|上下文|令牌|使用|窗口/i;
  const REACT_PRIVATE_KEY_RE = /^__react(?:Props|Fiber|Container)\$/;

  for (const key of Object.keys(window)) {
    if (!/CodexContextUsageMeter(?:Installed)?$/.test(key)) continue;

    const legacyApi = window[key];
    if (legacyApi && typeof legacyApi.destroy === "function") {
      legacyApi.destroy();
    }
    delete window[key];
  }

  const legacyRootId = ["codex", "context", "usage", "meter"].join("-");
  document.getElementById(legacyRootId)?.remove();
  document.getElementById(`${legacyRootId}-style`)?.remove();

  if (window[INSTALL_KEY]) {
    const api = window[API_KEY];
    if (api && api.version !== SCRIPT_VERSION && typeof api.destroy === "function") {
      api.destroy();
    } else {
      if (api && typeof api.refresh === "function") {
        api.refresh();
      }
      return;
    }
  }

  window[INSTALL_KEY] = true;

  const contextTerms =
    /context|token|tokens|usage|window|budget|remaining|上下文|令牌|使用|窗口/i;
  const ratioWords =
    /context|token|tokens|usage|window|budget|remaining|上下文|令牌|使用|窗口/i;
  const state = {
    activeConversationId: null,
    lastReading: null,
    readingsByConversationId: new Map(),
    lastAnimatedUsedByConversationId: new Map(),
    root: null,
    value: null,
    fill: null,
    lastScanAt: 0,
    lastScannedConversationId: null,
    navigationPendingUntil: 0,
    switchRetryUntil: 0,
    retryTimer: 0,
    timer: 0,
    observer: null,
    navigationListener: null,
    pendingUpdate: 0,
    pendingUpdateDueAt: 0,
    cachedActiveConversationId: null,
    activeConversationIdLookupAt: 0,
    appSignalScope: null,
    appSignalModules: null,
    appSignalModulesPromise: null,
    appSignalLastLookupAt: 0,
    appSignalCachedReading: null,
    appSignalCachedConversationId: null,
    appSignalCachedAt: 0,
    appSignalModulesRequestedAt: 0,
    waitingForAppSignalModules: false,
    expensiveFallbackScannedAt: 0,
    expensiveFallbackConversationId: null,
    windowContextTextKeys: null,
    windowContextTextKeysAt: 0,
    windowUsageKeys: null,
    windowUsageKeysAt: 0,
    reactPrivateKeyCache: new WeakMap(),
    scanGeneration: 0,
    filteredReflectKeyCache: new WeakMap(),
    appSignalSkipGeneration: new WeakMap(),
    threadContentLookupAt: 0,
    threadContentLookupResult: false,
  };

  function getCaptureState() {
    let captureState = window[CAPTURE_STATE_KEY];
    if (!captureState || typeof captureState !== "object") {
      captureState = {};
      window[CAPTURE_STATE_KEY] = captureState;
    }

    captureState.version = SCRIPT_VERSION;
    captureState.inspectText = inspectCandidateText;
    captureState.inspectValue = inspectCandidateValue;
    captureState.acceptReading = acceptReading;
    captureState.parsePayloadText = parsePayloadText;
    return captureState;
  }

  function installStyle() {
    if (document.getElementById(STYLE_ID)) return;

    const style = document.createElement("style");
    style.id = STYLE_ID;
    style.textContent = `
      #${ROOT_ID} {
        position: fixed;
        top: max(10px, env(safe-area-inset-top));
        left: 50%;
        transform: translateX(-50%);
        z-index: 2147483647;
        min-width: 220px;
        max-width: min(420px, calc(100vw - 32px));
        padding: 8px 10px 9px;
        border: 1px solid rgba(255, 255, 255, 0.16);
        border-radius: 8px;
        background: rgba(20, 22, 28, 0.88);
        color: rgba(255, 255, 255, 0.92);
        box-shadow: 0 8px 28px rgba(0, 0, 0, 0.24);
        font: 12px/1.35 system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
        overflow: visible;
        pointer-events: none;
        user-select: none;
        backdrop-filter: blur(10px);
      }

      #${ROOT_ID} .ccm-row {
        display: flex;
        align-items: center;
        justify-content: center;
        margin-bottom: 6px;
        white-space: nowrap;
      }

      #${ROOT_ID} .ccm-value {
        color: rgba(255, 255, 255, 0.98);
        font-weight: 650;
        font-variant-numeric: tabular-nums;
        overflow: hidden;
        text-align: center;
        text-overflow: ellipsis;
      }

      #${ROOT_ID} .ccm-track {
        width: 100%;
        height: 7px;
        overflow: hidden;
        border-radius: 999px;
        background: rgba(255, 255, 255, 0.16);
      }

      #${ROOT_ID} .ccm-fill {
        width: 0%;
        height: 100%;
        border-radius: inherit;
        background: linear-gradient(90deg, #4ea1ff, #4ade80);
        transition: width 180ms ease, background 180ms ease;
      }

      #${ROOT_ID}[data-level="warn"] .ccm-fill {
        background: linear-gradient(90deg, #f59e0b, #f97316);
      }

      #${ROOT_ID}[data-level="danger"] .ccm-fill {
        background: linear-gradient(90deg, #fb7185, #ef4444);
      }

      #${ROOT_ID}[hidden] {
        display: none !important;
      }

      #${ROOT_ID} .ccm-hit-pop {
        position: absolute;
        right: calc(100% + 10px);
        top: 50%;
        z-index: 1;
        color: #fff7ed;
        background: linear-gradient(92deg, #fff7ed 0%, #fecdd3 38%, #fb7185 68%, #f97316 100%);
        -webkit-background-clip: text;
        background-clip: text;
        -webkit-text-fill-color: transparent;
        font-size: 14px;
        font-weight: 850;
        line-height: 1;
        opacity: 0;
        filter: drop-shadow(0 1px 0 rgba(0, 0, 0, 0.78))
          drop-shadow(0 3px 8px rgba(0, 0, 0, 0.58))
          drop-shadow(0 0 14px rgba(251, 113, 133, 0.56))
          drop-shadow(0 0 26px rgba(249, 115, 22, 0.24));
        text-shadow: 0 0 1px rgba(255, 255, 255, 0.45);
        transform: translate(44px, -50%) scale(0.72);
        transform-origin: right center;
        animation: ccm-hit-pop 3000ms cubic-bezier(0.16, 0.84, 0.24, 1) forwards;
        pointer-events: none;
        white-space: nowrap;
        will-change: opacity, transform, filter;
      }

      @keyframes ccm-hit-pop {
        0% {
          opacity: 0;
          transform: translate(44px, -50%) scale(0.72);
        }
        12% {
          opacity: 1;
          transform: translate(8px, -50%) scale(1);
        }
        72% {
          opacity: 1;
          transform: translate(-128px, -50%) scale(1.82);
        }
        100% {
          opacity: 0;
          transform: translate(-176px, -50%) scale(2.08);
        }
      }
    `;
    document.head.appendChild(style);
  }

  function ensureRoot() {
    let root = document.getElementById(ROOT_ID);
    if (root) {
      state.root = root;
      state.value = root.querySelector(".ccm-value");
      state.fill = root.querySelector(".ccm-fill");
      return root;
    }

    root = document.createElement("div");
    root.id = ROOT_ID;
    root.dataset.known = "false";
    root.dataset.level = "normal";
    root.innerHTML = `
      <div class="ccm-row">
        <span class="ccm-value">Context Left --</span>
      </div>
      <div class="ccm-track">
        <div class="ccm-fill"></div>
      </div>
    `;
    document.body.appendChild(root);
    state.root = root;
    state.value = root.querySelector(".ccm-value");
    state.fill = root.querySelector(".ccm-fill");
    return root;
  }

  function toNumber(value, unit) {
    if (value == null) return null;

    const parsed = Number(String(value).replace(/,/g, ""));
    if (!Number.isFinite(parsed)) return null;

    const normalizedUnit = String(unit || "").toLowerCase();
    if (normalizedUnit === "k") return parsed * 1000;
    if (normalizedUnit === "m") return parsed * 1000000;
    return parsed;
  }

  function clampPercent(value) {
    if (!Number.isFinite(value)) return null;
    return Math.max(0, Math.min(100, value));
  }

  function compactNumber(value) {
    if (!Number.isFinite(value)) return "";
    if (value >= 1000000) return `${(value / 1000000).toFixed(1)}M`;
    if (value >= 1000) return `${(value / 1000).toFixed(1)}K`;
    return String(Math.round(value));
  }

  function showTokenSpendEffect(root, deltaTokens) {
    if (!root || !Number.isFinite(deltaTokens) || deltaTokens <= 0) return;

    const pop = document.createElement("div");
    pop.className = "ccm-hit-pop";
    pop.textContent = `-${Math.round(deltaTokens).toLocaleString("en-US")} Tokens`;
    root.appendChild(pop);

    window.setTimeout(() => {
      pop.remove();
    }, 3100);
  }

  function hasDescendant(element, selector) {
    return !!(element && element.querySelector(selector));
  }

  // 只把主会话内容区当作可显示区域；Codex App DOM 更新后，优先从这里重找 thread/transcript 锚点。
  function hasThreadContentSurface() {
    if (!isConversationWindow()) return false;

    const now = Date.now();
    if (now - state.threadContentLookupAt < ACTIVE_CONVERSATION_LOOKUP_CACHE_MS) {
      return state.threadContentLookupResult;
    }

    const main = document.querySelector("main");
    const result = hasDescendant(main, THREAD_CONTENT_SELECTOR);
    state.threadContentLookupAt = now;
    state.threadContentLookupResult = result;
    return result;
  }

  function invalidateThreadContentCache() {
    state.threadContentLookupAt = 0;
  }

  // Pet/头像层也运行在 app://-/index.html，需要额外按 route 排除非对话窗口。
  function isConversationWindow() {
    const url = new URL(location.href);
    const route = `${url.pathname} ${url.search} ${url.hash}`.toLowerCase();
    if (route.includes("avatar-overlay") || route.includes("pet")) return false;

    return url.protocol === "app:" && url.pathname.endsWith("/index.html");
  }

  function hideMeter(root, value, fill, title) {
    if (root.dataset.known !== "false") root.dataset.known = "false";
    if (root.dataset.level !== "normal") root.dataset.level = "normal";
    if (root.title !== title) root.title = title;
    if (value.textContent !== "Context Left --") value.textContent = "Context Left --";
    if (fill.style.width !== "0%") fill.style.width = "0%";
    root.hidden = true;
    root.querySelectorAll(".ccm-hit-pop").forEach((node) => node.remove());
  }

  function makeReading(percent, source, raw, used, limit) {
    const safePercent = clampPercent(percent);
    if (safePercent == null) return null;

    return {
      percent: safePercent,
      source,
      raw: String(raw || "").slice(0, 240),
      used: Number.isFinite(used) ? used : null,
      limit: Number.isFinite(limit) ? limit : null,
      conversationId: null,
    };
  }

  function normalizeConversationId(value) {
    if (value == null) return null;
    if (typeof value !== "string" && typeof value !== "number") return null;

    const text = String(value).trim();
    if (!text) return null;

    const uuidMatch = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i.exec(text);
    if (uuidMatch) return uuidMatch[0].toLowerCase();

    return text.replace(/^[a-z]+:/i, "").toLowerCase();
  }

  function conversationIdsMatch(left, right) {
    const normalizedLeft = normalizeConversationId(left);
    const normalizedRight = normalizeConversationId(right);
    return !!normalizedLeft && !!normalizedRight && normalizedLeft === normalizedRight;
  }

  function withConversationId(reading, conversationId) {
    if (!reading) return null;

    const normalizedConversationId = normalizeConversationId(conversationId);
    if (normalizedConversationId) {
      reading.conversationId = normalizedConversationId;
    }

    return reading;
  }

  // React 私有 key 每次构建都会换后缀；按节点缓存 key 列表能避开冷启动时的大量 Reflect.ownKeys。
  function getReactPrivateKeys(node) {
    if (!node || (typeof node !== "object" && typeof node !== "function")) return [];

    const cached = state.reactPrivateKeyCache.get(node);
    if (cached) return cached;

    let keys = [];
    try {
      keys = Reflect.ownKeys(node).map(String).filter((key) => REACT_PRIVATE_KEY_RE.test(key));
    } catch {
      keys = [];
    }

    state.reactPrivateKeyCache.set(node, keys);
    return keys;
  }

  function getFilteredReflectKeys(value, cacheName, pattern, limit) {
    if (!value || (typeof value !== "object" && typeof value !== "function")) return [];

    let cache = state.filteredReflectKeyCache.get(value);
    if (!cache) {
      cache = {};
      state.filteredReflectKeyCache.set(value, cache);
    }

    if (cache[cacheName]) return cache[cacheName];

    let keys = [];
    try {
      keys = Reflect.ownKeys(value)
        .map(String)
        .filter((key) => pattern.test(key))
        .slice(0, limit);
    } catch {
      keys = [];
    }

    cache[cacheName] = keys;
    return keys;
  }

  // 只缓存对象 key 列表，不缓存属性值；React/store 对象的值会变，key 通常稳定。
  function getContextValueKeys(value) {
    return getFilteredReflectKeys(value, "contextValueText", VALUE_TEXT_KEY_RE, 100);
  }

  // Codex 部分会话 ID 只存在于 React 写到 DOM 节点上的私有 props，普通 attribute 读不到。
  function getReactPropValue(node, propName) {
    if (!node) return null;

    for (const key of getReactPrivateKeys(node)) {
      if (!key.startsWith("__reactProps$")) continue;

      let props;
      try {
        props = node[key];
      } catch {
        continue;
      }

      if (props && props[propName] != null) {
        return props[propName];
      }
    }

    return null;
  }

  function getElementConversationId(element) {
    for (let node = element; node && node.nodeType === Node.ELEMENT_NODE; node = node.parentElement) {
      const attrValue =
        node.getAttribute("data-app-action-sidebar-thread-id") ||
        node.getAttribute("data-thread-id") ||
        node.getAttribute("data-conversation-id") ||
        getReactPropValue(node, "data-app-action-sidebar-thread-id") ||
        getReactPropValue(node, "data-thread-id") ||
        getReactPropValue(node, "data-conversation-id");
      const normalized = normalizeConversationId(attrValue);
      if (normalized) return normalized;
    }

    return null;
  }

  function readActiveConversationId() {
    const now = Date.now();
    if (now - state.activeConversationIdLookupAt < ACTIVE_CONVERSATION_LOOKUP_CACHE_MS) {
      return state.cachedActiveConversationId;
    }

    // 当前会话 ID 主要挂在左侧会话列表的 current/selected/active 状态节点上。
    const selectors = [
      `[aria-current="page"][data-app-action-sidebar-thread-id]`,
      `[data-app-action-sidebar-thread-active="true"][data-app-action-sidebar-thread-id]`,
      `[aria-selected="true"][data-app-action-sidebar-thread-id]`,
      `[aria-current="page"]`,
      `[data-app-action-sidebar-thread-active="true"]`,
      `[aria-selected="true"]`,
    ];

    for (const selector of selectors) {
      const element = document.querySelector(selector);
      const conversationId = getElementConversationId(element);
      if (conversationId) {
        state.cachedActiveConversationId = conversationId;
        state.activeConversationIdLookupAt = now;
        return conversationId;
      }
    }

    state.cachedActiveConversationId = null;
    state.activeConversationIdLookupAt = now;
    return null;
  }

  function getObjectConversationId(value) {
    if (!value || typeof value !== "object") return null;

    const candidates = [
      value.conversationId,
      value.localConversationId,
      value.threadId,
      value.id,
      value.key,
      value.params && value.params.conversationId,
      value.params && value.params.localConversationId,
      value.params && value.params.threadId,
      value.thread && value.thread.id,
      value.thread && value.thread.threadId,
      value.conversation && value.conversation.id,
    ];

    for (const candidate of candidates) {
      const normalized = normalizeConversationId(candidate);
      if (normalized) return normalized;
    }

    return null;
  }

  function activateConversationId(activeConversationId, options = {}) {
    const normalizedConversationId = normalizeConversationId(activeConversationId);
    if (!normalizedConversationId) return false;

    if (normalizedConversationId !== state.activeConversationId) {
      clearRetryUpdate();
      window.clearTimeout(state.pendingUpdate);
      state.pendingUpdateDueAt = 0;
      state.activeConversationId = normalizedConversationId;
      state.cachedActiveConversationId = normalizedConversationId;
      state.activeConversationIdLookupAt = Date.now();
      state.lastReading = state.readingsByConversationId.get(normalizedConversationId) || null;
      state.lastScanAt = 0;
      state.lastScannedConversationId = null;
      state.expensiveFallbackScannedAt = 0;
      state.expensiveFallbackConversationId = null;
      state.navigationPendingUntil = options.pendingNavigation ? Date.now() + NAVIGATION_PENDING_MS : 0;
      state.switchRetryUntil = Date.now() + SWITCH_RETRY_WINDOW_MS;
      scheduleRetryUpdate();
      return true;
    }

    return false;
  }

  // 只刷新会话指针，不触发“切换会话”副作用；用于侧栏隐藏或短暂缺失 active 节点的场景。
  function retainConversationId(conversationId) {
    const normalizedConversationId = normalizeConversationId(conversationId);
    if (!normalizedConversationId) return false;

    state.activeConversationId = normalizedConversationId;
    state.cachedActiveConversationId = normalizedConversationId;
    state.activeConversationIdLookupAt = Date.now();
    return true;
  }

  function updateActiveConversationId() {
    const activeConversationId = readActiveConversationId();
    if (activeConversationId) {
      if (
        state.activeConversationId &&
        !conversationIdsMatch(activeConversationId, state.activeConversationId) &&
        Date.now() < state.navigationPendingUntil
      ) {
        return state.activeConversationId;
      }

      activateConversationId(activeConversationId);
    } else if (hasThreadContentSurface() && state.activeConversationId) {
      // sidebar 隐藏时 active/current 节点会消失；主会话区仍在时沿用最后确认的会话 ID。
      retainConversationId(state.activeConversationId);
    } else if (hasThreadContentSurface() && state.lastReading && state.lastReading.conversationId) {
      retainConversationId(state.lastReading.conversationId);
    } else {
      state.activeConversationId = null;
      state.lastReading = null;
      state.cachedActiveConversationId = null;
      state.activeConversationIdLookupAt = Date.now();
      state.navigationPendingUntil = 0;
      state.switchRetryUntil = 0;
      clearRetryUpdate();
    }

    return state.activeConversationId;
  }

  function parsePercentText(text, source) {
    if (!text || !contextTerms.test(text)) return null;

    const patterns = [
      /(?:context|token|tokens|usage|window|budget|remaining|上下文|令牌|使用|窗口)[^%\n]{0,80}?(\d{1,3}(?:\.\d+)?)\s*%/i,
      /(\d{1,3}(?:\.\d+)?)\s*%[^%\n]{0,80}?(?:context|token|tokens|usage|window|budget|remaining|上下文|令牌|使用|窗口)/i,
    ];

    for (const pattern of patterns) {
      const match = pattern.exec(text);
      if (!match) continue;

      const percent = Number(match[1]);
      if (Number.isFinite(percent) && percent >= 0 && percent <= 100) {
        const raw = match[0];
        const usedPercent = /left|remaining|remain|available|free|剩余|可用/i.test(raw)
          ? 100 - percent
          : percent;
        return makeReading(usedPercent, source, raw);
      }
    }

    return null;
  }

  // /status 输出格式的文本锚点；如果上游改了 Context 行文案，优先同步这里。
  function parseStatusContextText(text, source) {
    if (!text || !/\bContext\s*:/i.test(text)) return null;

    const normalized = String(text).replace(/\s+/g, " ");
    const statusPattern =
      /\bContext\s*:\s*(\d{1,3}(?:\.\d+)?)\s*%\s*(left|remaining|used|full)?\s*\(\s*([\d,.]+)\s*([kKmM]?)\s*used\s*\/\s*([\d,.]+)\s*([kKmM]?)\s*\)/i;
    const statusMatch = statusPattern.exec(normalized);

    if (statusMatch) {
      const reportedPercent = Number(statusMatch[1]);
      const mode = String(statusMatch[2] || "left").toLowerCase();
      const used = toNumber(statusMatch[3], statusMatch[4]);
      const limit = toNumber(statusMatch[5], statusMatch[6]);

      if (used != null && limit != null && limit > 0 && used >= 0 && used <= limit * 1.25) {
        return makeReading((used / limit) * 100, source, statusMatch[0], used, limit);
      }

      if (Number.isFinite(reportedPercent)) {
        const usedPercent = mode === "left" || mode === "remaining" ? 100 - reportedPercent : reportedPercent;
        return makeReading(usedPercent, source, statusMatch[0]);
      }
    }

    const leftPattern = /\bContext\s*:\s*(\d{1,3}(?:\.\d+)?)\s*%\s*(left|remaining)\b/i;
    const leftMatch = leftPattern.exec(normalized);
    if (leftMatch) {
      const leftPercent = Number(leftMatch[1]);
      if (Number.isFinite(leftPercent)) {
        return makeReading(100 - leftPercent, source, leftMatch[0]);
      }
    }

    return null;
  }

  function parseRatioText(text, source) {
    if (!text || !ratioWords.test(text)) return null;

    const patterns = [
      /(?:context|token|tokens|usage|window|budget|上下文|令牌|使用|窗口)[^\n]{0,100}?([\d,.]+)\s*([kKmM]?)\s*(?:\/|of)\s*([\d,.]+)\s*([kKmM]?)/i,
      /([\d,.]+)\s*([kKmM]?)\s*(?:\/|of)\s*([\d,.]+)\s*([kKmM]?)[^\n]{0,100}?(?:context|token|tokens|usage|window|budget|上下文|令牌|使用|窗口)/i,
    ];

    for (const pattern of patterns) {
      const match = pattern.exec(text);
      if (!match) continue;

      const used = toNumber(match[1], match[2]);
      const limit = toNumber(match[3], match[4]);
      if (!used || !limit || used < 0 || limit <= 0 || used > limit * 1.25) {
        continue;
      }

      return makeReading((used / limit) * 100, source, match[0], used, limit);
    }

    return null;
  }

  function parseStructuredText(text, source) {
    if (!text || !contextTerms.test(text)) return null;

    const percentFields = [
      /["']?(?:context|token|usage|window)[A-Za-z0-9_$-]{0,36}(?:percent|percentage)["']?\s*[:=]\s*(\d{1,3}(?:\.\d+)?)/i,
      /["']?(?:percent|percentage)[A-Za-z0-9_$-]{0,36}(?:context|token|usage|window)["']?\s*[:=]\s*(\d{1,3}(?:\.\d+)?)/i,
      /["']?(?:context|token|usage|window)[A-Za-z0-9_$-]{0,36}(?:ratio)["']?\s*[:=]\s*(0?\.\d+|1(?:\.0+)?)/i,
    ];

    for (const pattern of percentFields) {
      const match = pattern.exec(text);
      if (!match) continue;

      let percent = Number(match[1]);
      if (pattern.source.includes("ratio")) percent *= 100;
      if (Number.isFinite(percent) && percent >= 0 && percent <= 100) {
        return makeReading(percent, source, match[0]);
      }
    }

    const usedMatch =
      /["']?(?:context|token|tokens)[A-Za-z0-9_$-]{0,36}(?:used|current|total|input)["']?\s*[:=]\s*(\d+(?:\.\d+)?)/i.exec(
        text,
      );
    const limitMatch =
      /["']?(?:context|token|tokens)[A-Za-z0-9_$-]{0,36}(?:limit|max|window|capacity|budget)["']?\s*[:=]\s*(\d+(?:\.\d+)?)/i.exec(
        text,
      );

    if (usedMatch && limitMatch) {
      const used = Number(usedMatch[1]);
      const limit = Number(limitMatch[1]);
      if (Number.isFinite(used) && Number.isFinite(limit) && limit > 0) {
        return makeReading((used / limit) * 100, source, `${usedMatch[0]} ${limitMatch[0]}`, used, limit);
      }
    }

    return null;
  }

  function lowerCompact(value) {
    return String(value || "")
      .toLowerCase()
      .replace(/[^a-z0-9\u4e00-\u9fa5]+/g, "");
  }

  function collectNumericFields(value, path, depth, seen, fields) {
    if (fields.length > 400 || depth < 0 || value == null) return;

    const valueType = typeof value;
    if (valueType === "number" && Number.isFinite(value)) {
      const key = lowerCompact(path);
      const relevant = /(context|token|usage|window|budget|上下文|令牌|使用|窗口)/i.test(path);
      fields.push({ path, key, value, relevant });
      return;
    }

    if (valueType === "string") {
      const number = Number(value.replace(/,/g, ""));
      if (Number.isFinite(number)) {
        const key = lowerCompact(path);
        const relevant = /(context|token|usage|window|budget|上下文|令牌|使用|窗口)/i.test(path);
        fields.push({ path, key, value: number, relevant });
      }
      return;
    }

    if (valueType !== "object") return;
    if (seen.has(value)) return;
    seen.add(value);

    let keys = [];
    try {
      keys = Object.keys(value).slice(0, 120);
    } catch {
      return;
    }

    for (const key of keys) {
      let child;
      try {
        child = value[key];
      } catch {
        continue;
      }

      collectNumericFields(child, path ? `${path}.${key}` : key, depth - 1, seen, fields);
    }
  }

  function findField(fields, pattern) {
    return fields.find((field) => field.relevant && pattern.test(field.key));
  }

  // 面向未知 payload 的最后一层结构化解析：只根据字段路径语义配对数值，避免绑定单一接口形状。
  function parseStructuredValue(value, source) {
    const fields = [];
    collectNumericFields(value, "", 8, new WeakSet(), fields);

    const percentField = findField(fields, /(percent|percentage|pct|百分比)/);
    if (percentField && percentField.value >= 0 && percentField.value <= 100) {
      return makeReading(percentField.value, source, percentField.path);
    }

    const ratioField = findField(fields, /(ratio|fraction|比例)/);
    if (ratioField && ratioField.value >= 0 && ratioField.value <= 1) {
      return makeReading(ratioField.value * 100, source, ratioField.path);
    }

    const usedField = findField(fields, /(used|current|consumed|filled|total|input|prompt|已用|当前|消耗)/);
    const limitField = findField(fields, /(limit|max|maximum|capacity|window|budget|contextwindow|上限|最大|容量|预算|窗口)/);
    if (usedField && limitField && limitField.value > 0 && usedField.value >= 0 && usedField.value <= limitField.value * 1.25) {
      return makeReading(
        (usedField.value / limitField.value) * 100,
        source,
        `${usedField.path} / ${limitField.path}`,
        usedField.value,
        limitField.value,
      );
    }

    const remainingField = findField(fields, /(remaining|remain|left|available|free|剩余|可用)/);
    if (remainingField && limitField && limitField.value > 0 && remainingField.value >= 0 && remainingField.value <= limitField.value) {
      const used = limitField.value - remainingField.value;
      return makeReading(
        (used / limitField.value) * 100,
        source,
        `${remainingField.path} / ${limitField.path}`,
        used,
        limitField.value,
      );
    }

    if (usedField && remainingField && usedField.value >= 0 && remainingField.value >= 0) {
      const limit = usedField.value + remainingField.value;
      if (limit > 0) {
        return makeReading(
          (usedField.value / limit) * 100,
          source,
          `${usedField.path} / ${remainingField.path}`,
          usedField.value,
          limit,
        );
      }
    }

    return null;
  }

  function parseStatusContextUsageObject(value, source, conversationId) {
    if (!value || typeof value !== "object") return null;

    const modelContextWindow = firstFiniteNumber(value.modelContextWindow, value.model_context_window);
    const lastUsage = value.last || value.lastTokenUsage || value.last_token_usage;
    const totalTokens = firstFiniteNumber(
      lastUsage && lastUsage.totalTokens,
      lastUsage && lastUsage.total_tokens,
    );
    if (Number.isFinite(modelContextWindow) && modelContextWindow > 0 && Number.isFinite(totalTokens) && totalTokens >= 0) {
      const usedTokens = Math.min(totalTokens, modelContextWindow);
      return withConversationId(makeReading(
        (usedTokens / modelContextWindow) * 100,
        source,
        "status tokenUsage",
        usedTokens,
        modelContextWindow,
      ), conversationId);
    }

    const percent = Number(value.percent);
    const usedTokens = firstFiniteNumber(value.usedTokens, value.used_tokens);
    const contextWindow = firstFiniteNumber(value.contextWindow, value.context_window);

    if (Number.isFinite(usedTokens) && Number.isFinite(contextWindow) && contextWindow > 0) {
      return withConversationId(makeReading(
        (usedTokens / contextWindow) * 100,
        source,
        "status contextUsage",
        usedTokens,
        contextWindow,
      ), conversationId);
    }

    if (Number.isFinite(percent) && percent >= 0 && percent <= 100) {
      return withConversationId(makeReading(percent, source, "status contextUsage.percent"), conversationId);
    }

    return null;
  }

  function firstFiniteNumber(...values) {
    for (const value of values) {
      const number = Number(value);
      if (Number.isFinite(number)) return number;
    }

    return null;
  }

  function parseTokenCountInfoObject(value, source, conversationId) {
    if (!value || typeof value !== "object") return null;

    const modelContextWindow = firstFiniteNumber(value.model_context_window, value.modelContextWindow);
    const lastUsage = value.last_token_usage || value.lastTokenUsage || value.last;
    const totalTokens = firstFiniteNumber(
      lastUsage && lastUsage.total_tokens,
      lastUsage && lastUsage.totalTokens,
    );

    if (!Number.isFinite(modelContextWindow) || modelContextWindow <= 0) return null;
    if (!Number.isFinite(totalTokens) || totalTokens < 0) return null;

    const usedTokens = Math.min(totalTokens, modelContextWindow);
    return withConversationId(makeReading(
      (usedTokens / modelContextWindow) * 100,
      source,
      "token_count info",
      usedTokens,
      modelContextWindow,
    ), conversationId);
  }

  // fetch / websocket / postMessage 捕获到的对象形状不一致，这里统一收敛到 reading 并过滤非当前会话。
  function inspectCandidateValue(value, source, ownerConversationId) {
    if (!value || typeof value !== "object") return null;

    const activeConversationId = state.activeConversationId || readActiveConversationId();
    const valueConversationId = getObjectConversationId(value) || normalizeConversationId(ownerConversationId);

    if (activeConversationId && valueConversationId && !conversationIdsMatch(activeConversationId, valueConversationId)) {
      return null;
    }

    const directReading = parseStatusContextUsageObject(value, source, valueConversationId);
    if (directReading) return directReading;

    if (
      value.method === "thread/tokenUsage/updated" ||
      value.type === "thread/tokenUsage/updated" ||
      value.event === "thread/tokenUsage/updated"
    ) {
      const eventConversationId = getObjectConversationId(value.params || value) || valueConversationId;
      if (activeConversationId && eventConversationId && !conversationIdsMatch(activeConversationId, eventConversationId)) {
        return null;
      }

      const paramsReading =
        parseStatusContextUsageObject(value.params && value.params.tokenUsage, source, eventConversationId) ||
        parseStatusContextUsageObject(value.params, source, eventConversationId);
      if (paramsReading) return paramsReading;
    }

    if (value.type === "token_count" || value.event === "token_count") {
      const tokenCountReading = parseTokenCountInfoObject(value.info, source, valueConversationId || activeConversationId);
      if (tokenCountReading) return tokenCountReading;
    }

    if (value.payload && value.payload.type === "token_count") {
      const payloadConversationId = getObjectConversationId(value.payload) || valueConversationId || activeConversationId;
      const tokenCountReading = parseTokenCountInfoObject(value.payload.info, source, payloadConversationId);
      if (tokenCountReading) return tokenCountReading;
    }

    const nestedReading = parseStatusContextUsageObject(
      value.contextUsage || value.tokenUsage || value.usage,
      source,
      valueConversationId,
    );
    if (nestedReading) return nestedReading;

    const tokenCountInfoReading = parseTokenCountInfoObject(value.info, source, valueConversationId || activeConversationId);
    if (tokenCountInfoReading) return tokenCountInfoReading;

    return null;
  }

  // 反向遍历 React / window 状态树时，Map key 或父对象常常比叶子值更像会话归属来源。
  function findStatusContextUsageObject(value, depth, seen, activeConversationId, ownerConversationId) {
    if (!value || typeof value !== "object" || depth < 0) return null;
    if (seen.has(value)) return null;
    seen.add(value);

    const valueConversationId = getObjectConversationId(value);
    const nextOwnerConversationId = valueConversationId || ownerConversationId;
    const ownerMatchesActive =
      !activeConversationId ||
      conversationIdsMatch(activeConversationId, nextOwnerConversationId) ||
      conversationIdsMatch(activeConversationId, ownerConversationId);

    if (ownerMatchesActive) {
      const direct = parseStatusContextUsageObject(value, "status-react", nextOwnerConversationId);
      if (direct) return direct;
    }

    if (Array.isArray(value)) {
      const limit = Math.min(value.length, 120);
      for (let index = 0; index < limit; index += 1) {
        const reading = findStatusContextUsageObject(
          value[index],
          depth - 1,
          seen,
          activeConversationId,
          nextOwnerConversationId,
        );
        if (reading) return reading;
      }
      return null;
    }

    if (value instanceof Map) {
      let index = 0;
      for (const [mapKey, mapValue] of value) {
        if (index >= 120) break;

        const mapKeyConversationId = normalizeConversationId(mapKey);
        const childOwnerConversationId = mapKeyConversationId || nextOwnerConversationId;

        const keyReading = findStatusContextUsageObject(
          mapKey,
          depth - 1,
          seen,
          activeConversationId,
          childOwnerConversationId,
        );
        if (keyReading) return keyReading;

        const valueReading = findStatusContextUsageObject(
          mapValue,
          depth - 1,
          seen,
          activeConversationId,
          childOwnerConversationId,
        );
        if (valueReading) return valueReading;

        index += 1;
      }
      return null;
    }

    if (value instanceof Set) {
      let index = 0;
      for (const setValue of value) {
        if (index >= 120) break;

        const reading = findStatusContextUsageObject(
          setValue,
          depth - 1,
          seen,
          activeConversationId,
          nextOwnerConversationId,
        );
        if (reading) return reading;

        index += 1;
      }
      return null;
    }

    const keys = getFilteredReflectKeys(value, "statusTree", STATUS_TREE_KEY_RE, 120);
    const keySet = new Set(keys);

    for (const key of PREFERRED_STATUS_KEYS) {
      if (!keySet.has(key)) continue;

      let child;
      try {
        child = value[key];
      } catch {
        continue;
      }

      const reading = findStatusContextUsageObject(
        child,
        depth - 1,
        seen,
        activeConversationId,
        nextOwnerConversationId,
      );
      if (reading) return reading;
    }

    for (const key of keys) {
      if (PREFERRED_STATUS_KEY_SET.has(key)) continue;

      let child;
      try {
        child = value[key];
      } catch {
        continue;
      }

      const keyConversationId = normalizeConversationId(key);
      const childOwnerConversationId = keyConversationId || nextOwnerConversationId;
      const reading = findStatusContextUsageObject(
        child,
        depth - 1,
        seen,
        activeConversationId,
        childOwnerConversationId,
      );
      if (reading) return reading;
    }

    return null;
  }

  // React 私有字段是从界面节点回溯运行态状态的桥；升级后若读不到值，优先检查 __react* 键。
  function scanStatusReactContextUsage(activeConversationId) {
    const nodes = [
      document.getElementById("root"),
      document.querySelector(`[aria-current="page"]`),
      document.querySelector(`[data-app-action-sidebar-thread-active="true"]`),
      document.body,
      document.documentElement,
    ].filter(Boolean);
    const limit = nodes.length;

    for (let index = 0; index < limit; index += 1) {
      const node = nodes[index];
      const keys = getReactPrivateKeys(node);

      for (const key of keys) {
        let value;
        try {
          value = node[key];
        } catch {
          continue;
        }

        const reading = findStatusContextUsageObject(value, 10, new WeakSet(), activeConversationId, null);
        if (reading) return reading;
      }
    }

    return null;
  }

  function isAppSignalScope(value) {
    return !!(
      value &&
      typeof value === "object" &&
      typeof value.get === "function" &&
      typeof value.watch === "function" &&
      value.node &&
      value.chain
    );
  }

  // app signal scope 没有稳定全局入口，只能从 React fiber 链里按结构特征反查。
  function findAppSignalScopeInValue(value, depth, seen) {
    if (!value || typeof value !== "object" || depth < 0) return null;
    if (seen.has(value)) return null;
    if (state.appSignalSkipGeneration.get(value) === state.scanGeneration) return null;
    seen.add(value);

    if (isAppSignalScope(value)) return value;

    if (Array.isArray(value)) {
      const limit = Math.min(value.length, 40);
      for (let index = 0; index < limit; index += 1) {
        const scope = findAppSignalScopeInValue(value[index], depth - 1, seen);
        if (scope) return scope;
      }
      return null;
    }

    if (value instanceof Map) {
      let index = 0;
      for (const [mapKey, mapValue] of value) {
        if (index >= 30) break;

        const keyScope = findAppSignalScopeInValue(mapKey, depth - 1, seen);
        if (keyScope) return keyScope;

        const valueScope = findAppSignalScopeInValue(mapValue, depth - 1, seen);
        if (valueScope) return valueScope;

        index += 1;
      }
    }

    const keys = getFilteredReflectKeys(value, "appSignalScope", APP_SIGNAL_SCOPE_KEY_RE, 120);

    for (const key of keys) {
      let child;
      try {
        child = value[key];
      } catch {
        continue;
      }

      const scope = findAppSignalScopeInValue(child, depth - 1, seen);
      if (scope) return scope;
    }

    state.appSignalSkipGeneration.set(value, state.scanGeneration);
    return null;
  }

  function findAppSignalScope() {
    if (isAppSignalScope(state.appSignalScope)) return state.appSignalScope;

    const now = Date.now();
    if (now - state.appSignalLastLookupAt < 2000) return null;
    state.appSignalLastLookupAt = now;

    // app signal scope 挂在 React 树里；只扫稳定锚点，避免初始化时遍历整页 DOM。
    const root = document.getElementById("root");
    const current = document.querySelector(`[aria-current="page"]`);
    const activeThread = document.querySelector(`[data-app-action-sidebar-thread-active="true"]`);
    const nodes = [
      root,
      current,
      activeThread,
      current && current.closest("[data-app-action-sidebar-thread-id]"),
      activeThread && activeThread.closest("[data-app-action-sidebar-thread-id]"),
      document.body,
      document.documentElement,
      ...(root
        ? Array.from(root.querySelectorAll(REACT_STATE_HOST_SELECTOR)).slice(0, REACT_HOST_SCAN_LIMIT)
        : []),
    ].filter(Boolean);

    const seen = new WeakSet();
    for (const node of nodes) {
      const keys = getReactPrivateKeys(node);
      for (const key of keys) {
        let value;
        try {
          value = node[key];
        } catch {
          continue;
        }

        const scope = findAppSignalScopeInValue(value, 18, seen);
        if (scope) {
          state.appSignalScope = scope;
          return scope;
        }
      }
    }

    return null;
  }

  // hashed asset 文件名会随版本变化；先从已加载资源定位，fallback 只保留当前版本的相对路径。
  function findLoadedAssetUrl(fragment, fallbackPath) {
    const selectors = [
      `script[src*="${fragment}"]`,
      `link[href*="${fragment}"]`,
    ];

    for (const selector of selectors) {
      const element = document.querySelector(selector);
      const value = element && (element.src || element.href);
      if (value) return value;
    }

    const resources =
      typeof performance !== "undefined" && typeof performance.getEntriesByType === "function"
        ? performance.getEntriesByType("resource")
        : [];
    for (const resource of resources) {
      if (resource && typeof resource.name === "string" && resource.name.includes(fragment)) {
        return resource.name;
      }
    }

    return new URL(fallbackPath, location.href).href;
  }

  // 优先读取 Status 使用的 app signal；bundle hash 或导出名变化时，先更新这两个资产入口。
  function ensureAppSignalModules() {
    if (state.appSignalModules) return state.appSignalModules;
    if (state.appSignalModulesPromise) return null;
    state.appSignalModulesRequestedAt = Date.now();

    const appServerUrl = findLoadedAssetUrl(
      "app-server-manager-signals",
      "./assets/app-server-manager-signals-WXrD8bmC.js",
    );
    const signalUrl = findLoadedAssetUrl(
      "setting-storage",
      "./assets/setting-storage-DBp4-kRn.js",
    );

    state.appSignalModulesPromise = Promise.all([
      import(appServerUrl),
      import(signalUrl),
    ])
      .then(([appServerSignals, signalStorage]) => {
        state.appSignalModules = { appServerSignals, signalStorage };
        scheduleUpdate();
        return state.appSignalModules;
      })
      .catch(() => {
        state.appSignalModulesPromise = null;
        return null;
      });

    return null;
  }

  // setting-storage 的 rt helper 会解开嵌套 signal；缺失时退回 scope.get 的两段读取。
  function readSignalValue(scope, selector, argument) {
    if (!scope || !selector) return null;

    const modules = state.appSignalModules;
    const readHelper = modules && modules.signalStorage && modules.signalStorage.rt;
    if (typeof readHelper === "function") {
      try {
        return readHelper(scope, selector, argument);
      } catch {
        return null;
      }
    }

    try {
      const nestedSignal = scope.get(selector, argument);
      if (nestedSignal && typeof nestedSignal === "object") {
        return scope.get(nestedSignal);
      }
    } catch {
      return null;
    }

    return null;
  }

  function scanAppSignalContextUsage(activeConversationId) {
    const now = Date.now();
    const requestedConversationId = normalizeConversationId(activeConversationId);
    if (
      state.appSignalCachedReading &&
      requestedConversationId &&
      conversationIdsMatch(requestedConversationId, state.appSignalCachedConversationId) &&
      now - state.appSignalCachedAt < APP_SIGNAL_READING_CACHE_MS
    ) {
      return state.appSignalCachedReading;
    }

    const modules = ensureAppSignalModules();
    if (!modules || !modules.appServerSignals) {
      state.waitingForAppSignalModules = !!state.appSignalModulesPromise;
      return null;
    }
    state.waitingForAppSignalModules = false;

    const scope = findAppSignalScope();
    if (!scope) return null;

    const conversationId =
      normalizeConversationId(activeConversationId) ||
      normalizeConversationId(scope.value && scope.value.conversationId);
    if (!conversationId) return null;

    // 当前版本里 appServerSignals.B 对应 latestTokenUsageInfo；上游重排导出时需要重新定位。
    const latestTokenUsageSelector = modules.appServerSignals.B;
    const tokenUsage = readSignalValue(scope, latestTokenUsageSelector, conversationId);
    const reading = parseStatusContextUsageObject(tokenUsage, "app-signal", conversationId);
    if (reading) {
      state.appSignalCachedReading = reading;
      state.appSignalCachedConversationId = conversationId;
      state.appSignalCachedAt = now;
      return reading;
    }

    return null;
  }

  function shouldWaitForAppSignalModules(now) {
    return !!(
      state.appSignalModulesPromise &&
      !state.appSignalModules &&
      now - state.appSignalModulesRequestedAt < APP_SIGNAL_IMPORT_GRACE_MS
    );
  }

  // 流式响应可能是 JSONL，也可能是一整段 JSON；两种都按同一套 reading 规则落库。
  function parsePayloadText(text, source, ownerConversationId) {
    const clipped = String(text || "").slice(0, MAX_CAPTURE_TEXT_LENGTH);
    const textReading = parseTextForReading(clipped, source);
    if (textReading) return withConversationId(textReading, ownerConversationId);

    if (!contextTerms.test(clipped)) return null;

    const lines = clipped.split(/\r?\n/);
    if (lines.length > 1) {
      for (const line of lines) {
        if (!line || !contextTerms.test(line)) continue;

        try {
          const parsedLine = JSON.parse(line);
          const reading =
            inspectCandidateValue(parsedLine, source, ownerConversationId) ||
            parseStructuredValue(parsedLine, source);
          if (reading) return withConversationId(reading, reading.conversationId || ownerConversationId);
        } catch {
          const lineReading = parseTextForReading(line, source);
          if (lineReading) return withConversationId(lineReading, ownerConversationId);
        }
      }
    }

    try {
      const parsed = JSON.parse(clipped);
      const reading = inspectCandidateValue(parsed, source, ownerConversationId) || parseStructuredValue(parsed, source);
      return withConversationId(reading, reading && reading.conversationId ? reading.conversationId : ownerConversationId);
    } catch {
      return null;
    }
  }

  // 捕获层会先缓存非当前会话读数，只有会话匹配时才推动当前 UI 刷新。
  function acceptReading(reading) {
    if (!reading) return;

    const activeConversationId = state.activeConversationId || readActiveConversationId();
    if (reading.conversationId) {
      state.readingsByConversationId.set(reading.conversationId, reading);
    }

    if (activeConversationId && reading.conversationId && !conversationIdsMatch(activeConversationId, reading.conversationId)) {
      return;
    }

    state.lastReading = reading;
    scheduleUpdate(CAPTURE_UPDATE_DELAY_MS);
  }

  function inspectCandidateText(text, source, ownerConversationId) {
    if (!text || text.length > MAX_CAPTURE_TEXT_LENGTH) return;
    if (!CAPTURE_TEXT_HINT_RE.test(text)) return;

    acceptReading(parsePayloadText(text, source, ownerConversationId));
  }

  function hasCaptureHintText(text) {
    if (typeof text !== "string" || text.length > MAX_CAPTURE_TEXT_LENGTH) return false;
    return CAPTURE_TEXT_HINT_RE.test(text);
  }

  function getRequestConversationId(input) {
    const direct = normalizeConversationId(input && (input.url || input.href || input));
    if (direct) return direct;

    return state.activeConversationId || readActiveConversationId();
  }

  function getNativeFetch(captureState) {
    return captureState.nativeFetch || window.fetch;
  }

  function getNativeWebSocket(captureState) {
    return captureState.NativeWebSocket || window.WebSocket;
  }

  // fetch 捕获只 clone 小体积文本响应，不消费原 response，避免影响 Codex 自己的请求链。
  function installFetchCapture() {
    const captureState = getCaptureState();
    if (captureState.fetchVersion === SCRIPT_VERSION || typeof window.fetch !== "function") return;

    const originalFetch = getNativeFetch(captureState);
    captureState.nativeFetch = originalFetch;
    window.fetch = function codexContextMeterFetch(...args) {
      const requestConversationId = getRequestConversationId(args[0]);
      return originalFetch.apply(this, args).then((response) => {
        try {
          const urlText = String(args[0] && (args[0].url || args[0].href || args[0]) || response.url || "");
          if (urlText && !CAPTURE_TEXT_HINT_RE.test(urlText)) return response;

          const contentType = response.headers && response.headers.get("content-type");
          const contentLength = response.headers && Number(response.headers.get("content-length"));
          const isTextLike = !contentType || /json|text|event-stream|x-ndjson/i.test(contentType);
          if (isTextLike && (!Number.isFinite(contentLength) || contentLength <= MAX_CAPTURE_TEXT_LENGTH)) {
            response
              .clone()
              .text()
              .then((text) => {
                const currentCaptureState = window[CAPTURE_STATE_KEY];
                if (currentCaptureState && typeof currentCaptureState.inspectText === "function") {
                  currentCaptureState.inspectText(text, "fetch", requestConversationId);
                }
              })
              .catch(() => {});
          }
        } catch {
          return response;
        }

        return response;
      });
    };

    captureState.fetchVersion = SCRIPT_VERSION;
    window.__codexContextMeterFetchPatched = true;
  }

  // WebSocket 构造器需要保留原型和 readyState 常量，减少对页面侧类型判断的影响。
  function installWebSocketCapture() {
    const captureState = getCaptureState();
    if (captureState.webSocketVersion === SCRIPT_VERSION || typeof window.WebSocket !== "function") return;

    const NativeWebSocket = getNativeWebSocket(captureState);
    captureState.NativeWebSocket = NativeWebSocket;

    function MeterWebSocket(...args) {
      const socket = new NativeWebSocket(...args);
      socket.addEventListener("message", (event) => {
        try {
          if (typeof event.data === "string") {
            if (!hasCaptureHintText(event.data)) return;

            const currentCaptureState = window[CAPTURE_STATE_KEY];
            if (currentCaptureState && typeof currentCaptureState.inspectText === "function") {
              currentCaptureState.inspectText(event.data, "websocket", state.activeConversationId || readActiveConversationId());
            }
          } else if (event.data instanceof Blob && event.data.size <= MAX_CAPTURE_TEXT_LENGTH) {
            event.data
              .text()
              .then((text) => {
                const currentCaptureState = window[CAPTURE_STATE_KEY];
                if (currentCaptureState && typeof currentCaptureState.inspectText === "function") {
                  currentCaptureState.inspectText(text, "websocket", state.activeConversationId || readActiveConversationId());
                }
              })
              .catch(() => {});
          }
        } catch {
          return;
        }
      });
      return socket;
    }

    MeterWebSocket.prototype = NativeWebSocket.prototype;
    MeterWebSocket.CONNECTING = NativeWebSocket.CONNECTING;
    MeterWebSocket.OPEN = NativeWebSocket.OPEN;
    MeterWebSocket.CLOSING = NativeWebSocket.CLOSING;
    MeterWebSocket.CLOSED = NativeWebSocket.CLOSED;
    window.WebSocket = MeterWebSocket;
    captureState.webSocketVersion = SCRIPT_VERSION;
    window.__codexContextMeterWebSocketPatched = true;
  }

  // postMessage 监听器每次重装前先移除旧实例，支持 Codex++ 反复重新加载用户脚本。
  function installPostMessageCapture() {
    const captureState = getCaptureState();
    if (captureState.messageListener) {
      window.removeEventListener("message", captureState.messageListener, true);
    }

    captureState.messageListener = (event) => {
      try {
        const currentCaptureState = window[CAPTURE_STATE_KEY];
        if (!currentCaptureState) return;

        const reading =
          (typeof currentCaptureState.inspectValue === "function"
            ? currentCaptureState.inspectValue(event.data, "message", state.activeConversationId || readActiveConversationId())
            : null) ||
          (hasCaptureHintText(event.data) && typeof currentCaptureState.parsePayloadText === "function"
            ? currentCaptureState.parsePayloadText(event.data, "message", state.activeConversationId || readActiveConversationId())
            : null);

        if (typeof currentCaptureState.acceptReading === "function") {
          currentCaptureState.acceptReading(reading);
        }
      } catch {
        return;
      }
    };

    window.addEventListener("message", captureState.messageListener, true);
    captureState.messageVersion = SCRIPT_VERSION;
    window.__codexContextMeterPostMessagePatched = true;
  }

  function parseTextForReading(text, source) {
    const clipped = String(text || "").slice(0, MAX_TEXT_LENGTH);
    return (
      parseStatusContextText(clipped, source) ||
      parsePercentText(clipped, source) ||
      parseRatioText(clipped, source) ||
      parseStructuredText(clipped, source)
    );
  }

  function isVisibleElement(element) {
    if (!element || element.nodeType !== Node.ELEMENT_NODE) return false;

    const rect = element.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return false;

    const style = getComputedStyle(element);
    return style.display !== "none" && style.visibility !== "hidden" && Number(style.opacity || "1") > 0;
  }

  function isConversationContent(element) {
    return !!(element && element.closest(CONVERSATION_CONTENT_SELECTOR));
  }

  function shouldIgnoreMutationTarget(element) {
    if (!element || element.nodeType !== Node.ELEMENT_NODE) return false;
    if (element.closest(`#${ROOT_ID}`)) return true;
    return !!element.closest(MESSAGE_MUTATION_SELECTOR);
  }

  // 兜底读取已渲染的 /status 输出；排除会话正文，避免把用户消息里的 Context 行当成状态。
  function collectStatusCommandText() {
    const chunks = [];
    const walker = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_ELEMENT);
    let visited = 0;
    let node = walker.nextNode();

    while (node && visited < STATUS_TEXT_NODE_SCAN_LIMIT) {
      visited += 1;
      if (chunks.length >= 12) break;
      const current = node;
      node = walker.nextNode();

      const text = (current.textContent || "").replace(/\s+/g, " ").trim();
      if (text.length < 20 || text.length > 1800) continue;
      if (!/\bStatus\b/i.test(text) || !/\bContext\s*:\s*\d{1,3}(?:\.\d+)?\s*%/i.test(text)) continue;
      if (!/\bSession\s*:|\b5h limit\s*:|\b7d limit\s*:/i.test(text)) continue;
      if (current.id === ROOT_ID || current.closest(`#${ROOT_ID}`)) continue;
      if (isConversationContent(current)) continue;
      if (!isVisibleElement(current)) continue;

      chunks.push(text);
    }

    return chunks.join("\n");
  }

  function collectDomText() {
    const parts = [];
    let scanned = 0;

    const attrNodes = document.querySelectorAll("[aria-label], [title], [data-testid], [data-test-id]");
    for (const node of attrNodes) {
      if (scanned >= DOM_ATTRIBUTE_SCAN_LIMIT || parts.length >= DOM_TEXT_PART_LIMIT) break;
      scanned += 1;
      if (node.closest(`#${ROOT_ID}`)) continue;

      for (const attr of ["aria-label", "title", "data-testid", "data-test-id"]) {
        const value = node.getAttribute(attr);
        if (value && contextTerms.test(value)) parts.push(value);
      }

      const text = (node.textContent || "").trim();
      if (text && text.length <= 500 && contextTerms.test(text)) {
        parts.push(text);
      }
    }

    return parts.join("\n").slice(0, MAX_TEXT_LENGTH);
  }

  function safeStorageText(storage) {
    const parts = [];
    for (let index = 0; index < storage.length && parts.length < 80; index += 1) {
      const key = storage.key(index);
      if (!key || !contextTerms.test(key)) continue;

      let value = "";
      try {
        value = storage.getItem(key) || "";
      } catch {
        value = "";
      }
      parts.push(`${key}: ${value.slice(0, 4000)}`);
    }
    return parts.join("\n");
  }

  function collectValueText(value, depth, seen, parts) {
    if (parts.length > 160 || depth < 0 || value == null) return;

    const valueType = typeof value;
    if (valueType === "string" || valueType === "number" || valueType === "boolean") {
      const text = String(value);
      if (contextTerms.test(text)) parts.push(text);
      return;
    }

    if (valueType !== "object") return;
    if (seen.has(value)) return;
    seen.add(value);

    if (Array.isArray(value)) {
      const limit = Math.min(value.length, 80);
      for (let index = 0; index < limit; index += 1) {
        collectValueText(value[index], depth - 1, seen, parts);
      }
      return;
    }

    if (value instanceof Map) {
      let index = 0;
      for (const [mapKey, mapValue] of value) {
        if (index >= 80) break;
        collectValueText(mapKey, depth - 1, seen, parts);
        collectValueText(mapValue, depth - 1, seen, parts);
        index += 1;
      }
      return;
    }

    if (value instanceof Set) {
      let index = 0;
      for (const setValue of value) {
        if (index >= 80) break;
        collectValueText(setValue, depth - 1, seen, parts);
        index += 1;
      }
      return;
    }

    const keys = getContextValueKeys(value);

    for (const key of keys) {
      let child;
      try {
        child = value[key];
      } catch {
        continue;
      }

      if (child == null || typeof child !== "object") {
        parts.push(`${key}: ${String(child)}`);
      } else {
        parts.push(key);
      }

      if (typeof child === "object") {
        collectValueText(child, depth - 1, seen, parts);
      }
    }
  }

  // window 全局对象很大，先缓存候选 key；只有主路径失败时才读取这些值。
  function scanLikelyWindowState() {
    const parts = [];
    const seen = new WeakSet();
    const now = Date.now();
    if (!state.windowContextTextKeys || now - state.windowContextTextKeysAt > WINDOW_KEY_CACHE_MS) {
      state.windowContextTextKeys = Object.keys(window).filter((key) => contextTerms.test(key));
      state.windowContextTextKeysAt = now;
    }

    for (const key of state.windowContextTextKeys) {
      let value;
      try {
        value = window[key];
      } catch {
        continue;
      }

      parts.push(key);
      collectValueText(value, 3, seen, parts);
    }

    return parts.join("\n").slice(0, MAX_TEXT_LENGTH);
  }

  // React 兜底只检查可能承载线程/消息状态的宿主节点，避免遍历全部 body 元素。
  function scanReactProps() {
    const parts = [];
    const seen = new WeakSet();
    const root = document.getElementById("root");
    const nodes = root ? root.querySelectorAll(REACT_STATE_HOST_SELECTOR) : [];
    const limit = Math.min(nodes.length, REACT_HOST_SCAN_LIMIT);

    for (let index = 0; index < limit && parts.length < 160; index += 1) {
      const node = nodes[index];
      const keys = getReactPrivateKeys(node);
      for (const key of keys) {
        let value;
        try {
          value = node[key];
        } catch {
          continue;
        }
        collectValueText(value, 4, seen, parts);
      }
    }

    return parts.join("\n").slice(0, MAX_TEXT_LENGTH);
  }

  // 这里直接找结构化 usage 对象，比把 window 状态拼成文本再正则解析更便宜。
  function scanWindowForContextUsage(activeConversationId) {
    const seen = new WeakSet();
    const now = Date.now();
    if (!state.windowUsageKeys || now - state.windowUsageKeysAt > WINDOW_KEY_CACHE_MS) {
      state.windowUsageKeys = Object.keys(window).filter((key) =>
        /codex|thread|token|usage|context|store|query|cache|notification|message/i.test(key),
      );
      state.windowUsageKeysAt = now;
    }

    for (const key of state.windowUsageKeys) {
      let value;
      try {
        value = window[key];
      } catch {
        continue;
      }

      const reading = findStatusContextUsageObject(value, 6, seen, activeConversationId, null);
      if (reading) return reading;
    }

    return null;
  }

  // 读取顺序按稳定性排列：app signal > React 状态 > window 缓存 > /status 文本 > storage/DOM 兜底。
  function detectReading() {
    state.scanGeneration += 1;
    const activeConversationId = updateActiveConversationId();
    const cachedReading = activeConversationId ? state.readingsByConversationId.get(activeConversationId) : null;
    if (cachedReading) return cachedReading;

    if (!activeConversationId) {
      const appSignalReading = scanAppSignalContextUsage(null);
      if (appSignalReading && appSignalReading.conversationId) {
        retainConversationId(appSignalReading.conversationId);
        state.switchRetryUntil = 0;
        clearRetryUpdate();
        return appSignalReading;
      }

      return null;
    }

    if (state.lastReading && state.lastReading.conversationId && conversationIdsMatch(activeConversationId, state.lastReading.conversationId)) {
      return state.lastReading;
    }

    const now = Date.now();
    const activeChangedSinceScan = activeConversationId !== state.lastScannedConversationId;
    const inSwitchRetryWindow = !!activeConversationId && now < state.switchRetryUntil;
    if (!activeChangedSinceScan && !inSwitchRetryWindow && now - state.lastScanAt < SLOW_SCAN_INTERVAL_MS) {
      return null;
    }
    // 会话切换初期允许快速兜底；同一会话内的昂贵扫描按窗口限频。
    const canRunExpensiveFallback =
      activeChangedSinceScan ||
      !conversationIdsMatch(activeConversationId, state.expensiveFallbackConversationId) ||
      now - state.expensiveFallbackScannedAt >= EXPENSIVE_FALLBACK_INTERVAL_MS;

    state.lastScannedConversationId = activeConversationId;
    state.lastScanAt = now;

    const appSignalReading = scanAppSignalContextUsage(activeConversationId);
    if (appSignalReading) {
      if (!activeConversationId && appSignalReading.conversationId) {
        retainConversationId(appSignalReading.conversationId);
      }
      state.switchRetryUntil = 0;
      clearRetryUpdate();
      return appSignalReading;
    }

    if (shouldWaitForAppSignalModules(now)) {
      scheduleUpdate(APP_SIGNAL_IMPORT_GRACE_MS);
      return state.lastReading;
    }

    const statusReactReading = scanStatusReactContextUsage(activeConversationId);
    if (statusReactReading) {
      state.switchRetryUntil = 0;
      clearRetryUpdate();
      return statusReactReading;
    }

    if (canRunExpensiveFallback) {
      state.expensiveFallbackScannedAt = now;
      state.expensiveFallbackConversationId = activeConversationId;

      // 以下 fallback 从“结构化且便宜”逐步降级到“文本化且昂贵”，顺序不要随意调换。
      const windowReading = scanWindowForContextUsage(activeConversationId);
      if (windowReading) {
        state.switchRetryUntil = 0;
        clearRetryUpdate();
        return windowReading;
      }

      const statusReading = parseTextForReading(collectStatusCommandText(), "status");
      if (statusReading) {
        state.switchRetryUntil = 0;
        clearRetryUpdate();
        return statusReading;
      }

      const localReading = parseTextForReading(safeStorageText(localStorage), "localStorage");
      if (localReading) {
        state.switchRetryUntil = 0;
        clearRetryUpdate();
        return localReading;
      }

      const sessionReading = parseTextForReading(safeStorageText(sessionStorage), "sessionStorage");
      if (sessionReading) {
        state.switchRetryUntil = 0;
        clearRetryUpdate();
        return sessionReading;
      }

      const domReading = parseTextForReading(collectDomText(), "dom");
      if (domReading) {
        state.switchRetryUntil = 0;
        clearRetryUpdate();
        return domReading;
      }

      const slowWindowReading = parseTextForReading(scanLikelyWindowState(), "window");
      if (slowWindowReading) return slowWindowReading;

      const reactReading = parseTextForReading(scanReactProps(), "react");
      if (reactReading) return reactReading;
    }

    if (inSwitchRetryWindow) {
      scheduleRetryUpdate();
    } else {
      clearRetryUpdate();
    }

    if (
      !state.lastReading ||
      !state.lastReading.conversationId ||
      conversationIdsMatch(activeConversationId, state.lastReading.conversationId)
    ) {
      return state.lastReading;
    }

    return null;
  }

  function updateMeter() {
    installStyle();

    if (!document.body) return;

    const root = ensureRoot();
    const value = state.value || root.querySelector(".ccm-value");
    const fill = state.fill || root.querySelector(".ccm-fill");
    const reading = detectReading();
    const activeConversationId = state.activeConversationId || readActiveConversationId();

    if (!value || !fill) return;

    if (!hasThreadContentSurface()) {
      state.lastReading = null;
      hideMeter(root, value, fill, "No thread content is open in the main view.");
      return;
    }

    if (!reading) {
      if (state.waitingForAppSignalModules) {
        scheduleUpdate(APP_SIGNAL_IMPORT_GRACE_MS);
        if (root.dataset.known === "true" && value.textContent !== "Context Left --") return;
        hideMeter(root, value, fill, "Waiting for Codex context usage signal.");
        return;
      }

      const title = activeConversationId
        ? `No context usage value is exposed for conversation ${activeConversationId} in the current page state yet.`
        : "No context usage value is exposed in the current page state yet.";
      hideMeter(root, value, fill, title);
      return;
    }

    state.lastReading = reading;

    const leftPercent = clampPercent(100 - reading.percent);
    const percentText = leftPercent.toFixed(1);
    const details =
      reading.used != null && reading.limit != null
        ? ` ${compactNumber(reading.used)} / ${compactNumber(reading.limit)}`
        : "";
    const readingConversationId = normalizeConversationId(
      reading.conversationId || activeConversationId || "__unknown__"
    );

    if (readingConversationId && Number.isFinite(reading.used)) {
      const previousUsed = state.lastAnimatedUsedByConversationId.get(readingConversationId);
      if (Number.isFinite(previousUsed) && reading.used > previousUsed) {
        showTokenSpendEffect(root, reading.used - previousUsed);
      }
      state.lastAnimatedUsedByConversationId.set(readingConversationId, reading.used);
    }

    const level = leftPercent <= 10 ? "danger" : leftPercent <= 25 ? "warn" : "normal";
    const title = `Source: ${reading.source}${reading.raw ? ` | ${reading.raw}` : ""}`;
    const text = `Context Left ${percentText}%${details}`;
    const width = `${leftPercent.toFixed(1)}%`;

    if (root.dataset.known !== "true") root.dataset.known = "true";
    if (root.hidden) root.hidden = false;
    if (root.dataset.level !== level) root.dataset.level = level;
    if (root.title !== title) root.title = title;
    if (value.textContent !== text) value.textContent = text;
    if (fill.style.width !== width) fill.style.width = width;
  }

  function scheduleUpdate(delayMs = MUTATION_UPDATE_DELAY_MS) {
    const delay = Math.max(0, Number(delayMs) || 0);
    const dueAt = Date.now() + delay;
    if (state.pendingUpdate && state.pendingUpdateDueAt <= dueAt) return;

    window.clearTimeout(state.pendingUpdate);
    state.pendingUpdateDueAt = dueAt;
    state.pendingUpdate = window.setTimeout(() => {
      state.pendingUpdate = 0;
      state.pendingUpdateDueAt = 0;
      updateMeter();
    }, delay);
  }

  // 提前捕获侧栏会话切换，避免 DOM/状态更新滞后时短暂沿用旧会话读数。
  function handlePotentialNavigation(event) {
    const target = event.target && event.target.closest
      ? event.target.closest("[data-app-action-sidebar-thread-id]")
      : null;
    const conversationId = getElementConversationId(target);
    if (!conversationId) return;

    activateConversationId(conversationId, { pendingNavigation: true });
    scheduleUpdate(CAPTURE_UPDATE_DELAY_MS);
  }

  function clearRetryUpdate() {
    window.clearTimeout(state.retryTimer);
    state.retryTimer = 0;
  }

  function scheduleRetryUpdate() {
    if (state.retryTimer || Date.now() >= state.switchRetryUntil) return;

    state.retryTimer = window.setTimeout(() => {
      state.retryTimer = 0;
      updateMeter();
    }, SWITCH_RETRY_INTERVAL_MS);
  }

  function installObserver() {
    if (state.observer) return;

    state.navigationListener = handlePotentialNavigation;
    document.addEventListener("pointerdown", state.navigationListener, true);
    document.addEventListener("click", state.navigationListener, true);
    document.addEventListener("keydown", state.navigationListener, true);

    state.observer = new MutationObserver((mutations) => {
      for (const mutation of mutations) {
      const target = mutation.target && mutation.target.nodeType === Node.ELEMENT_NODE
          ? mutation.target
          : mutation.target && mutation.target.parentElement;
        if (shouldIgnoreMutationTarget(target)) continue;

        invalidateThreadContentCache();
        scheduleUpdate(MUTATION_UPDATE_DELAY_MS);
        return;
      }
    });
    state.observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: [
        // 会话切换只依赖侧栏 active/current/id 相关属性；Codex 改名时同步这组属性。
        "aria-current",
        "aria-selected",
        "data-app-action-sidebar-thread-active",
        "data-app-action-sidebar-thread-id",
      ],
      childList: true,
      subtree: true,
    });
  }

  window[API_KEY] = {
    version: SCRIPT_VERSION,
    refresh: updateMeter,
    destroy() {
      window.clearInterval(state.timer);
      window.clearTimeout(state.pendingUpdate);
      state.pendingUpdateDueAt = 0;
      clearRetryUpdate();
      if (state.observer) state.observer.disconnect();
      if (state.navigationListener) {
        document.removeEventListener("pointerdown", state.navigationListener, true);
        document.removeEventListener("click", state.navigationListener, true);
        document.removeEventListener("keydown", state.navigationListener, true);
      }

      const root = document.getElementById(ROOT_ID);
      if (root) root.remove();

      const style = document.getElementById(STYLE_ID);
      if (style) style.remove();

      const captureState = window[CAPTURE_STATE_KEY];
      if (captureState && captureState.messageListener) {
        window.removeEventListener("message", captureState.messageListener, true);
        delete captureState.messageListener;
      }
      if (captureState) {
        delete captureState.fetchVersion;
        delete captureState.webSocketVersion;
        delete captureState.messageVersion;
      }

      delete window[INSTALL_KEY];
      delete window[API_KEY];
    },
    getState() {
      return {
        activeConversationId: state.activeConversationId,
        lastReading: state.lastReading,
        cachedConversationIds: Array.from(state.readingsByConversationId.keys()),
        animatedConversationIds: Array.from(state.lastAnimatedUsedByConversationId.keys()),
        hasAppSignalScope: isAppSignalScope(state.appSignalScope),
        hasAppSignalModules: !!state.appSignalModules,
      };
    },
  };

  installStyle();
  installFetchCapture();
  installWebSocketCapture();
  installPostMessageCapture();
  updateMeter();
  installObserver();
  state.timer = window.setInterval(updateMeter, UPDATE_INTERVAL_MS);
})();

