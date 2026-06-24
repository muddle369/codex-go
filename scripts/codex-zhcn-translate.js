// ==UserScript==
// @name         Codex简体中文汉化
// @namespace    http://tampermonkey.net/
// @version      1.0
// @description  Codex客户端全界面简体汉化补丁
// @author       BigPizzaV3
// @match        app://openai-codex/*
// @grant        none
// ==/UserScript==
(function(){const e=document.createObserver?new MutationObserver(()=>{let e=document.querySelectorAll("button,label,h1,h2,h3,h4,span,div[role]");e.forEach(t=>{let n=t.innerText||t.textContent;if(!n)return;n=n.replace("Settings","设置").replace("New chat","新建对话").replace("Delete","删除").replace("Export","导出").replace("Save","保存").replace("Cancel","取消").replace("Model","模型").replace("API Key","密钥").replace("Add","添加").replace("Remove","移除");t.innerText=n})}):null;e.observe(document.body,{childList:!0,subtree:!0})})();
