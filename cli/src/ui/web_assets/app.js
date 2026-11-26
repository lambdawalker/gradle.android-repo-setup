"use strict";

// This page renders the same setup questions as the terminal. Credentials and
// the browser session stay in memory; only the chosen color theme is stored.
const $ = id => document.getElementById(id);
const el = (tag, text = "", className = "") => {
  const node = document.createElement(tag);
  node.textContent = text;
  if (className) node.className = className;
  return node;
};
const theme = $("theme");
const savedTheme = sessionStorage.getItem("android-repo-setup-theme");
theme.value = savedTheme === "light" || savedTheme === "dark" ? savedTheme : "system";
function applyTheme() {
  if (theme.value === "system") {
    delete document.documentElement.dataset.theme;
    sessionStorage.removeItem("android-repo-setup-theme");
  } else {
    document.documentElement.dataset.theme = theme.value;
    sessionStorage.setItem("android-repo-setup-theme", theme.value);
  }
}
theme.addEventListener("change", applyTheme);
applyTheme();

let session = "";
let csrf = "";
// sessionStorage is isolated by origin (including this random loopback port)
// and by browser tab. Reloads can resume without reusing the launch token.
const sessionKey = "android-repo-setup-session-v1";
const pausedPromptKey = "android-repo-setup-paused-prompt-v1";
function savedPausedPrompt() {
  try {
    const value = Number(sessionStorage.getItem(pausedPromptKey));
    return Number.isSafeInteger(value) && value > 0 ? value : null;
  } catch { return null; }
}
let pausedPromptId = savedPausedPrompt();
let requireResumeOnRestore = false;
function rememberPausedPrompt(id) {
  if (!Number.isSafeInteger(id) || id <= 0) return;
  pausedPromptId = id;
  try { sessionStorage.setItem(pausedPromptKey, String(id)); } catch { /* Keep this tab's state in memory. */ }
}
function forgetPausedPrompt() {
  pausedPromptId = null;
  try { sessionStorage.removeItem(pausedPromptKey); } catch { /* Storage may be disabled. */ }
}
function savedSession() {
  try {
    const value = JSON.parse(sessionStorage.getItem(sessionKey) || "null");
    if (/^[a-f0-9]{64}$/i.test(value?.session || "") && /^[a-f0-9]{64}$/i.test(value?.csrf || "")) return value;
  } catch { /* Storage can be disabled or contain an obsolete record. */ }
  return null;
}
function rememberSession() {
  try { sessionStorage.setItem(sessionKey, JSON.stringify({session, csrf})); } catch { /* This tab can continue in memory. */ }
}
function forgetSession() {
  try { sessionStorage.removeItem(sessionKey); } catch { /* Storage may be disabled. */ }
  forgetPausedPrompt();
}
let state = null;
let lastRevision = -1;
let promptId;
let lastStatus;
let keySignature = "";
let selectedDetail = null;
let pending = false;
let closed = false;
let closingRequested = false;
let activeTab = "actions";
let timer = null;
let deadline = 0;
let paused = false;
let artifactSignature = "";

const tabs = [...document.querySelectorAll('.tab[role="tab"]')];
function activateTab(tab, focus = false) {
  activeTab = tab.id.slice("tab-".length);
  for (const other of tabs) {
    const selected = other === tab;
    other.setAttribute("aria-selected", String(selected));
    other.tabIndex = selected ? 0 : -1;
    $(other.getAttribute("aria-controls")).hidden = !selected;
  }
  if (focus) tab.focus();
}
for (const tab of tabs) tab.addEventListener("click", () => activateTab(tab));
$("tab-actions").parentElement.addEventListener("keydown", event => {
  const selected = tabs.indexOf(document.activeElement);
  if (selected < 0) return;
  if (event.altKey && event.shiftKey && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
    event.preventDefault();
    const target = tabs[selected + (event.key === "ArrowLeft" ? -1 : 1)];
    if (target) {
      tablist.insertBefore(tabs[selected], event.key === "ArrowLeft" ? target : target.nextSibling);
      tabs.splice(selected, 1);
      tabs.splice(tabs.indexOf(target) + (event.key === "ArrowLeft" ? 0 : 1), 0, document.activeElement);
    }
    return;
  }
  const next = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 :
    event.key === "ArrowLeft" ? (selected + tabs.length - 1) % tabs.length :
    event.key === "ArrowRight" ? (selected + 1) % tabs.length : -1;
  if (next >= 0) { event.preventDefault(); activateTab(tabs[next], true); }
});
const tablist = $("tab-actions").parentElement;
let dragTab;
tablist.addEventListener("pointerdown", event => {
  const tab = event.target.closest(".tab");
  if (tab && event.button === 0) dragTab = {tab, x: event.clientX, y: event.clientY, id: event.pointerId};
});
window.addEventListener("pointermove", event => {
  if (!dragTab || dragTab.id !== event.pointerId || Math.hypot(event.clientX - dragTab.x, event.clientY - dragTab.y) < 6) return;
  dragTab.tab.classList.add("dragging");
  const before = [...tablist.querySelectorAll(".tab")].filter(tab => tab !== dragTab.tab)
    .find(tab => event.clientX < tab.getBoundingClientRect().left + tab.getBoundingClientRect().width / 2);
  tablist.insertBefore(dragTab.tab, before || null);
  event.preventDefault();
});
window.addEventListener("pointerup", () => {
  if (!dragTab) return;
  dragTab.tab.classList.remove("dragging");
  dragTab = null;
  tabs.splice(0, tabs.length, ...tablist.querySelectorAll(".tab"));
});

