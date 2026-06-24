/*
@codex-plus-script
name: Codex Context Ring Restore
description: Restore the original official context-usage ring near the composer controls as much as possible, and inject a lightweight fallback ring only when the official ring is unavailable.
version: 0.1.2
author: win9zhx
*/

(() => {
  const INSTALL_KEY = "__codexContextRingRestoreInstalled";
  const STYLE_ID = "codex-context-ring-restore-style";
  const FALLBACK_ATTR = "data-codex-context-ring-restore";
  const PANEL_VERSION = "context-ring-restore-3";
  const CACHE_TTL_MS = 1500;
  const CAPTURE_TEXT_HINT_RE = /context|token|tokens|usage|window|budget|remaining|compress this conversation|压缩此对话的上下文|上下文|令牌|使用|窗口/i;
  const MAX_CAPTURE_TEXT_LENGTH = 800000;

  let cachedContextUsage = { at: 0, value: null };
  const officialMenuUsageByConversationId = new Map();
  const capturedUsageByConversationId = new Map();
  let captureInstalled = false;

  if (window[INSTALL_KEY]) return;
  window[INSTALL_KEY] = true;

  function classNameText(node) {
    return typeof node?.className === "string" ? node.className : "";
  }

  function isReasoningControl(node) {
    if (!(node instanceof Element)) return false;
    return node.matches("[data-codex-intelligence-trigger]") || !!node.querySelector("[data-codex-intelligence-trigger]");
  }

  function isModelControl(node) {
    if (!(node instanceof Element)) return false;
    if (!(node.matches(".h-token-button-composer") || node.querySelector(".h-token-button-composer"))) return false;
    if (node.querySelector("[data-codex-intelligence-trigger]")) return false;
    const label = [node.getAttribute("aria-label") || "", node.getAttribute("title") || ""].join(" ");
    if (/隐藏边栏|显示边栏|hide sidebar|show sidebar/i.test(label)) return false;
    return true;
  }

  function directChildContaining(parent, child) {
    if (!(parent instanceof Element) || !(child instanceof Element)) return null;
    return Array.from(parent.children).find((node) => node instanceof Element && (node === child || node.contains(child))) || null;
  }

  function findStructuralContextGroup(footer) {
    if (!(footer instanceof Element)) return null;
    const triggers = Array.from(footer.querySelectorAll("[data-codex-intelligence-trigger]"));
    for (const trigger of triggers) {
      let node = trigger.parentElement;
      while (node && node !== footer) {
        const className = classNameText(node);
        if (className.includes("items-center") && node.querySelector(".h-token-button-composer")) {
          const reasoningItem = directChildContaining(node, trigger);
          const children = Array.from(node.children);
          const modelItem = children.slice(0, Math.max(0, children.indexOf(reasoningItem))).reverse().find(isModelControl) || null;
          if (modelItem && reasoningItem) return { group: node, modelItem, reasoningItem };
        }
        node = node.parentElement;
      }
    }
    return null;
  }

  function findInlineContextGroup() {
    const footer = document.querySelector(".composer-footer");
    if (!(footer instanceof Element)) return null;
    const structural = findStructuralContextGroup(footer);
    if (structural) return structural.group;
    const groups = Array.from(footer.querySelectorAll("div")).filter((node) => {
      const className = classNameText(node);
      if (!className.includes("items-center")) return false;
      if (!node.querySelector(".h-token-button-composer")) return false;
      return !!node.querySelector("button, [role='button'], [aria-haspopup='menu']");
    });
    return groups.find((node) => Array.from(node.children).some(isReasoningControl) && Array.from(node.children).some(isModelControl)) || null;
  }

  function findContextModelItem(group) {
    if (!(group instanceof Element)) return null;
    const structural = findStructuralContextGroup(document.querySelector(".composer-footer"));
    if (structural?.group === group) return structural.modelItem;
    return Array.from(group.children).find(isModelControl) || null;
  }

  function findContextReasoningItem(group) {
    if (!(group instanceof Element)) return null;
    const structural = findStructuralContextGroup(document.querySelector(".composer-footer"));
    if (structural?.group === group) return structural.reasoningItem;
    return Array.from(group.children).find((node) => node instanceof Element && isReasoningControl(node)) || null;
  }

  function normalizeSvgPathData(value) {
    return String(value || "").replace(/[\s,]+/g, "").trim().toLowerCase();
  }

  function isOfficialContextRingVisual(node) {
    if (!(node instanceof Element)) return false;
    const className = classNameText(node);
    if (!className.includes("size-token-button-composer")) return false;
    if (!className.includes("items-center")) return false;
    if (!className.includes("justify-center")) return false;
    if (!className.includes("text-token-description-foreground")) return false;

    const svg = node.querySelector("svg[aria-hidden='true']");
    if (!svg) return false;
    const circles = Array.from(svg.querySelectorAll("circle"));
    if (circles.length !== 2) return false;
    if (!circles.some((circle) => circle.hasAttribute("stroke-dasharray"))) return false;
    if (!circles.some((circle) => circle.hasAttribute("stroke-dashoffset"))) return false;

    const paths = Array.from(svg.querySelectorAll("path"));
    if (!paths.length) return true;
    const pathData = paths.map((path) => normalizeSvgPathData(path.getAttribute("d"))).join(" ");
    return !pathData || (pathData.includes("a") && pathData.includes("0"));
  }

  function looksLikeContextRing(node) {
    if (!(node instanceof Element)) return false;
    if (node.closest("button[type='submit'], button[aria-label*='send' i], button[aria-label*='发送'], [data-testid*='send' i]")) return false;
    const visual = node.matches(".size-token-button-composer, [class*='size-token-button-composer']")
      ? node
      : node.querySelector(".size-token-button-composer, [class*='size-token-button-composer']");
    if (!(visual instanceof Element)) return false;
    const text = (node.textContent || "").trim();
    if (text.length > 12) return false;
    return isOfficialContextRingVisual(visual);
  }

  function findContextRingHost(scope) {
    const footer = document.querySelector(".composer-footer");
    const root = scope instanceof Element ? scope : footer || document;
    const candidates = Array.from(root.querySelectorAll(".size-token-button-composer, [class*='size-token-button-composer']"));
    const ring = candidates.find((candidate) => looksLikeContextRing(candidate));
    if (!(ring instanceof Element)) return null;
    return ring.closest("span.flex.items-center, span") || ring;
  }

  function getOwnKeyByPrefix(target, prefix) {
    if (!target || (typeof target !== "object" && typeof target !== "function")) return null;
    return Object.keys(target).find((key) => key.startsWith(prefix)) || null;
  }

  function getReactFiber(target) {
    const fiberKey = getOwnKeyByPrefix(target, "__reactFiber$");
    if (fiberKey) return target[fiberKey];
    const containerKey = getOwnKeyByPrefix(target, "__reactContainer$");
    return containerKey ? target[containerKey] : null;
  }

  function getReactProps(target) {
    const propsKey = getOwnKeyByPrefix(target, "__reactProps$");
    return propsKey ? target[propsKey] : null;
  }

  function enqueueGraphValue(queue, seen, value, depth) {
    if (!value || (typeof value !== "object" && typeof value !== "function")) return;
    if (seen.has(value)) return;
    seen.add(value);
    queue.push({ value, depth });
  }

  function collectSearchRoots() {
    const seeds = [];
    const seenNodes = new Set();

    function addNode(node) {
      if (!(node instanceof Node) || seenNodes.has(node)) return;
      seenNodes.add(node);
      seeds.push(node);
    }

    const footer = document.querySelector(".composer-footer");
    const editor = document.querySelector(".ProseMirror");

    [
      document.activeElement,
      editor,
      editor?.parentElement,
      footer,
      footer?.parentElement,
      document.querySelector(".size-token-button-composer"),
      document.querySelector("[data-codex-intelligence-trigger]"),
      document.body,
    ].forEach(addNode);

    for (const start of Array.from(seenNodes)) {
      let node = start;
      let hops = 0;
      while (node && hops < 6) {
        addNode(node);
        node = node.parentNode || (node instanceof ShadowRoot ? node.host : null);
        hops += 1;
      }
    }

    return seeds.flatMap((node) => [node, getReactFiber(node), getReactProps(node), node.pmViewDesc]).filter(Boolean);
  }

  function searchObjectGraph(roots, matcher, options = {}) {
    const maxNodes = options.maxNodes ?? 9000;
    const maxDepth = options.maxDepth ?? 10;
    const seen = new WeakSet();
    const queue = [];

    roots.forEach((root) => enqueueGraphValue(queue, seen, root, 0));

    let visited = 0;
    while (queue.length && visited < maxNodes) {
      const { value, depth } = queue.shift();
      visited += 1;

      try {
        if (matcher(value)) return value;
      } catch (_) {
        // Ignore probing errors from host objects.
      }

      if (depth >= maxDepth) continue;

      if (value instanceof Node) {
        enqueueGraphValue(queue, seen, getReactFiber(value), depth + 1);
        enqueueGraphValue(queue, seen, getReactProps(value), depth + 1);
        enqueueGraphValue(queue, seen, value.pmViewDesc, depth + 1);
        continue;
      }

      if (Array.isArray(value)) {
        value.slice(0, 50).forEach((item) => enqueueGraphValue(queue, seen, item, depth + 1));
        continue;
      }

      if (value instanceof Map) {
        Array.from(value.values()).slice(0, 50).forEach((item) => enqueueGraphValue(queue, seen, item, depth + 1));
        continue;
      }

      if (value instanceof Set) {
        Array.from(value.values()).slice(0, 50).forEach((item) => enqueueGraphValue(queue, seen, item, depth + 1));
        continue;
      }

      for (const key of Object.keys(value).slice(0, 80)) {
        let nextValue;
        try {
          nextValue = value[key];
        } catch (_) {
          continue;
        }
        enqueueGraphValue(queue, seen, nextValue, depth + 1);
      }
    }

    return null;
  }

  function findReactBackedValue(matcher) {
    return searchObjectGraph(collectSearchRoots(), matcher);
  }

  function firstFiniteNumber(...values) {
    for (const value of values) {
      const number = Number(value);
      if (Number.isFinite(number)) return number;
    }
    return null;
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

  function getElementConversationId(element) {
    for (let node = element; node && node.nodeType === Node.ELEMENT_NODE; node = node.parentElement) {
      const attrValue =
        node.getAttribute("data-app-action-sidebar-thread-id") ||
        node.getAttribute("data-thread-id") ||
        node.getAttribute("data-conversation-id");
      const normalized = normalizeConversationId(attrValue);
      if (normalized) return normalized;
    }
    return null;
  }

  function readActiveConversationId() {
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
      if (conversationId) return conversationId;
    }

    const threadSurface = document.querySelector("[data-thread-id], [data-conversation-id], [data-app-action-sidebar-thread-id]");
    return getElementConversationId(threadSurface);
  }

  function parseContextUsageShape(value) {
    if (!value || typeof value !== "object") return null;

    const modelContextWindow = firstFiniteNumber(
      value.model_context_window,
      value.modelContextWindow,
      value.context_window,
      value.contextWindow,
      value.window_tokens,
      value.windowTokens,
    );

    const lastUsage =
      value.last_token_usage ||
      value.lastTokenUsage ||
      value.last_usage ||
      value.lastUsage ||
      value.last;

    const totalTokens = firstFiniteNumber(
      lastUsage && lastUsage.total_tokens,
      lastUsage && lastUsage.totalTokens,
      value.total_tokens,
      value.totalTokens,
      value.tokens_used,
      value.tokensUsed,
      value.used_tokens,
      value.usedTokens,
    );

    if (!Number.isFinite(modelContextWindow) || modelContextWindow <= 0) return null;
    if (!Number.isFinite(totalTokens) || totalTokens < 0) return null;

    return {
      modelContextWindow,
      totalTokens,
    };
  }

  function makeUsageReading(percent, usedTokens, contextWindow, exact = true) {
    if (!Number.isFinite(percent)) return null;
    const safePercent = Math.max(0, Math.min(100, Number(percent)));
    const roundedPercent = Math.max(0, Math.min(100, Math.round(safePercent)));
    const hasRatio = Number.isFinite(usedTokens) && Number.isFinite(contextWindow) && contextWindow > 0;
    const normalizedUsedTokens = hasRatio ? Math.min(Number(usedTokens), Number(contextWindow)) : null;
    const normalizedContextWindow = hasRatio ? Number(contextWindow) : null;
    const remainingTokens = hasRatio ? Math.max(normalizedContextWindow - normalizedUsedTokens, 0) : null;

    return {
      exact,
      percent: safePercent,
      usedTokens: normalizedUsedTokens,
      contextWindow: normalizedContextWindow,
      remainingTokens,
      summary: `已使用 ${roundedPercent}%`,
      label: hasRatio
        ? `${formatTokenCount(normalizedUsedTokens)} / ${formatTokenCount(normalizedContextWindow)}`
        : `${roundedPercent}%`,
      detail: hasRatio
        ? `已用 ${formatTokenCount(normalizedUsedTokens)} / ${formatTokenCount(normalizedContextWindow)} tokens（${roundedPercent}%），剩余 ${formatTokenCount(remainingTokens)}。`
        : `已使用 ${roundedPercent}%。`,
    };
  }

  function isContextUsageInfo(value) {
    return !!parseContextUsageShape(value);
  }

  function formatTokenCount(value) {
    return Math.round(value).toLocaleString("en-US");
  }

  function buildExactContextUsage(info) {
    const parsed = parseContextUsageShape(info);
    if (!parsed) return null;
    const contextWindow = Number(parsed.modelContextWindow);
    const usedTokens = Math.min(Number(parsed.totalTokens), contextWindow);
    const remainingTokens = Math.max(contextWindow - usedTokens, 0);
    const percent = (usedTokens / contextWindow) * 100;
    if (!Number.isFinite(percent)) return null;
    return makeUsageReading(percent, usedTokens, contextWindow, true) || {
      exact: true,
      percent,
      usedTokens,
      contextWindow,
      remainingTokens,
    };
  }

  function buildExactContextUsageFromCandidate(value) {
    if (!value || typeof value !== "object") return null;

    const directReading = buildExactContextUsage(value);
    if (directReading) return directReading;

    if (
      value.method === "thread/tokenUsage/updated" ||
      value.type === "thread/tokenUsage/updated" ||
      value.event === "thread/tokenUsage/updated"
    ) {
      const paramsReading =
        buildExactContextUsage(value.params && value.params.tokenUsage) ||
        buildExactContextUsage(value.params);
      if (paramsReading) return paramsReading;
    }

    if (value.type === "token_count" || value.event === "token_count") {
      const infoReading = buildExactContextUsage(value.info);
      if (infoReading) return infoReading;
    }

    if (value.payload && (value.payload.type === "token_count" || value.payload.event === "token_count")) {
      const payloadReading = buildExactContextUsage(value.payload.info);
      if (payloadReading) return payloadReading;
    }

    const nestedReading = buildExactContextUsage(
      value.contextUsage || value.context_usage || value.tokenUsage || value.token_usage || value.usage,
    );
    if (nestedReading) return nestedReading;

    const infoReading = buildExactContextUsage(value.info);
    if (infoReading) return infoReading;

    return null;
  }

  function findExactContextUsage() {
    const exactCandidate = findReactBackedValue(isContextUsageInfo);
    if (exactCandidate) {
      return buildExactContextUsage(exactCandidate);
    }

    const candidate = findReactBackedValue((value) => !!buildExactContextUsageFromCandidate(value));
    return candidate ? buildExactContextUsageFromCandidate(candidate) : null;
  }

  function rememberCapturedUsage(usage, conversationId) {
    if (!usage) return null;
    const activeConversationId = normalizeConversationId(conversationId) || readActiveConversationId();
    if (activeConversationId) {
      capturedUsageByConversationId.set(activeConversationId, usage);
    }
    return usage;
  }

  function getRememberedCapturedUsage(conversationId) {
    const activeConversationId = normalizeConversationId(conversationId) || readActiveConversationId();
    return activeConversationId ? capturedUsageByConversationId.get(activeConversationId) || null : null;
  }

  function rememberOfficialMenuUsage(usage, conversationId) {
    if (!usage) return null;
    const activeConversationId = normalizeConversationId(conversationId) || readActiveConversationId();
    if (activeConversationId) {
      officialMenuUsageByConversationId.set(activeConversationId, usage);
    }
    return usage;
  }

  function getRememberedOfficialMenuUsage(conversationId) {
    const activeConversationId = normalizeConversationId(conversationId) || readActiveConversationId();
    return activeConversationId ? officialMenuUsageByConversationId.get(activeConversationId) || null : null;
  }

  function parseStructuredText(text) {
    if (!text || !CAPTURE_TEXT_HINT_RE.test(text)) return null;

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
        return makeUsageReading(percent, null, null, true);
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
        return makeUsageReading((used / limit) * 100, used, limit, true);
      }
    }

    return null;
  }

  function parsePayloadText(text) {
    const clipped = String(text || "").slice(0, MAX_CAPTURE_TEXT_LENGTH);
    if (!CAPTURE_TEXT_HINT_RE.test(clipped)) return null;

    try {
      const parsed = JSON.parse(clipped);
      return buildExactContextUsageFromCandidate(parsed) || parseStructuredText(clipped);
    } catch (_) {
      return parseStructuredText(clipped);
    }
  }

  function inspectCandidateText(text, conversationId) {
    if (!text || text.length > MAX_CAPTURE_TEXT_LENGTH) return null;
    if (!CAPTURE_TEXT_HINT_RE.test(text)) return null;
    return rememberCapturedUsage(parsePayloadText(text), conversationId);
  }

  function inspectCandidateValue(value, conversationId) {
    if (!value || typeof value !== "object") return null;
    return rememberCapturedUsage(buildExactContextUsageFromCandidate(value), conversationId);
  }

  function installFetchCapture() {
    const patchedFlag = "__codexContextRingRestoreFetchPatched";
    if (window[patchedFlag] || typeof window.fetch !== "function") return;

    const nativeFetch = window.fetch.bind(window);
    window.fetch = function codexContextRingRestoreFetch(...args) {
      return nativeFetch(...args).then((response) => {
        try {
          const urlText = String(args[0] && (args[0].url || args[0].href || args[0]) || response.url || "");
          if (urlText && !CAPTURE_TEXT_HINT_RE.test(urlText)) return response;

          const contentType = response.headers && response.headers.get("content-type");
          const contentLength = response.headers && Number(response.headers.get("content-length"));
          const isTextLike = !contentType || /json|text|event-stream|x-ndjson/i.test(contentType);
          if (isTextLike && (!Number.isFinite(contentLength) || contentLength <= MAX_CAPTURE_TEXT_LENGTH)) {
            response.clone().text().then((text) => {
              inspectCandidateText(text, readActiveConversationId());
              schedule();
            }).catch(() => {});
          }
        } catch (_) {
          return response;
        }
        return response;
      });
    };

    window[patchedFlag] = true;
  }

  function installWebSocketCapture() {
    const patchedFlag = "__codexContextRingRestoreWebSocketPatched";
    if (window[patchedFlag] || typeof window.WebSocket !== "function") return;

    const NativeWebSocket = window.WebSocket;
    function ContextRingRestoreWebSocket(...args) {
      const socket = new NativeWebSocket(...args);
      socket.addEventListener("message", (event) => {
        try {
          if (typeof event.data === "string") {
            inspectCandidateText(event.data, readActiveConversationId());
          } else if (event.data instanceof Blob && event.data.size <= MAX_CAPTURE_TEXT_LENGTH) {
            event.data.text().then((text) => {
              inspectCandidateText(text, readActiveConversationId());
              schedule();
            }).catch(() => {});
            return;
          }
          schedule();
        } catch (_) {
          return;
        }
      });
      return socket;
    }

    ContextRingRestoreWebSocket.prototype = NativeWebSocket.prototype;
    ContextRingRestoreWebSocket.CONNECTING = NativeWebSocket.CONNECTING;
    ContextRingRestoreWebSocket.OPEN = NativeWebSocket.OPEN;
    ContextRingRestoreWebSocket.CLOSING = NativeWebSocket.CLOSING;
    ContextRingRestoreWebSocket.CLOSED = NativeWebSocket.CLOSED;
    window.WebSocket = ContextRingRestoreWebSocket;
    window[patchedFlag] = true;
  }

  function installPostMessageCapture() {
    const listenerKey = "__codexContextRingRestorePostMessageListener";
    if (window[listenerKey]) return;

    const listener = (event) => {
      try {
        if (typeof event.data === "string") {
          inspectCandidateText(event.data, readActiveConversationId());
        } else {
          inspectCandidateValue(event.data, readActiveConversationId());
        }
        schedule();
      } catch (_) {
        return;
      }
    };

    window.addEventListener("message", listener, true);
    window[listenerKey] = listener;
  }

  function installCaptureHooks() {
    if (captureInstalled) return;
    captureInstalled = true;
    installFetchCapture();
    installWebSocketCapture();
    installPostMessageCapture();
  }

  function findOfficialMenuContextUsage() {
    const items = Array.from(document.querySelectorAll("button, [role='button'], [cmdk-item], [data-command], li, div"));
    for (const item of items) {
      if (!(item instanceof Element)) continue;
      const text = item.textContent?.replace(/\s+/g, " ").trim() || "";
      if (!text) continue;
      if (!/压缩此对话的上下文|compress this conversation/i.test(text)) continue;
      const percentMatch = /已使用\s*(\d{1,3}(?:\.\d+)?)%|used\s*(\d{1,3}(?:\.\d+)?)%/i.exec(text);
      if (!percentMatch) continue;
      const percent = firstFiniteNumber(percentMatch[1], percentMatch[2]);
      if (!Number.isFinite(percent)) continue;
      return makeUsageReading(percent, null, null, true);
    }
    return null;
  }

  function getContextUsage() {
    const now = Date.now();
    if (cachedContextUsage.value && now - cachedContextUsage.at < CACHE_TTL_MS) {
      return cachedContextUsage.value;
    }
    installCaptureHooks();
    const activeConversationId = readActiveConversationId();
    const runtimeUsage =
      rememberCapturedUsage(findExactContextUsage(), activeConversationId) ||
      getRememberedCapturedUsage(activeConversationId);
    const officialMenuUsage =
      rememberOfficialMenuUsage(findOfficialMenuContextUsage(), activeConversationId) ||
      getRememberedOfficialMenuUsage(activeConversationId);
    const usage = runtimeUsage || officialMenuUsage || approximateContextUsage();
    cachedContextUsage = { at: now, value: usage };
    return usage;
  }

  function stabilizeInlineContextRing() {
    const group = findInlineContextGroup();
    const host = findContextRingHost(document);
    if (!(group instanceof Element) || !(host instanceof Element)) return false;
    if (!looksLikeContextRing(host)) return false;
    host.hidden = false;
    host.removeAttribute("hidden");
    host.style.removeProperty("display");
    host.style.removeProperty("visibility");
    host.style.removeProperty("opacity");
    if (group.contains(host)) return true;
    const modelItem = findContextModelItem(group);
    if (!modelItem) return false;
    group.insertBefore(host, modelItem.nextSibling);
    return true;
  }

  function cleanupBrokenContextRingSlot() {
    document.querySelectorAll(`[${FALLBACK_ATTR}='slot']`).forEach((node) => node.remove());
  }

  function contextColor(percent) {
    if (percent >= 85) return "#e25555";
    if (percent >= 65) return "#d98f28";
    return "#339cff";
  }

  function updateFallbackContextUsageRing(button, usage) {
    const percent = Math.max(0, Math.min(100, Number(usage?.percent || 0)));
    button.style.setProperty("--codex-context-offset", String(100 - percent));
    button.style.setProperty("--codex-context-color", contextColor(percent));
    const summary = usage?.summary || `已使用 ${percent}%`;
    const detail = usage?.detail || "上下文使用情况为粗略估算。";
    button.dataset.exact = usage?.exact ? "1" : "0";
    button.setAttribute("aria-label", `上下文使用情况：${summary}`);
    button.setAttribute("title", `${summary}\n${detail}`);
    const label = button.querySelector(`[${FALLBACK_ATTR}='label']`);
    if (label) {
      label.textContent = usage?.label || `${Math.round(percent)}%`;
    }
  }

  function approximateContextUsage() {
    const text = document.body?.innerText || "";
    const normalized = text.replace(/\s+/g, " ").trim();
    const size = normalized.length;
    const estimated = Math.min(92, Math.max(4, Math.round(size / 180)));
    const inferredWindow = Math.max(1, size * 2);
    const inferredUsed = Math.max(1, Math.round((estimated / 100) * inferredWindow));
    return {
      exact: false,
      percent: estimated,
      summary: `已使用约 ${estimated}%`,
      label: `${formatTokenCount(inferredUsed)} / ${formatTokenCount(inferredWindow)}`,
      detail: `已用约 ${formatTokenCount(inferredUsed)} / ${formatTokenCount(inferredWindow)} tokens（${estimated}%）。`,
    };
  }

  function ensureFallbackContextUsageRing() {
    const group = findInlineContextGroup();
    if (!(group instanceof Element)) return false;
    if (findContextRingHost(group)) return false;
    const modelItem = findContextModelItem(group);
    if (!modelItem) return false;
    let container = group.querySelector(`[${FALLBACK_ATTR}='container']`);
    if (!(container instanceof HTMLSpanElement)) {
      container = document.createElement("span");
      container.className = "codex-context-ring-restore-container";
      container.setAttribute(FALLBACK_ATTR, "container");
      container.innerHTML = `
        <span class="codex-context-ring-restore-button" ${FALLBACK_ATTR}="button" aria-hidden="true">
          <svg class="codex-context-ring-restore-svg" viewBox="0 0 36 36" aria-hidden="true">
            <circle class="codex-context-ring-restore-track" cx="18" cy="18" r="15.5"></circle>
            <circle class="codex-context-ring-restore-progress" cx="18" cy="18" r="15.5" pathLength="100" transform="rotate(-90 18 18)"></circle>
          </svg>
        </span>
        <span class="codex-context-ring-restore-label" ${FALLBACK_ATTR}="label"></span>
      `;
    }
    const button = container.querySelector(`[${FALLBACK_ATTR}='button']`);
    if (!(button instanceof HTMLElement)) return false;
    const reasoningItem = findContextReasoningItem(group);
    if (reasoningItem) group.insertBefore(container, reasoningItem);
    else group.insertBefore(container, modelItem.nextSibling);
    updateFallbackContextUsageRing(button, getContextUsage());
    return true;
  }

  function repairContextRingLayout() {
    cleanupBrokenContextRingSlot();
    if (stabilizeInlineContextRing()) return;
    ensureFallbackContextUsageRing();
  }

  function installStyle() {
    const existing = document.getElementById(STYLE_ID);
    if (existing?.dataset.version === PANEL_VERSION) return;
    existing?.remove();

    const style = document.createElement("style");
    style.id = STYLE_ID;
    style.dataset.version = PANEL_VERSION;
    style.textContent = `
      .codex-context-ring-restore-container {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        min-width: 0;
        flex: 0 0 auto;
        color: #b8c0cc;
      }
      .codex-context-ring-restore-button {
        position: relative;
        display: inline-flex;
        width: 28px;
        height: 28px;
        min-width: 28px;
        align-items: center;
        justify-content: center;
        border-radius: 999px;
        background: transparent;
        padding: 0;
        flex: 0 0 28px;
      }
      .codex-context-ring-restore-svg {
        width: 20px;
        height: 20px;
        transform: translateZ(0);
      }
      .codex-context-ring-restore-label {
        max-width: 112px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-size: 12px;
        line-height: 1;
        color: #b8c0cc;
        font-variant-numeric: tabular-nums;
      }
      .codex-context-ring-restore-track {
        fill: none;
        stroke: rgba(184,192,204,.24);
        stroke-width: 4;
      }
      .codex-context-ring-restore-progress {
        fill: none;
        stroke: var(--codex-context-color, #339cff);
        stroke-width: 4;
        stroke-linecap: round;
        stroke-dasharray: 100;
        stroke-dashoffset: var(--codex-context-offset, 100);
        transition: stroke-dashoffset .18s ease, stroke .18s ease;
      }
    `;
    document.documentElement.appendChild(style);
  }

  function scan() {
    installStyle();
    requestAnimationFrame(() => {
      repairContextRingLayout();
    });
  }

  function schedule() {
    if (window.__codexContextRingRestoreQueued) return;
    window.__codexContextRingRestoreQueued = true;
    requestAnimationFrame(() => {
      window.__codexContextRingRestoreQueued = false;
      scan();
    });
  }

  schedule();
  window.__codexContextRingRestoreObserver?.disconnect?.();
  window.__codexContextRingRestoreObserver = new MutationObserver(schedule);
  window.__codexContextRingRestoreObserver.observe(document.documentElement, { childList: true, subtree: true });
})();
