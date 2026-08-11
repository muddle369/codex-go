(() => {
  const route = new URL(window.location.href).searchParams.get("initialRoute");
  if (route !== "/global-dictation") return;

  const patchVersion = "20260811-audio-helper-v2";
  const endpoint = __CODEXGO_AUDIO_TRANSCRIPTION_ENDPOINT__;
  const maxAttempts = 40;
  const retryDelayMs = 500;

  function appAssetUrl() {
    const urls = [
      ...Array.from(document.scripts || []).map((script) => script.src),
      ...Array.from(document.querySelectorAll("link[href]") || []).map((link) => link.href),
      ...performance.getEntriesByType("resource").map((entry) => entry.name),
    ].filter(Boolean);
    return urls.find((url) =>
      url.includes("/assets/app-initial-") && url.split("?")[0].endsWith(".js")
    ) || "";
  }

  function transcriptionHeaders(headers) {
    const next = { ...(headers || {}) };
    const base64Header = Object.keys(next).find((name) => name.toLowerCase() === "x-codex-base64");
    if (base64Header && base64Header !== "x-codex-base64") {
      next["x-codex-base64"] = next[base64Header];
      delete next[base64Header];
    }
    for (const name of Object.keys(next)) {
      const normalized = name.toLowerCase();
      if (normalized === "x-openai-attach-auth" || normalized === "x-openai-attach-integrity-state") {
        delete next[name];
      }
    }
    const language = String(document.documentElement.lang || navigator.language || "")
      .trim()
      .split(/[-_]/)[0]
      .toLowerCase();
    const languageHeader = Object.keys(next).find(
      (name) => name.toLowerCase() === "x-codexgo-audio-language"
    );
    if (language && !languageHeader) {
      next["x-codexgo-audio-language"] = language;
    }
    return next;
  }

  function patchClient(client) {
    if (!client || typeof client.post !== "function") return false;
    const prototype = Object.getPrototypeOf(client);
    if (!prototype || typeof prototype.post !== "function") return false;
    if (prototype.__codexGoAudioRedirectVersion === patchVersion) return true;
    const originalPost = prototype.__codexGoAudioOriginalPost || prototype.post;
    prototype.__codexGoAudioOriginalPost = originalPost;
    prototype.post = function codexGoAudioRedirectPost(url, body, headers, signal) {
      if (url === "/transcribe") {
        return originalPost.call(this, endpoint, body, transcriptionHeaders(headers), signal);
      }
      return originalPost.call(this, url, body, headers, signal);
    };
    prototype.__codexGoAudioRedirectVersion = patchVersion;
    return true;
  }

  if (window.__codexGoAudioRedirectScheduled === patchVersion) return;
  window.__codexGoAudioRedirectScheduled = patchVersion;
  let attempt = 0;
  const install = async () => {
    attempt += 1;
    try {
      const assetUrl = appAssetUrl();
      if (!assetUrl) throw new Error("Codex app-initial asset is not ready");
      const module = await import(assetUrl);
      const clients = new Set();
      for (const candidate of Object.values(module || {})) {
        if (candidate && typeof candidate.post === "function") clients.add(candidate);
        if (candidate && typeof candidate.getInstance === "function") {
          try {
            const instance = candidate.getInstance();
            if (instance && typeof instance.post === "function") clients.add(instance);
          } catch {
          }
        }
      }
      let patchedCount = 0;
      for (const client of clients) {
        if (patchClient(client)) patchedCount += 1;
      }
      if (patchedCount > 0) {
        window.__codexGoAudioRedirectInstalled = patchVersion;
        window.__codexGoAudioRedirectScheduled = "";
        return;
      }
    } catch {
    }
    if (attempt < maxAttempts) {
      window.setTimeout(() => void install(), retryDelayMs);
    } else {
      window.__codexGoAudioRedirectScheduled = "";
    }
  };
  void install();
})();