function setProblem(message) {
  $("footer-status").textContent = message;
  let problem = $("prompt-error");
  if (problem) {
    problem.textContent = message;
    problem.hidden = !message;
  }
}
function showNotice(title, message) {
  if (locked) return;
  $("notice-title").textContent = title;
  $("notice-body").textContent = message;
  $("notice-dialog").showModal();
}
let locked = false;
let unlockInput;
function showLock() {
  if (locked) return;
  locked = true;
  pauseCountdown();
  for (const input of $("workspace").querySelectorAll('input[type="password"]')) input.value = "";
  const dialog = $("notice-dialog");
  if (dialog.open) dialog.close();
  $("notice-title").textContent = "Wizard locked";
  const body = $("notice-body");
  body.replaceChildren(el("p", "Enter the local vault passphrase or the recovery code printed in the launching terminal to continue."));
  const form = field("Unlock credential", "", true);
  unlockInput = form.input;
  unlockInput.addEventListener("keydown", event => {
    if (event.key === "Enter") { event.preventDefault(); unlockWizard(); }
  });
  body.append(form.wrapper);
  const problem = el("p", "", "problem");
  problem.id = "unlock-error";
  problem.setAttribute("role", "alert");
  problem.hidden = true;
  body.append(problem);
  $("notice-close").textContent = "Unlock";
  dialog.showModal();
  unlockInput.focus();
}
async function unlockWizard() {
  if (!locked || pending) return;
  const credential = unlockInput.value;
  unlockInput.value = "";
  pending = true;
  $("notice-close").disabled = true;
  try {
    const data = await request("/api/unlock", "POST", {credential});
    locked = false;
    $("notice-dialog").close();
    $("notice-close").textContent = "Close";
    lastRevision = Number(data.revision);
    renderState(data);
  } catch (error) {
    const problem = $("unlock-error");
    problem.textContent = error.message;
    problem.hidden = false;
    unlockInput.focus();
  } finally {
    pending = false;
    $("notice-close").disabled = false;
    updateFooter();
  }
}
$("notice-close").addEventListener("click", () => locked ? unlockWizard() : $("notice-dialog").close());
$("notice-dialog").addEventListener("cancel", event => { if (locked) event.preventDefault(); });

async function request(path, method = "GET", value = null) {
  const headers = {Authorization: "Bearer " + session};
  if (method !== "GET") {
    headers["Content-Type"] = "application/json";
    headers["X-CSRF-Token"] = csrf;
  }
  const response = await fetch(path, {method, headers, body: value === null ? undefined : JSON.stringify(value), cache: "no-store"});
  if (response.status === 401 && path !== "/api/unlock") {
    forgetSession();
    session = "";
    csrf = "";
    const error = new Error("The local session expired. Restart the CLI to open a new wizard.");
    error.unauthorized = true;
    throw error;
  }
  if (response.status === 409) throw new Error("This question changed. Review the current step and try again.");
  if (!response.ok) {
    const body = (await response.text()).trim();
    let message = body;
    try { message = JSON.parse(body).error || body; } catch { /* Plain-text error. */ }
    throw new Error(message || "The local wizard is unavailable.");
  }
  return response.status === 204 ? null : response.json();
}

let lastActivity = 0;
function reportActivity(event) {
  if (!event.isTrusted || !session || closed || closingRequested || locked || document.hidden || Date.now() - lastActivity < 1000) return;
  lastActivity = Date.now();
  request("/api/activity", "POST", {}).catch(() => {});
}
for (const kind of ["pointerdown", "keydown", "input", "scroll"]) {
  document.addEventListener(kind, reportActivity, {capture: kind === "scroll", passive: true});
}

