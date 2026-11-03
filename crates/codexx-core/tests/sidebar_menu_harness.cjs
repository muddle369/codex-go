const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");

const source = fs.readFileSync(process.argv[2], "utf8");
const names = [
  "codexPlusSidebarAnchor",
  "positionCodexPlusSidebarMenu",
  "setCodexPlusTriggerLabel",
  "codexPlusComposerWorkStatus",
  "codexPlusEffectiveStatus",
  "renderCodexPlusStatusLine",
];
const functions = names.map((name) => {
  const start = source.indexOf(`  function ${name}(`);
  assert.notEqual(start, -1, `missing production function: ${name}`);
  const end = source.indexOf("\n  }\n", start);
  assert.notEqual(end, -1);
  return source.slice(start, end + 5);
});

function rectangle(left, top, width, height) {
  return { left, top, width, height, right: left + width, bottom: top + height };
}

const properties = new Map();
const label = { dataset: {}, textContent: "" };
const trigger = {
  attributes: {},
  querySelector: () => label,
  classList: { contains: (name) => name === "codex-plus-sidebar-trigger" },
  setAttribute(name, value) { this.attributes[name] = value; },
};
const line = { dataset: { status: "checking" }, offsetWidth: 24, title: "" };
const menu = {
  hidden: false,
  dataset: {},
  style: {
    setProperty: (name, value) => properties.set(name, value),
    getPropertyValue: (name) => properties.get(name) || "",
  },
  getBoundingClientRect: () => rectangle(0, 0, 36, 44),
};
const upperControls = {
  children: [],
  getBoundingClientRect: () => rectangle(6, 50, 40, 859),
  querySelector: () => ({}),
};
const bottomControls = {
  children: [],
  getBoundingClientRect: () => rectangle(8, 915, 36, 80),
  querySelector: () => ({}),
};
const rail = {
  children: [upperControls, bottomControls],
  getBoundingClientRect: () => rectangle(0, 44, 52, 955),
};
let railVisible = true;
let sidebar = null;
const sendButton = {
  hidden: false,
  disabled: true,
  getAttribute: (name) => name === "aria-label" ? "发送" : null,
  getClientRects: () => [{}],
  matches: () => false,
};
const hiddenStopButton = {
  hidden: false,
  disabled: false,
  getAttribute: (name) => name === "aria-label" ? "停止" : null,
  getClientRects: () => [],
  matches: () => false,
};
const composerButtons = [sendButton];
const footer = {
  querySelectorAll: () => composerButtons,
  querySelector: () => null,
};
const context = vm.createContext({
  HTMLElement: Object,
  getComputedStyle: () => ({ display: "flex", visibility: "visible" }),
  document: {
    querySelector: (selector) => selector === '[data-app-navigation-rail="true"]'
      ? railVisible ? rail : null
      : sidebar,
    getElementById: () => menu,
    querySelectorAll: (selector) => selector === "[data-codex-plus-status-line]" ? [line] : [],
  },
  window: { innerWidth: 1549, innerHeight: 1003, setTimeout: () => 1 },
  codexPlusMenuId: "codex-plus-menu",
  codexPlusSidebarMenuLastPosition: "",
  codexPlusDisplayName: "CodexGO",
  codexPlusVersion: "1.5.0",
  codexPlusBackendStatus: { status: "ok" },
  codexPlusWorkState: "idle",
  codexPlusComposerFooter: () => footer,
});
vm.runInContext(functions.join("\n"), context);

assert.equal(context.codexPlusSidebarAnchor().anchor, bottomControls);
context.positionCodexPlusSidebarMenu();
assert.equal(properties.get("--codex-plus-sidebar-menu-left"), "8px");
assert.equal(properties.get("--codex-plus-sidebar-menu-top"), "864px");
assert.equal(menu.hidden, false);

bottomControls.getBoundingClientRect = () => rectangle(8, 959, 36, 36);
assert.equal(context.codexPlusSidebarAnchor().anchor, bottomControls);
context.positionCodexPlusSidebarMenu();
assert.equal(properties.get("--codex-plus-sidebar-menu-top"), "908px");

bottomControls.getBoundingClientRect = () => rectangle(8, 516, 36, 80);
rail.getBoundingClientRect = () => rectangle(0, 44, 52, 556);
context.window.innerHeight = 604;
context.positionCodexPlusSidebarMenu();
assert.equal(properties.get("--codex-plus-sidebar-menu-top"), "465px");

context.setCodexPlusTriggerLabel(trigger);
assert.equal(label.textContent, "1.5.0");
assert.equal(trigger.attributes["aria-label"], "CodexGO 1.5.0");
context.codexPlusVersion = "1.5.1";
context.setCodexPlusTriggerLabel(trigger);
assert.equal(label.textContent, "1.5.1");
assert.equal(trigger.attributes["aria-label"], "CodexGO 1.5.1");

context.codexPlusWorkState = "working";
context.renderCodexPlusStatusLine();
assert.equal(line.dataset.status, "working");
context.codexPlusWorkState = "idle";
context.renderCodexPlusStatusLine();
assert.equal(line.dataset.status, "idle");
context.codexPlusBackendStatus.status = "failed";
context.renderCodexPlusStatusLine();
assert.equal(line.dataset.status, "failed");
assert.notEqual(line.dataset.completion, "true");

assert.equal(context.codexPlusComposerWorkStatus(), "idle");
composerButtons.push(hiddenStopButton);
assert.equal(context.codexPlusComposerWorkStatus(), "idle");
hiddenStopButton.getClientRects = () => [{}];
assert.equal(context.codexPlusComposerWorkStatus(), "working");

railVisible = false;
context.positionCodexPlusSidebarMenu();
assert.equal(menu.hidden, true);
railVisible = true;
context.positionCodexPlusSidebarMenu();
assert.equal(menu.hidden, false);

railVisible = false;
const settingsButton = {
  getBoundingClientRect: () => rectangle(8, 516, 36, 36),
  getAttribute: () => "打开设置",
};
sidebar = {
  getBoundingClientRect: () => rectangle(0, 44, 52, 556),
  querySelectorAll: () => [settingsButton],
};
assert.equal(context.codexPlusSidebarAnchor().anchor, settingsButton);

console.log("Sidebar positioning, version refresh, and status transitions passed.");