function pauseCountdown() {
  if (!timer) return;
  clearInterval(timer);
  timer = null;
  paused = true;
  rememberPausedPrompt(promptId);
  $("pause-button").textContent = "Resume";
  updateCountdownLabel();
}
function stopCountdown() {
  if (timer) clearInterval(timer);
  timer = null;
  paused = false;
  $("pause-button").hidden = true;
}
function updateCountdownLabel() {
  if (!state?.prompt) return;
  const seconds = Math.max(0, Math.ceil((deadline - Date.now()) / 1000));
  $("footer-status").textContent = paused ? "Countdown paused. Review the value and continue when ready." :
    `The displayed default will be used in ${seconds} seconds.`;
}
function startCountdown(prompt, explicitResume = false) {
  stopCountdown();
  if (!prompt.countdownSeconds || prompt.secret || !["text", "saved"].includes(prompt.kind)) return;
  if (!explicitResume && (pausedPromptId === prompt.id || requireResumeOnRestore)) {
    rememberPausedPrompt(prompt.id);
    paused = true;
    $("pause-button").hidden = false;
    $("pause-button").textContent = "Resume";
    updateCountdownLabel();
    return;
  }
  forgetPausedPrompt();
  deadline = Date.now() + prompt.countdownSeconds * 1000;
  $("pause-button").hidden = false;
  $("pause-button").textContent = "Pause";
  updateCountdownLabel();
  timer = setInterval(() => {
    if (document.hidden) { pauseCountdown(); return; }
    if (Date.now() >= deadline) {
      stopCountdown();
      submitPrompt("primary");
    } else updateCountdownLabel();
  }, 200);
}
$("pause-button").addEventListener("click", () => {
  if (!state?.prompt) return;
  if (!paused) { pauseCountdown(); return; }
  // A resumed countdown gets its full, displayed interval.
  startCountdown(state.prompt, true);
});
document.addEventListener("visibilitychange", () => { if (document.hidden) pauseCountdown(); });
for (const eventName of ["input", "change", "pointerdown", "keydown", "scroll"]) {
  document.addEventListener(eventName, event => {
    if (event.isTrusted && !event.target.closest?.("#pause-button") && state?.prompt && timer) pauseCountdown();
  }, {capture: eventName === "scroll"});
}

function field(label, value, secret = false, help = "") {
  const wrapper = el("div", "", "form-field");
  const input = document.createElement("input");
  input.type = secret ? "password" : "text";
  input.autocomplete = "off";
  input.maxLength = secret ? 48 * 1024 : 8 * 1024;
  input.value = secret ? "" : value || "";
  input.setAttribute("aria-label", label);
  const caption = el("label", label);
  caption.append(input);
  wrapper.append(caption);
  if (help) wrapper.append(el("small", help));
  return {wrapper, input};
}
function renderChoice(prompt, multiple) {
  const grid = el("div", "", "action-grid");
  for (const option of prompt.options || []) {
    const choice = el("label", "", "action-card");
    const input = document.createElement("input");
    input.type = multiple ? "checkbox" : "radio";
    input.name = "setup-choice";
    input.value = option.value;
    input.checked = Boolean(option.selected);
    const title = el("span", option.label, "action-title");
    const description = el("span", option.description || "", "action-description");
    choice.append(input, title, description);
    grid.append(choice);
  }
  return grid;
}
function chooseTabFor(prompt) {
  activateTab($("tab-" + (["menu", "modules"].includes(prompt.kind) ? "actions" : "settings")));
}
function renderPrompt(prompt) {
  stopCountdown();
  const actions = $("action-content");
  const settings = $("settings-content");
  actions.replaceChildren();
  settings.replaceChildren();
  const title = prompt.label || (prompt.kind === "menu" ? "Choose setup tasks" : "Configure setup");
  const container = ["menu", "modules"].includes(prompt.kind) ? actions : settings;
  container.append(el("h2", title, "prompt-title"));
  const card = $("continue-button");
  card.hidden = false;
  $("back-button").hidden = prompt.kind === "menu";
  $("pause-button").hidden = true;
  card.disabled = false;
  card.textContent = "Continue";

  if (prompt.kind === "menu" || prompt.kind === "modules") {
    const note = el("p", prompt.kind === "menu" ?
      "Select independent setup operations. Review their effects before running them." :
      "Choose the modules and apps this repository should configure.", "prompt-help");
    container.append(note, renderChoice(prompt, true));
    const secondary = el("div", "", "secondary-actions");
    if (prompt.kind === "menu") {
      for (const [label, value] of [["What will this do?", "help"], ["View vault", "view-vault"], ["Remake vault", "remake-vault"]]) {
        const button = el("button", label, "button outline");
        button.type = "button";
        button.addEventListener("click", () => value === "help" ? showNotice("Selected setup tasks", selectedHelp()) : submitPrompt(value));
        secondary.append(button);
      }
      container.append(secondary);
    }
    card.textContent = prompt.kind === "menu" ? "Setup selected tasks" : "Continue";
    container.addEventListener("change", updateFooter);
  } else if (prompt.kind === "text") {
    const input = field(title, prompt.default, Boolean(prompt.secret));
    container.append(input.wrapper);
    input.input.addEventListener("keydown", event => { if (event.key === "Enter") { event.preventDefault(); submitPrompt("primary"); } });
  } else if (prompt.kind === "form") {
    const fields = el("div", "", "field-list");
    for (const item of prompt.fields || []) fields.append(field(item.label, item.value, Boolean(item.secret), item.help).wrapper);
    container.append(fields);
  } else if (prompt.kind === "confirm") {
    container.append(el("p", "Review the proposed change before you apply it.", "prompt-help"));
    const check = el("label", "", "confirmation");
    const input = document.createElement("input");
    input.type = "checkbox";
    check.append(input, el("span", "I have reviewed this and want to continue."));
    container.append(check);
    input.addEventListener("change", updateFooter);
    card.textContent = "Apply changes";
  } else if (prompt.kind === "saved") {
    const choices = [
      {value: "reuse", label: "Use vault value", description: "Keep the saved value for this step.", selected: true},
      {value: "new", label: "Set new value", description: "Enter a replacement in the next step."},
    ];
    container.append(el("p", `A saved value is available for ${title}.`, "prompt-help"), renderChoice({options: choices}, false));
    container.addEventListener("change", updateFooter);
  } else if (prompt.kind === "key") {
    container.append(el("p", prompt.message || "Select a local signing key or create a new one.", "prompt-help"));
    const list = el("div", "", "key-list");
    for (const option of prompt.options || []) {
      const row = el("label", "", "key-option");
      const input = document.createElement("input");
      input.type = "radio";
      input.name = "signing-key";
      input.value = option.value;
      row.append(input, el("span", ""));
      row.lastChild.append(el("strong", option.label), el("small", option.description || ""));
      list.append(row);
    }
    container.append(list);
    card.textContent = "Use selected key";
    container.addEventListener("change", updateFooter);
  } else {
    container.append(el("p", "Waiting for the next setup step.", "empty-state"));
    card.disabled = true;
  }
  const error = el("p", "", "problem");
  error.id = "prompt-error";
  error.hidden = true;
  container.append(error);
  chooseTabFor(prompt);
  updateFooter();
  startCountdown(prompt);
  requireResumeOnRestore = false;
}
function selectedHelp() {
  const options = state?.prompt?.options || [];
  const selected = new Set([...$("action-content").querySelectorAll('input[type="checkbox"]:checked')].map(input => input.value));
  const help = options.filter(option => selected.has(option.value)).map(option => `${option.label}\n${option.description || "No further details available."}`);
  return help.join("\n\n") || "Select a task to read its description.";
}
function promptValue(prompt, action) {
  if (action !== "primary") return action;
  const container = ["menu", "modules"].includes(prompt.kind) ? $("action-content") : $("settings-content");
  switch (prompt.kind) {
    case "text": {
      const input = container.querySelector("input");
      return prompt.secret ? input.value : input.value.trim();
    }
    case "form": return [...container.querySelectorAll(".form-field input")].map(input => input.value);
    case "confirm": return container.querySelector('.confirmation input').checked;
    case "menu":
    case "modules": return [...container.querySelectorAll('input[type="checkbox"]:checked')].map(input => Number(input.value));
    case "saved": return container.querySelector('input[type="radio"]:checked')?.value || "reuse";
    case "key": return container.querySelector('input[name="signing-key"]:checked')?.value || "";
    default: return "cancel";
  }
}
async function submitPrompt(action = "primary") {
  const prompt = state?.prompt;
  if (!prompt || pending || closed) return;
  const value = action === "cancel" && prompt.kind === "confirm" ? false : promptValue(prompt, action);
  if (action === "primary" && ((["menu", "modules"].includes(prompt.kind) && !value.length) ||
      (prompt.kind === "key" && !value) || (prompt.kind === "confirm" && !value))) return;
  if (prompt.countdownSeconds && !prompt.secret) rememberPausedPrompt(prompt.id);
  stopCountdown();
  pending = true;
  updateFooter();
  try {
    await request("/api/answer", "POST", action === "cancel" && prompt.kind !== "confirm" ?
      {id: prompt.id, cancel: true} : {id: prompt.id, value});
    forgetPausedPrompt();
    $("footer-status").textContent = "Continuing setup…";
    await refresh();
  } catch (error) {
    setProblem(error.message);
    if (prompt.countdownSeconds && state?.prompt?.id === prompt.id) {
      paused = true;
      $("pause-button").hidden = false;
      $("pause-button").textContent = "Resume";
    }
  } finally {
    pending = false;
    updateFooter();
  }
}
$("continue-button").addEventListener("click", () => {
  if (["idle", "finished", "error"].includes(state?.status)) startSetup();
  else submitPrompt("primary");
});
$("back-button").addEventListener("click", () => submitPrompt("cancel"));
$("close-button").addEventListener("click", async () => {
  if (closed || closingRequested) return;
  if (state?.status === "running" && !window.confirm("Stop the current setup? Accepted answers remain saved for a later run.")) return;
  closingRequested = true;
  stopCountdown();
  updateFooter();
  try {
    await request("/api/close", "POST", {});
    // Keep the scoped session until cleanup is confirmed, including if this
    // page reloads while the worker is still stopping.
    $("footer-status").textContent = "Stop requested; waiting for cleanup…";
    $("action-content").replaceChildren(el("p", "Stop requested; waiting for local work to finish and release its locks.", "empty-state"));
    await refresh();
  } catch (error) {
    closingRequested = false;
    setProblem(error.message);
  } finally { updateFooter(); }
});
function finishClose() {
  if (locked) {
    locked = false;
    $("notice-dialog").close();
    $("notice-close").textContent = "Close";
  }
  forgetSession();
  session = "";
  csrf = "";
  closed = true;
  closingRequested = false;
  stopCountdown();
  $("action-content").replaceChildren(el("p", "The local wizard is closed. You may close this tab.", "empty-state"));
  $("footer-status").textContent = "Session closed after cleanup";
  $("back-button").hidden = true;
  $("continue-button").hidden = true;
  $("close-button").disabled = true;
}
async function startSetup() {
  if (pending || closed) return;
  pending = true;
  updateFooter();
  try { await request("/api/start", "POST", {action: "setup"}); await refresh(); }
  catch (error) { setProblem(error.message); }
  finally { pending = false; updateFooter(); }
}
function updateFooter() {
  const prompt = state?.prompt;
  const primary = $("continue-button");
  if (closed || closingRequested || state?.status === "closing" || state?.status === "locked") {
    primary.disabled = true;
    $("back-button").hidden = true;
    $("pause-button").hidden = true;
    $("close-button").disabled = true;
    return;
  }
  if (pending) { primary.disabled = true; return; }
  if (["idle", "finished", "error"].includes(state?.status)) {
    primary.hidden = false;
    primary.textContent = state.status === "idle" ? "Start setup" : "Run setup again";
    primary.disabled = false;
    $("back-button").hidden = true;
    return;
  }
  if (!prompt) { primary.hidden = true; $("back-button").hidden = true; return; }
  primary.hidden = false;
  const container = ["menu", "modules"].includes(prompt.kind) ? $("action-content") : $("settings-content");
  const choices = container.querySelectorAll('input[type="checkbox"]:checked');
  primary.disabled = ["menu", "modules"].includes(prompt.kind) && choices.length === 0 ||
    prompt.kind === "confirm" && !container.querySelector('.confirmation input')?.checked ||
    prompt.kind === "key" && !container.querySelector('input[name="signing-key"]:checked');
}

function renderProgress(data) {
  const phases = [
    ["repository", "Inspect repository"], ["actions", "Choose tasks"], ["configure", "Configure"],
    ["review", "Review changes"], ["apply", "Apply setup"], ["complete", "Finish"],
  ];
  const phase = String(data.phase || "").toLowerCase();
  const current = phase.includes("module") ? 1 : phase.includes("action") || phase.includes("menu") ? 1 :
    phase.includes("review") || phase.includes("confirm") ? 3 :
    phase.includes("apply") || phase.includes("write") ? 4 :
    phase.includes("complete") || data.status === "finished" ? 5 :
    phase.includes("sign") || phase.includes("publish") || phase.includes("workflow") || phase.includes("vault") ? 2 : 0;
  const list = $("phase-list");
  list.replaceChildren(...phases.map(([id, label], index) => {
    const item = el("li", label, index < current ? "complete" : index === current ? "current" : "");
    if (index === current) item.setAttribute("aria-current", "step");
    if (data.status === "error" && index === current) item.classList.add("failed");
    return item;
  }));
}
function renderContext(data) {
  const context = data.context || {};
  const repository = String(context.repository || "");
  const root = String(context.root || "");
  const parts = [["Repository", repository || "Current checkout"], ["Local path", root || "Current directory"]];
  if (Array.isArray(context.modules) && context.modules.length) {
    const modules = context.modules.filter(module => typeof module === "string" || module.selected !== false)
      .map(module => typeof module === "string" ? module : module.name || module.path || module.directory || "Module");
    parts.push(["Modules", modules.length ? modules.join(", ") : "No modules selected"]);
  }
  const list = $("context-list");
  list.replaceChildren();
  for (const [label, value] of parts) list.append(el("dt", label), el("dd", value));
  const link = $("repository-link");
  if (/^[\w.-]+\/[\w.-]+$/.test(repository)) {
    link.href = "https://github.com/" + repository;
    link.textContent = "/" + repository;
    link.hidden = false;
  } else link.hidden = true;
  $("project-tab").textContent = repository.split("/").at(-1) || "Repository setup";
  document.title = "Setup: " + $("project-tab").textContent;
}
function renderArtifacts(data) {
  const artifacts = Array.isArray(data.artifacts) ? data.artifacts : [];
  const signature = JSON.stringify(artifacts);
  if (signature === artifactSignature) return;
  artifactSignature = signature;
  $("artifact-count").textContent = String(artifacts.length);
  const list = $("artifact-list");
  const query = $("artifact-search").value.trim().toLowerCase();
  list.replaceChildren();
  for (const artifact of artifacts) {
    const path = typeof artifact === "string" ? artifact : artifact.path || artifact.name || "File";
    if (!path.toLowerCase().includes(query)) continue;
    const row = el("button", "", "artifact-row");
    row.type = "button";
    const icon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    icon.classList.add("icon");
    const use = document.createElementNS(icon.namespaceURI, "use");
    use.setAttribute("href", "#icon-file");
    icon.append(use);
    row.append(icon, el("span", path, "artifact-name"), el("span", artifact.status || "", "artifact-status"));
    row.setAttribute("aria-current", String(selectedDetail === path));
    row.addEventListener("click", () => {
      selectedDetail = path;
      $("detail-content").replaceChildren(el("h3", path), el("p", artifact.description || artifact.status || "Generated by setup."));
      for (const other of list.querySelectorAll(".artifact-row")) other.setAttribute("aria-current", String(other === row));
    });
    list.append(row);
  }
  if (!list.childElementCount) list.append(el("p", artifacts.length ? "No matching files." : "Affected files appear here when known.", "empty-state"));
}
$("artifact-search").addEventListener("input", () => { artifactSignature = ""; if (state) renderArtifacts(state); });
function renderLog(data) {
  const lines = Array.isArray(data.notes) ? data.notes : String(data.notes || "").split("\n").filter(Boolean);
  const output = $("process-output");
  const content = lines.join("\n");
  if (output.textContent !== content) {
    const scrollEnd = output.scrollTop + output.clientHeight >= output.scrollHeight - 30;
    output.textContent = content;
    if (scrollEnd) output.scrollTop = output.scrollHeight;
  }
  $("log-count").textContent = `${lines.length} ${lines.length === 1 ? "line" : "lines"}`;
}
$("wrap-output").addEventListener("click", event => {
  const pressed = event.currentTarget.getAttribute("aria-pressed") !== "true";
  event.currentTarget.setAttribute("aria-pressed", String(pressed));
  $("process-output").classList.toggle("wrapped", pressed);
});
function renderOverview(data) {
  const content = $("overview-content");
  content.replaceChildren();
  const add = (title, value, error = false) => {
    const card = el("section", "", "overview-card" + (error ? " error" : ""));
    card.append(el("h2", title), el("p", value));
    content.append(card);
  };
  add("Current step", data.prompt?.label || data.phase || "Ready to start");
  const selected = data.prompt?.kind === "menu" ? (data.prompt.options || []).filter(option => option.selected).map(option => option.label) : [];
  if (selected.length) add("Selected setup tasks", selected.join("\n"));
  if (data.status === "error") add("Setup needs attention", (data.notes || []).at(-1) || "Review process output for recovery steps.", true);
  if (data.status === "finished") add("Setup finished", "Review the generated files and the process output.");
  add("Files and outputs", `${(data.artifacts || []).length} known`);
}
function renderState(data) {
  state = data;
  if (data.status === "closed") { finishClose(); return; }
  if (data.status === "locked") {
    showLock();
    $("footer-status").textContent = "Wizard locked after inactivity";
    updateFooter();
    return;
  }
  if (locked) {
    locked = false;
    $("notice-dialog").close();
    $("notice-close").textContent = "Close";
  }
  if (data.status === "closing") {
    closingRequested = true;
    stopCountdown();
    $("action-content").replaceChildren(el("p", "Stop requested; waiting for local work to finish and release its locks.", "empty-state"));
    $("footer-status").textContent = "Stop requested; waiting for cleanup…";
  }
  const status = $("action-state");
  status.textContent = ({idle: "Ready", running: "In progress", finished: "Complete", error: "Needs attention", closing: "Stopping"})[data.status] || data.status || "Connecting";
  status.className = "state-badge " + (data.status === "running" ? "running" : data.status === "finished" ? "complete" : data.status === "error" ? "failed" : "");
  renderContext(data);
  renderProgress(data);
  renderArtifacts(data);
  renderLog(data);
  renderOverview(data);
  const nextKeySignature = data.prompt?.kind === "key" ? JSON.stringify([data.prompt.options, data.prompt.message]) : "";
  const nextPromptId = data.prompt?.id ?? null;
  const promptChanged = nextPromptId !== promptId;
  const statusChanged = data.status !== lastStatus;
  if (promptChanged && promptId !== undefined) forgetPausedPrompt();
  if (promptChanged || statusChanged && !data.prompt || nextKeySignature !== keySignature && data.prompt?.kind === "key") {
    const selectedKey = promptChanged ? "" : $("settings-content").querySelector('input[name="signing-key"]:checked')?.value || "";
    const existingTab = activeTab;
    keySignature = nextKeySignature;
    promptId = nextPromptId;
    if (data.prompt && data.status !== "closing") {
      renderPrompt(data.prompt);
      if (!promptChanged) {
        const key = [...$("settings-content").querySelectorAll('input[name="signing-key"]')].find(input => input.value === selectedKey);
        if (key) key.checked = true;
        updateFooter();
        activateTab($("tab-" + existingTab));
      }
    }
    else {
      stopCountdown();
      $("settings-content").replaceChildren(el("p", data.status === "running" ? "Working… Accepted answers are saved as setup progresses." : "Choose an action to inspect its settings.", "empty-state"));
      if (data.status === "idle") $("action-content").replaceChildren(el("p", "Start the local setup to choose repository tasks.", "empty-state"));
      else if (data.status === "running") $("action-content").replaceChildren(el("p", "Working… New choices will appear here when they are needed.", "empty-state"));
      else if (data.status === "finished" || data.status === "error") {
        $("action-content").replaceChildren(el("p", data.status === "finished" ? "Setup finished. Review the results in Overview and Process output." : "Setup needs attention. Review Process output for recovery steps.", "empty-state"));
        activateTab($(data.status === "error" ? "tab-output" : "tab-overview"));
      }
    }
  }
  lastStatus = data.status;
  if (!data.prompt && data.status === "running") $("footer-status").textContent = "Working on your repository…";
  else if (!data.prompt && data.status === "idle") $("footer-status").textContent = "Ready to set up this repository";
  else if (!data.prompt && data.status === "finished") $("footer-status").textContent = "Setup complete";
  else if (!data.prompt && data.status === "error") $("footer-status").textContent = "Setup needs attention; review the process output";
  else if (data.status === "closing") $("footer-status").textContent = "Stop requested; waiting for cleanup…";
  else if (data.prompt && paused) updateCountdownLabel();
  if (!data.prompt && ["idle", "finished", "error"].includes(data.status)) requireResumeOnRestore = false;
  updateFooter();
  // The lock screen hides footer controls. The same pending prompt can return
  // after unlock without triggering renderPrompt, so restore its paused action.
  if (data.prompt && paused && data.prompt.countdownSeconds && !data.prompt.secret) {
    $("pause-button").hidden = false;
    $("pause-button").textContent = "Resume";
    updateCountdownLabel();
  }
}
async function refresh() {
  if (closed || !session) return;
  const data = await request("/api/state");
  if (Number(data.revision) !== lastRevision) {
    lastRevision = Number(data.revision);
    renderState(data);
  }
}
async function poll() {
  if (closed) return;
  try { await refresh(); }
  catch (error) {
    setProblem(error.message);
    if (error.unauthorized) {
      closed = true;
      stopCountdown();
      $("continue-button").disabled = true;
      $("close-button").disabled = true;
    }
  }
  if (!closed) setTimeout(poll, 500);
}

// Capture the fragment before any request can include it in navigation. The
// server exchanges this launch-only token for a short-lived session and CSRF.
const fragment = location.hash.startsWith("#token=") ? location.hash.slice(7) : "";
if (location.hash) history.replaceState(null, "", location.pathname + location.search);
async function bootstrap() {
  const previous = savedSession();
  if (previous) {
    session = previous.session;
    csrf = previous.csrf;
    requireResumeOnRestore = true;
    try {
      await refresh();
      $("close-button").disabled = false;
      poll();
      return;
    } catch (error) {
      if (!error.unauthorized) {
        setProblem("Connection interrupted. Retrying the saved local session…");
        poll();
        return;
      }
      forgetSession();
      session = "";
      csrf = "";
      requireResumeOnRestore = false;
      if (!/^[a-fA-F0-9]{64}$/.test(fragment)) {
        setProblem(error.message);
        return;
      }
    }
  }
  if (!/^[a-fA-F0-9]{64}$/.test(fragment)) {
    setProblem("Open the private setup URL printed by the CLI to start this wizard.");
    $("action-content").replaceChildren(el("p", "Launch this page from the CLI's printed link.", "empty-state"));
    return;
  }
  try {
    const response = await fetch("/api/session", {method: "POST", headers: {"Content-Type": "application/json"}, body: JSON.stringify({token: fragment}), cache: "no-store"});
    if (!response.ok) throw new Error("This setup link has expired. Start the CLI again to open a fresh wizard.");
    const credentials = await response.json();
    session = credentials.session;
    csrf = credentials.csrf;
    rememberSession();
    $("close-button").disabled = false;
    await refresh();
    poll();
  } catch (error) { setProblem(error.message); }
}
bootstrap();

// Both mouse and keyboard can resize panels. Narrow layouts stack the panes.
for (const handle of document.querySelectorAll("[data-split]")) {
  let start;
  const adjust = delta => {
    if (window.matchMedia("(max-width: 1120px)").matches) return;
    const type = handle.dataset.split;
    if (type === "left" || type === "right") {
      const property = type === "left" ? "--left" : "--right";
      const current = parseFloat(getComputedStyle($("workspace")).getPropertyValue(property)) || (type === "left" ? 290 : 345);
      $("workspace").style.setProperty(property, `${Math.max(type === "left" ? 225 : 260, Math.min(540, current + (type === "left" ? delta : -delta)))}px`);
    } else {
      const pane = type === "sidebar" ? $("sidebar") : $("right");
      const first = pane.firstElementChild;
      const min = 100;
      const current = first.getBoundingClientRect().height;
      pane.dataset.openRows = `${Math.max(min, Math.min(pane.clientHeight - 150, current + delta))}px 6px minmax(150px,1fr)`;
      pane.style.gridTemplateRows = pane.dataset.openRows;
    }
  };
  handle.addEventListener("pointerdown", event => { start = {x: event.clientX, y: event.clientY}; handle.setPointerCapture(event.pointerId); });
  handle.addEventListener("pointermove", event => {
    if (!start) return;
    const delta = handle.getAttribute("aria-orientation") === "vertical" ? event.clientX - start.x : event.clientY - start.y;
    adjust(delta);
    start = {x: event.clientX, y: event.clientY};
  });
  handle.addEventListener("pointerup", () => { start = null; });
  handle.addEventListener("keydown", event => {
    const horizontal = handle.getAttribute("aria-orientation") === "vertical";
    if ((horizontal && ["ArrowLeft", "ArrowRight"].includes(event.key)) || (!horizontal && ["ArrowUp", "ArrowDown"].includes(event.key))) {
      event.preventDefault();
      adjust(["ArrowLeft", "ArrowUp"].includes(event.key) ? -16 : 16);
    }
  });
}
function togglePanel(pane, body, buttonIds, key) {
  const open = !pane.classList.contains("collapsed");
  pane.classList.toggle("collapsed", open);
  pane.style.gridTemplateRows = open ? "34px 6px minmax(150px,1fr)" : pane.dataset.openRows || "";
  body.hidden = open;
  for (const id of buttonIds) {
    const button = $(id);
    button.setAttribute("aria-expanded", String(!open));
    button.setAttribute("aria-label", (open ? "Expand " : "Collapse ") + key);
    button.classList.toggle("active", !open);
    if (button.classList.contains("panel-toggle")) button.textContent = open ? "+" : "−";
  }
}
for (const id of ["toggle-progress", "stripe-progress"]) $(id).addEventListener("click", () => togglePanel($("sidebar"), $("phase-list"), ["toggle-progress", "stripe-progress"], "progress"));
for (const id of ["toggle-details", "stripe-details"]) $(id).addEventListener("click", () => togglePanel($("right"), $("detail-content"), ["toggle-details", "stripe-details"], "details"));
