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
let lastPhase;
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
const moduleFocusKey = "android-repo-setup-module-focus-v1";
let activeModule = (() => { try { return sessionStorage.getItem(moduleFocusKey) || null; } catch { return null; } })();
let availableModules = [];
let moduleTabsKey = "";
let lastMenuOptions = null;
let menuSelectionId = null;
const menuSelections = new Map();
// Prepared values remain in this browser tab's memory. Secrets are never
// written to sessionStorage or included in the state snapshot.
const settingsDrafts = new Map();
let draftExecution = null;
function clearPreparedValues() {
  settingsDrafts.clear();
  draftExecution = null;
  for (const input of $("workspace").querySelectorAll('input[type="password"]')) input.value = "";
  $("vault-credential").value = "";
  $("vault-confirmation").value = "";
}

const actionFields = {
  key: [
    {key: "keystore-path", label: "Keystore path", prompts: ["Keystore path"], help: "Outside the repository. Leave blank to use the saved or suggested path."},
    {key: "signing-key-alias", label: "Signing key alias", prompts: ["Signing key alias"], help: "Leave blank to use the saved alias."},
    {key: "keystore-password", label: "Keystore password", prompts: ["Keystore password"], secret: true, help: "At least six characters when creating a key."},
    {key: "confirm-keystore-password", label: "Confirm keystore password", prompts: ["Confirm keystore password"], secret: true},
    {key: "key-password", label: "Key password", prompts: ["Key password"], secret: true, help: "At least six characters when creating a key."},
    {key: "confirm-key-password", label: "Confirm key password", prompts: ["Confirm key password"], secret: true},
    {key: "application-ids", label: "Android application IDs", prompts: ["Android application IDs (comma-separated installed package names)"], help: "Comma-separated package names; leave blank to review discovered IDs later."},
  ],
  local: [],
  github: [
    {key: "github-token", label: "GitHub token", prompts: ["GitHub token"], secret: true, help: "Repository administration and environment permissions. Existing vault values can be reused."},
  ],
  workflows: [],
};
const actionNotes = {
  key: "The wizard will verify or create a keystore and ask you to review changes. Saved values can be reused.",
  local: "Uses the signing identity saved in the vault. You will confirm the write to ignored local.properties.",
  github: "Existing GitHub secrets are inspected before changes. A keystore may also be required if signing secrets are missing.",
  publishing: "Credentials depend on .project/setup/repositories.yml and the current GitHub environments. GPG key selection and final review occur during setup.",
  workflows: "No credentials are required. Changes to existing workflow files require your confirmation.",
};

$("sidebar-menu").addEventListener("click", () => {
  const hidden = $("workspace").classList.toggle("sidebar-hidden");
  const label = hidden ? "Show repository panels" : "Hide repository panels";
  $("sidebar-menu").setAttribute("aria-expanded", String(!hidden));
  $("sidebar-menu").setAttribute("aria-label", label);
  $("sidebar-menu").title = label;
});

const mainMenuButton = $("main-menu-button");
const mainMenuPanel = $("main-menu-panel");
const mainMenuWrap = mainMenuButton.parentElement;
const vaultMenuItems = [$("menu-view-vault"), $("menu-remake-vault")];
function menuItems() {
  return [...mainMenuPanel.querySelectorAll('[role="menuitem"]:not(:disabled)')];
}
function closeMainMenu(restoreFocus = false) {
  if (mainMenuPanel.hidden) return;
  mainMenuPanel.hidden = true;
  mainMenuButton.setAttribute("aria-expanded", "false");
  if (restoreFocus) mainMenuButton.focus();
}
function openMainMenu(focus = false, last = false) {
  mainMenuPanel.hidden = false;
  mainMenuButton.setAttribute("aria-expanded", "true");
  if (focus) {
    const items = menuItems();
    (last ? items.at(-1) : items[0])?.focus();
  }
}
function syncMainMenu() {
  const ready = Boolean(session) && !pending && !closed && !closingRequested && !locked &&
    (state?.prompt?.kind === "menu" || ["idle", "finished", "error"].includes(state?.status));
  const reason = ready ? "Open vault controls" : locked || state?.status === "locked" ?
    "Unlock the vault to use these controls" : "Available while choosing tasks or after setup stops";
  for (const item of vaultMenuItems) {
    item.disabled = !ready;
    item.title = reason;
    item.setAttribute("aria-description", reason);
  }
  $("menu-close-wizard").disabled = !session || closed || closingRequested || locked || state?.status === "closing" || state?.status === "locked";
  if (!mainMenuPanel.hidden && !mainMenuWrap.contains(document.activeElement)) closeMainMenu();
}
mainMenuButton.addEventListener("click", event => mainMenuPanel.hidden ? openMainMenu(event.detail === 0) : closeMainMenu());
mainMenuButton.addEventListener("keydown", event => {
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    openMainMenu(true, event.key === "ArrowUp");
  }
});
mainMenuPanel.addEventListener("keydown", event => {
  const items = menuItems();
  const index = items.indexOf(document.activeElement);
  if (event.key === "Escape") { event.preventDefault(); closeMainMenu(true); return; }
  if (event.key === "Tab") {
    event.preventDefault();
    closeMainMenu();
    (event.shiftKey ? mainMenuButton : $("sidebar-menu")).focus();
    return;
  }
  const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 :
    event.key === "ArrowDown" ? (index + 1) % items.length :
      event.key === "ArrowUp" ? (index + items.length - 1) % items.length : -1;
  if (next >= 0) { event.preventDefault(); items[next]?.focus(); }
});
document.addEventListener("keydown", event => {
  if (event.key === "Escape" && !mainMenuPanel.hidden) { event.preventDefault(); closeMainMenu(true); }
});
document.addEventListener("pointerdown", event => {
  if (!mainMenuPanel.hidden && !mainMenuWrap.contains(event.target)) closeMainMenu();
});
document.addEventListener("focusin", event => {
  if (!mainMenuPanel.hidden && !mainMenuWrap.contains(event.target)) closeMainMenu();
});
$("menu-toggle-panels").addEventListener("click", () => {
  closeMainMenu();
  $("sidebar-menu").click();
});
$("menu-close-wizard").addEventListener("click", () => {
  closeMainMenu();
  $("close-button").click();
});
async function runVaultMenuAction(action) {
  const menuPrompt = state?.prompt?.kind === "menu";
  closeMainMenu();
  if (pending || locked || closed || closingRequested || !session) return;
  if (menuPrompt) { await submitPrompt(action); return; }
  if (!["idle", "finished", "error"].includes(state?.status)) return;
  pending = true;
  updateFooter();
  try {
    const data = await request("/api/vault-action", "POST", {action});
    if (data?.revision != null) {
      lastRevision = Number(data.revision);
      renderState(data);
    } else await refresh();
  } catch (error) { setProblem(error.message); }
  finally { pending = false; updateFooter(); }
}
$("menu-view-vault").addEventListener("click", () => runVaultMenuAction("view-vault"));
$("menu-remake-vault").addEventListener("click", () => runVaultMenuAction("remake-vault"));

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
let locked = false;
let vaultMode = "unlock";
let vaultWorkerPromptId = null;
function obscureLockedWorkspace(value) {
  for (const id of ["workspace", "action-bar"]) {
    $(id).inert = value;
    $(id).classList.toggle("vault-locked", value);
  }
}
function vaultProblem(message) {
  $("vault-error").textContent = message;
  $("vault-error").hidden = !message;
}
function closeVaultDialog() {
  $("vault-credential").value = "";
  $("vault-confirmation").value = "";
  vaultProblem("");
  if ($("vault-dialog").open) $("vault-dialog").close();
  vaultWorkerPromptId = null;
}
function openVaultDialog(title, description, mode, confirm = false) {
  vaultMode = mode;
  $("vault-title").textContent = title;
  $("vault-description").textContent = description;
  $("vault-credential").value = "";
  $("vault-credential").autocomplete = confirm ? "new-password" : "current-password";
  $("vault-confirmation").value = "";
  $("vault-confirm-group").hidden = !confirm;
  $("vault-confirmation").required = confirm;
  $("vault-cancel").hidden = mode !== "worker";
  $("vault-submit").textContent = confirm ? "Create vault" : mode === "worker" ? "Continue" : "Unlock";
  vaultProblem("");
  if (!$("vault-dialog").open) $("vault-dialog").showModal();
  $("vault-credential").focus();
}
function showLock(data) {
  if (locked) return;
  locked = true;
  obscureLockedWorkspace(true);
  pauseCountdown();
  clearPreparedValues();
  vaultWorkerPromptId = null;
  const creating = data.vaultMode === "create";
  openVaultDialog(creating ? "Create local vault" : data.initial ? "Unlock local vault" : "Wizard locked",
    creating ? "Choose a vault passphrase of at least 12 characters. It encrypts saved setup answers on this device." :
      data.initial ? "Enter your vault passphrase before the wizard inspects this repository." :
        "Enter your vault passphrase to continue. The recovery code printed in the launching terminal can also unlock an idle session.",
    creating ? "create" : "unlock", creating);
}
async function unlockWizard() {
  if (!locked || pending) return;
  const credential = $("vault-credential").value;
  const confirmation = $("vault-confirmation").value;
  if (vaultMode === "create" && (credential.length < 12 || confirmation !== credential)) {
    vaultProblem(credential.length < 12 ? "Use at least 12 characters." : "The passphrases do not match.");
    $("vault-credential").focus();
    return;
  }
  if (!credential) { vaultProblem("Enter the vault passphrase."); $("vault-credential").focus(); return; }
  $("vault-credential").value = "";
  $("vault-confirmation").value = "";
  pending = true;
  $("vault-submit").disabled = true;
  let start = false;
  try {
    const data = await request("/api/unlock", "POST", vaultMode === "create" ? {credential, confirmation} : {credential});
    locked = false;
    closeVaultDialog();
    obscureLockedWorkspace(false);
    lastRevision = Number(data.revision);
    renderState(data);
    start = data.status === "idle";
  } catch (error) {
    vaultProblem(error.message);
    $("vault-credential").focus();
  } finally {
    pending = false;
    $("vault-submit").disabled = false;
    updateFooter();
  }
  if (start) await startSetup();
}
$("vault-dialog").addEventListener("cancel", event => event.preventDefault());
$("vault-form").addEventListener("submit", event => {
  event.preventDefault();
  if (locked) unlockWizard();
  else if (vaultWorkerPromptId != null) answerVaultPrompt();
});

async function request(path, method = "GET", value = null) {
  const headers = {Authorization: "Bearer " + session};
  if (method !== "GET") {
    headers["Content-Type"] = "application/json";
    headers["X-CSRF-Token"] = csrf;
  }
  const response = await fetch(path, {method, headers, body: value === null ? undefined : JSON.stringify(value), cache: "no-store"});
  if (response.status === 401 && path !== "/api/unlock") {
    forgetSession();
    clearPreparedValues();
    session = "";
    csrf = "";
    const error = new Error("The local session expired. Restart the CLI to open a new wizard.");
    error.unauthorized = true;
    throw error;
  }
  if (response.status === 409 && path === "/api/answer") throw new Error("This question changed. Review the current step and try again.");
  if (!response.ok) {
    const body = (await response.text()).trim();
    let message = body;
    try { message = JSON.parse(body).error || body; } catch { /* Plain-text error. */ }
    throw new Error(message || "The local wizard is unavailable.");
  }
  return response.status === 204 ? null : response.json();
}

function isVaultPrompt(prompt) {
  return prompt?.kind === "text" && prompt.secret &&
    /^(Create vault password|Confirm vault password|Unlock local setup vault)/.test(prompt.label || "");
}
function showVaultPrompt(prompt) {
  vaultWorkerPromptId = prompt.id;
  const creating = prompt.label.startsWith("Create vault password");
  openVaultDialog(prompt.label, creating ? "Choose at least 12 characters for the local encrypted vault." :
    prompt.label.startsWith("Confirm vault password") ? "Repeat the vault passphrase to confirm it." :
      "Enter the vault passphrase for this setup action.", "worker");
}
async function answerVaultPrompt(cancel = false) {
  const prompt = state?.prompt;
  if (locked || pending || !isVaultPrompt(prompt) || vaultWorkerPromptId !== prompt.id) return;
  const value = $("vault-credential").value;
  if (!cancel && (!value || prompt.label.startsWith("Create vault password") && value.length < 12)) {
    vaultProblem(value ? "Use at least 12 characters." : "Enter the vault passphrase.");
    $("vault-credential").focus();
    return;
  }
  $("vault-credential").value = "";
  pending = true;
  $("vault-submit").disabled = true;
  try {
    await request("/api/answer", "POST", cancel ? {id: prompt.id, cancel: true} : {id: prompt.id, value});
    closeVaultDialog();
    await refresh();
  } catch (error) {
    vaultProblem(error.message);
    $("vault-credential").focus();
  } finally {
    pending = false;
    $("vault-submit").disabled = false;
    updateFooter();
  }
}
$("vault-cancel").addEventListener("click", () => answerVaultPrompt(true));

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
function visibleMenuCards() {
  return [...$("action-content").querySelectorAll(".action-card")].filter(card => !card.hidden);
}
function selectedMenuCards() {
  return visibleMenuCards().filter(card => card.querySelector('input[type="checkbox"]:checked'));
}
function fieldsForAction(action, data) {
  if (action === "github") return [
    ...actionFields.github,
    ...actionFields.key.map(item => ({...item, draftAction: "key", help: item.help || "Needed only if GitHub signing secrets require a local keystore."})),
  ];
  if (action !== "publishing") return actionFields[action] || [];
  const fields = [{key: "github-access-token", label: "GitHub access token", prompts: ["GitHub access token"], secret: true,
    help: "Used to inspect and configure GitHub publishing environments."}];
  for (const destination of data?.context?.publishing?.destinations || []) {
    for (const item of destination.fields || []) {
      if (!item?.name || !item.secret) continue;
      // A GPG identity is chosen and reviewed by the CLI; it is not a text
      // credential that can safely be staged in this form.
      if (["SIGNING_IN_MEMORY_KEY", "SIGNING_IN_MEMORY_KEY_PASSWORD"].includes(item.name)) continue;
      fields.push({key: `${destination.environment}/${item.name}`, label: `${destination.environment} / ${item.name}`,
        prompts: [item.inputLabel], secret: true,
        help: item.required ? "Required when this secret is absent in GitHub." : "Optional. Leave blank to preserve an existing value."});
    }
  }
  return fields;
}
function draftKey(moduleId, actionId, fieldKey) { return `${moduleId}\u0000${actionId}\u0000${fieldKey}`; }
function renderSettingsGroups(data) {
  const options = data?.prompt?.kind === "menu" ? data.prompt.options : lastMenuOptions;
  if (!Array.isArray(options)) return;
  const content = $("settings-content");
  const module = availableModules.find(item => item.id === activeModule);
  let groups = $("prepared-settings");
  if (!groups || !content.contains(groups)) {
    groups = el("div");
    groups.id = "prepared-settings";
    content.append(groups);
  }
  groups.replaceChildren();
  if (!module) {
    groups.append(el("p", "Discovering modules and preparing settings…", "empty-state"));
    return;
  }
  groups.append(el("p", `Prepare ${module.path} values here. Blank fields are requested by the wizard only if needed. Prepared values stay in this tab's memory until setup ends; saved vault values are never shown.`, "settings-note"));
  const vaultGroup = el("section", "", "settings-group");
  vaultGroup.append(el("h2", "Local encrypted vault"), el("p", data.context?.vaultExists ?
    "Vault ready. Saved credentials remain hidden and are requested only when an action needs them." :
    "The wizard will create a vault before configuring actions."));
  groups.append(vaultGroup);
  const applicable = options.filter(option => operationAppliesToModule(option.id, module));
  if (!applicable.length) {
    groups.append(el("p", "This module is excluded from setup. Use the terminal module selector to include it.", "empty-state"));
    return;
  }
  for (const option of applicable) {
    const group = el("section", "", "settings-group");
    group.dataset.operationId = option.id;
    group.append(el("h2", option.label), el("p", actionNotes[option.id] || option.description || ""));
    if (option.id === "publishing" && data.context?.publishing?.registryPresent === false) {
      group.append(el("p", "Add .project/setup/repositories.yml before configuring publishing environments.", "problem"));
    }
    if (option.id === "publishing" && data.context?.publishing?.error) {
      group.append(el("p", `Publishing configuration needs attention: ${data.context.publishing.error}`, "problem"));
    }
    const fields = fieldsForAction(option.id, data);
    if (fields.length) {
      const list = el("div", "", "field-list");
      for (const item of fields) {
        const wrapper = el("div", "", "settings-field");
        const input = document.createElement("input");
        input.type = item.secret ? "password" : "text";
        input.autocomplete = "off";
        input.spellcheck = false;
        input.maxLength = item.secret ? 48 * 1024 : 8 * 1024;
        const key = draftKey(module.id, item.draftAction || option.id, item.key);
        input.dataset.draftKey = key;
        input.value = settingsDrafts.get(key) || "";
        input.placeholder = "Leave blank to review when requested";
        const label = el("label", item.label);
        label.append(input);
        wrapper.append(label);
        if (item.help) wrapper.append(el("small", item.help));
        input.addEventListener("input", () => {
          if (input.value) settingsDrafts.set(key, input.value);
          else settingsDrafts.delete(key);
          for (const other of groups.querySelectorAll(".settings-field input")) {
            if (other !== input && other.dataset.draftKey === key) other.value = input.value;
          }
        });
        list.append(wrapper);
      }
      group.append(list);
    } else group.append(el("p", "No advance inputs. The wizard will show any review or confirmation steps when needed.", "settings-note"));
    groups.append(group);
  }
}
function preparedAnswers(moduleId, selectedActions) {
  const answers = new Map();
  const actions = new Set(selectedActions);
  // GitHub signing may need the same existing keystore details even if only
  // the environment action is checked.
  if (actions.has("github")) actions.add("key");
  for (const actionId of actions) {
    for (const item of fieldsForAction(actionId, state)) {
      const value = settingsDrafts.get(draftKey(moduleId, item.draftAction || actionId, item.key));
      if (value) for (const promptLabel of item.prompts || []) answers.set(promptLabel, value);
    }
  }
  return answers;
}
function validatePreparedPasswords(moduleId, selectedActions) {
  if (!selectedActions.includes("key") && !selectedActions.includes("github")) return true;
  for (const [password, confirmation, label] of [
    ["keystore-password", "confirm-keystore-password", "keystore"],
    ["key-password", "confirm-key-password", "key"],
  ]) {
    const entered = settingsDrafts.get(draftKey(moduleId, "key", password));
    const repeated = settingsDrafts.get(draftKey(moduleId, "key", confirmation));
    if (entered && repeated && entered !== repeated) {
      setProblem(`The ${label} password and confirmation differ.`);
      activateTab($("tab-settings"));
      return false;
    }
  }
  return true;
}
function schedulePreparedAnswer(prompt) {
  if (!draftExecution || !["text", "saved"].includes(prompt.kind) ||
      draftExecution.used.has(prompt.id) || !draftExecution.answers.has(prompt.label)) return;
  const value = draftExecution.answers.get(prompt.label);
  if (!value) return;
  if (prompt.kind === "text") {
    const input = $("settings-content").querySelector(".form-field input");
    if (!input) return;
    input.value = value;
  } else {
    const replacement = [...$("settings-content").querySelectorAll('input[type="radio"]')].find(input => input.value === "new");
    if (!replacement) return;
    replacement.checked = true;
  }
  draftExecution.used.add(prompt.id);
  // The user already chose these prepared values by pressing Setup. Never
  // auto-answer confirmations, key choices, forms, or vault creation prompts.
  setTimeout(() => {
    if (!pending && !locked && state?.prompt?.id === prompt.id && draftExecution && !closed) submitPrompt("primary");
  }, 0);
}
function loadingState(title, message) {
  const container = el("div", "", "loading-state");
  container.setAttribute("role", "status");
  container.setAttribute("aria-live", "polite");
  container.append(el("span", "", "loading-spinner"), el("strong", title), el("p", message));
  container.firstChild.setAttribute("aria-hidden", "true");
  return container;
}
function vaultActionLabel(data) {
  return data.operation === "view-vault" ? "View vault" : data.operation === "remake-vault" ? "Remake vault" : "";
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
    if (prompt.kind === "menu") choice.dataset.operationId = option.id || "";
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
  if (isVaultPrompt(prompt)) {
    $("action-content").replaceChildren(loadingState("Vault passphrase required", "Complete the secure dialog to continue setup."));
    $("settings-content").replaceChildren(el("p", "Enter the vault passphrase in the dialog.", "empty-state"));
    $("back-button").hidden = true;
    $("continue-button").hidden = true;
    showVaultPrompt(prompt);
    return;
  }
  if (prompt.kind === "menu") {
    lastMenuOptions = prompt.options || [];
    draftExecution = null;
    if (menuSelectionId !== prompt.id) {
      menuSelectionId = prompt.id;
      menuSelections.clear();
      for (const module of availableModules) {
        menuSelections.set(module.id, new Set(lastMenuOptions.filter(option =>
          option.selected && operationAppliesToModule(option.id, module)).map(option => option.value)));
      }
    }
  }
  const actions = $("action-content");
  const settings = $("settings-content");
  actions.replaceChildren();
  settings.replaceChildren();
  const title = prompt.label || (prompt.kind === "menu" ? "Choose setup tasks" : "Configure setup");
  const container = ["menu", "modules"].includes(prompt.kind) ? actions : settings;
  if (container === settings) actions.append(el("p", "Continue the current setup question in Settings.", "empty-state"));
  if (prompt.kind !== "menu") container.append(el("h2", title, "prompt-title"));
  const card = $("continue-button");
  card.hidden = false;
  $("back-button").hidden = prompt.kind === "menu";
  $("pause-button").hidden = true;
  card.disabled = false;
  card.textContent = "Continue";

  if (prompt.kind === "menu" || prompt.kind === "modules") {
    if (prompt.kind === "modules") container.append(el("p",
      "Choose the modules and apps this repository should configure. Header tabs show module details; use these checkboxes to change the setup selection.", "prompt-help"));
    container.append(renderChoice(prompt, true));
    card.textContent = prompt.kind === "menu" ? "Setup selected tasks" : "Continue";
    container.addEventListener("change", () => {
      updateFooter();
      if (prompt.kind === "modules") syncModuleSelection();
      else { saveMenuSelection(); renderMenuCards(); renderOverview(state); }
    });
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
  highlightModuleChoice();
  renderMenuCards();
  renderSettingsGroups(state);
  updateFooter();
  startCountdown(prompt);
  schedulePreparedAnswer(prompt);
  requireResumeOnRestore = false;
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
    case "menu": return selectedMenuCards().map(card => Number(card.querySelector('input[type="checkbox"]').value));
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
  const menuModule = prompt.kind === "menu" && action === "primary" ? activeModule : null;
  if (prompt.kind === "menu" && !menuModule) draftExecution = null;
  const selectedActions = menuModule ? selectedMenuCards().map(card => card.dataset.operationId) : [];
  if (menuModule && !validatePreparedPasswords(menuModule, selectedActions)) return;
  if (prompt.countdownSeconds && !prompt.secret) rememberPausedPrompt(prompt.id);
  stopCountdown();
  pending = true;
  updateFooter();
  if (menuModule) draftExecution = {module: menuModule, answers: preparedAnswers(menuModule, selectedActions), used: new Set()};
  try {
    await request("/api/answer", "POST", action === "cancel" && prompt.kind !== "confirm" ?
      {id: prompt.id, cancel: true} : {id: prompt.id, value, ...(menuModule ? {module: menuModule} : {})});
    forgetPausedPrompt();
    $("footer-status").textContent = "Continuing setup…";
    await refresh();
  } catch (error) {
    if (menuModule) draftExecution = null;
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
  }
  closeVaultDialog();
  obscureLockedWorkspace(false);
  forgetSession();
  session = "";
  csrf = "";
  closed = true;
  closingRequested = false;
  stopCountdown();
  clearPreparedValues();
  lastMenuOptions = null;
  menuSelections.clear();
  menuSelectionId = null;
  $("action-content").replaceChildren(el("p", "The local wizard is closed. You may close this tab.", "empty-state"));
  $("footer-status").textContent = "Session closed after cleanup";
  $("back-button").hidden = true;
  $("continue-button").hidden = true;
  $("close-button").disabled = true;
  syncMainMenu();
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
  syncMainMenu();
  if (closed || closingRequested || state?.status === "closing" || state?.status === "locked") {
    primary.disabled = true;
    $("back-button").hidden = true;
    $("pause-button").hidden = true;
    $("close-button").disabled = true;
    return;
  }
  $("close-button").disabled = !session;
  if (pending) { primary.disabled = true; return; }
  if (["idle", "finished", "error"].includes(state?.status)) {
    primary.hidden = false;
    primary.textContent = state.status === "idle" || vaultActionLabel(state) ? "Start setup" : "Run setup again";
    primary.disabled = false;
    $("back-button").hidden = true;
    return;
  }
  if (!prompt) { primary.hidden = true; $("back-button").hidden = true; return; }
  if (isVaultPrompt(prompt)) { primary.hidden = true; $("back-button").hidden = true; return; }
  primary.hidden = false;
  const container = ["menu", "modules"].includes(prompt.kind) ? $("action-content") : $("settings-content");
  const choices = prompt.kind === "menu" ? selectedMenuCards().length : container.querySelectorAll('input[type="checkbox"]:checked').length;
  primary.disabled = ["menu", "modules"].includes(prompt.kind) && choices === 0 ||
    prompt.kind === "confirm" && !container.querySelector('.confirmation input')?.checked ||
    prompt.kind === "key" && !container.querySelector('input[name="signing-key"]:checked');
}

function renderProgress(data) {
  const vaultLabel = vaultActionLabel(data);
  if (vaultLabel) {
    const item = el("li", vaultLabel, data.status === "finished" ? "complete" : data.status === "error" ? "failed" : "current");
    if (data.status === "running") item.setAttribute("aria-current", "step");
    $("phase-list").replaceChildren(item);
    return;
  }
  const phases = [
    ["repository", "Inspect repository"], ["actions", "Choose tasks"], ["configure", "Configure"],
    ["review", "Review changes"], ["apply", "Apply setup"], ["complete", "Finish"],
  ];
  const phase = String(data.phase || "").toLowerCase();
  const current = phase.includes("module") ? 0 : phase.includes("action") || phase.includes("menu") ? 1 :
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
  if (availableModules.length) {
    const selected = availableModules.filter(module => module.selected).map(module => module.path);
    parts.push(["Modules", selected.length ? selected.join(", ") : "No modules selected"]);
  }
  const focused = availableModules.find(module => module.id === activeModule);
  if (focused) {
    parts.push(["Focused module", focused.path], ["Type", moduleKind(focused.kind)],
      ["Directory", focused.directory || "."], ["Setup selection", focused.selected ? "Included" : "Not selected"]);
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
  document.title = "Setup: " + (repository.split("/").at(-1) || "Android repository");
}
function moduleKind(kind) {
  return ({library: "Android library", application: "Android app", "jvm-library": "JVM library"})[kind] || "Module";
}
function moduleDescription(module) {
  return `${module.path} — ${moduleKind(module.kind)} · ${module.directory || "."} · ${module.selected ? "Included in setup" : "Not selected for setup"}`;
}
function moduleSetupAreas(module) {
  return module.kind === "application" ? "APK signing, local signing, signing environment and workflows" :
    "Publishing environments and workflows";
}
function operationAppliesToModule(id, module) {
  if (!module?.selected) return false;
  if (["key", "local", "github"].includes(id)) return module.kind === "application";
  if (id === "publishing") return ["library", "jvm-library"].includes(module.kind);
  return id === "workflows";
}
function saveMenuSelection() {
  if (state?.prompt?.kind !== "menu" || !activeModule) return;
  menuSelections.set(activeModule, new Set(selectedMenuCards().map(card => card.querySelector("input").value)));
}
function renderMenuCards() {
  if (state?.prompt?.kind !== "menu") return;
  const module = availableModules.find(item => item.id === activeModule);
  const cards = [...$("action-content").querySelectorAll(".action-card")];
  const selectedValues = menuSelections.get(activeModule);
  for (const card of cards) {
    card.hidden = !operationAppliesToModule(card.dataset.operationId, module);
    if (selectedValues) card.querySelector("input").checked = selectedValues.has(card.querySelector("input").value);
  }
}
function renderModuleDetail() {
  if (selectedDetail) return;
  const module = availableModules.find(item => item.id === activeModule);
  const artifacts = Array.isArray(state?.moduleArtifacts?.[activeModule]) ? state.moduleArtifacts[activeModule] : [];
  const output = artifacts.find(item => item.kind === "output") || artifacts.find(item => item.status === "not built") || artifacts[0];
  $("detail-content").replaceChildren(module ?
    el("h3", module.path) : el("h3", "Repository setup"),
    el("p", module ? `${moduleKind(module.kind)} · ${module.directory || "."}\n${module.selected ? "Included in repository setup." : "Not selected for repository setup."}\n${output ? `Artifact: ${output.path} (${output.status || "available"}).\n` : ""}Relevant setup areas: ${moduleSetupAreas(module)}.` :
      "Gradle discovery will show each supported module in a tab with its setup actions."));
}
function markModuleTabs() {
  for (const tab of $("module-tabs").children) {
    const selected = tab.dataset.moduleId === activeModule;
    if (selected) tab.setAttribute("aria-current", "true");
    else tab.removeAttribute("aria-current");
    const module = availableModules.find(item => item.id === tab.dataset.moduleId);
    if (module) {
      tab.title = moduleDescription(module);
      tab.setAttribute("aria-label", moduleDescription(module));
      tab.classList.toggle("unselected", !module.selected);
    }
  }
}
function selectModule(id, focus = false) {
  if (state?.status === "running" && state?.prompt?.kind !== "menu") return;
  if (!availableModules.some(module => module.id === id)) return;
  activeModule = id;
  try { sessionStorage.setItem(moduleFocusKey, id); } catch { /* Keep module focus in memory. */ }
  selectedDetail = null;
  markModuleTabs();
  if (focus) [...$("module-tabs").children].find(tab => (tab.dataset.moduleId || null) === id)?.focus();
  if (state) {
    renderContext(state);
    renderOverview(state);
  }
  highlightModuleChoice();
  renderMenuCards();
  renderSettingsGroups(state);
  if (state) renderArtifacts(state);
  updateFooter();
  renderModuleDetail();
}
function highlightModuleChoice() {
  const options = state?.prompt?.kind === "modules" ? state.prompt.options || [] : [];
  const option = options.find(item => item.id === activeModule);
  for (const input of $("action-content").querySelectorAll('input[name="setup-choice"]')) {
    const focused = Boolean(option && input.value === option.value);
    input.closest(".action-card")?.classList.toggle("focused-module", focused);
    if (focused) input.closest(".action-card")?.scrollIntoView({block: "nearest"});
  }
}
function renderModuleTabs(data) {
  const source = data.prompt?.kind === "modules" ? data.prompt.options :
    data.phase === "modules" && data.status === "running" ? [] : data.context?.modules;
  const modules = (Array.isArray(source) ? source : []).filter(module =>
    module && typeof (module.id || module.name) === "string" && typeof module.path === "string")
    .map(module => ({id: module.id || module.name, path: module.path, kind: module.kind,
      directory: module.directory, selected: module.selected !== false, value: module.value}));
  const key = JSON.stringify(modules);
  if (key === moduleTabsKey) return;
  moduleTabsKey = key;
  availableModules = modules;
  if (!modules.some(module => module.id === activeModule) ||
      data.prompt?.kind === "menu" && !modules.find(module => module.id === activeModule)?.selected) {
    activeModule = modules.find(module => module.selected)?.id || modules[0]?.id || null;
  }
  const nav = $("module-tabs");
  const focused = nav.contains(document.activeElement) ? document.activeElement.dataset.moduleId : undefined;
  nav.hidden = modules.length === 0;
  nav.replaceChildren(...modules.map(module => {
    const tab = el("button", module.path, "module-tab");
    tab.type = "button";
    tab.dataset.moduleId = module.id;
    return tab;
  }));
  markModuleTabs();
  if (focused !== undefined) [...nav.children].find(tab => tab.dataset.moduleId === focused)?.focus();
  if (!selectedDetail) renderModuleDetail();
}
function syncModuleSelection() {
  if (state?.prompt?.kind !== "modules") return;
  const selected = new Set([...$("action-content").querySelectorAll('input[name="setup-choice"]:checked')].map(input => input.value));
  for (const module of availableModules) module.selected = selected.has(module.value);
  markModuleTabs();
  renderContext(state);
  renderOverview(state);
  renderModuleDetail();
}
$("module-tabs").addEventListener("click", event => {
  const tab = event.target.closest(".module-tab");
  if (tab && $("module-tabs").contains(tab)) selectModule(tab.dataset.moduleId);
});
$("module-tabs").addEventListener("keydown", event => {
  const tabs = [...$("module-tabs").children];
  const current = tabs.indexOf(document.activeElement);
  if (current < 0) return;
  const next = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 :
    event.key === "ArrowLeft" ? (current + tabs.length - 1) % tabs.length :
    event.key === "ArrowRight" ? (current + 1) % tabs.length : -1;
  if (next >= 0) { event.preventDefault(); selectModule(tabs[next].dataset.moduleId, true); }
});
function renderArtifacts(data) {
  const module = availableModules.find(item => item.id === activeModule);
  const artifacts = Array.isArray(data.moduleArtifacts?.[activeModule]) ? data.moduleArtifacts[activeModule] : [];
  const signature = JSON.stringify([activeModule, artifacts]);
  if (signature === artifactSignature) return;
  artifactSignature = signature;
  $("artifact-count").textContent = String(artifacts.length);
  $("artifact-heading").textContent = module ? `Artifacts · ${module.path}` : "Module artifacts";
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
  if (!list.childElementCount) list.append(el("p", artifacts.length ? "No matching artifacts." : module ? "No artifacts discovered for this module." : "Module artifacts appear after discovery.", "empty-state"));
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
  const vaultLabel = vaultActionLabel(data);
  add(vaultLabel ? "Vault action" : "Current step", data.prompt?.label || vaultLabel || data.phase || "Ready to start");
  const module = availableModules.find(item => item.id === activeModule);
  if (module) {
    add("Focused module", `${module.path} · ${moduleKind(module.kind)}\n${module.directory || "."}`);
    add("Setup selection", module.selected ? "Included in repository setup." : "Not selected for repository setup. Run android-repo-setup select-modules to include it.");
    add("Relevant setup areas", moduleSetupAreas(module));
  }
  const selectedValues = data.prompt?.kind === "menu" && promptId === data.prompt.id ?
    new Set(selectedMenuCards().map(card => card.querySelector('input[type="checkbox"]').value)) : null;
  const selected = data.prompt?.kind === "menu" ? (data.prompt.options || [])
    .filter(option => selectedValues ? selectedValues.has(option.value) : option.selected && operationAppliesToModule(option.id, module))
    .map(option => option.label) : [];
  if (selected.length) add("Selected setup tasks", selected.join("\n"));
  if (data.status === "error") add(vaultLabel ? `${vaultLabel} needs attention` : "Setup needs attention", (data.notes || []).at(-1) || "Review process output for recovery steps.", true);
  if (data.status === "finished") add(vaultLabel ? `${vaultLabel} finished` : "Setup finished", vaultLabel ? "Review the result in Process output." : "Review the generated files and the process output.");
  add("Module artifacts", `${(data.moduleArtifacts?.[activeModule] || []).length} known`);
}
function renderState(data) {
  state = data;
  const vaultLabel = vaultActionLabel(data);
  if (data.status === "closed") { finishClose(); return; }
  if (vaultWorkerPromptId != null && !isVaultPrompt(data.prompt)) closeVaultDialog();
  if (data.status === "finished" || data.status === "error") { clearPreparedValues(); lastMenuOptions = null; }
  if (data.status === "locked") {
    showLock(data);
    $("footer-status").textContent = data.vaultMode === "create" ? "Create a vault passphrase to begin" :
      data.initial ? "Unlock the vault to begin" : "Wizard locked after 20 minutes of inactivity";
    updateFooter();
    return;
  }
  if (locked) {
    locked = false;
    closeVaultDialog();
    obscureLockedWorkspace(false);
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
  renderModuleTabs(data);
  for (const tab of $("module-tabs").children) {
    tab.disabled = data.status === "running" && data.prompt?.kind !== "menu";
    tab.title = tab.disabled ? "Complete the current setup step before switching modules" :
      moduleDescription(availableModules.find(module => module.id === tab.dataset.moduleId));
  }
  renderContext(data);
  renderProgress(data);
  renderArtifacts(data);
  renderLog(data);
  renderOverview(data);
  const nextKeySignature = data.prompt?.kind === "key" ? JSON.stringify([data.prompt.options, data.prompt.message]) : "";
  const nextPromptId = data.prompt?.id ?? null;
  const promptChanged = nextPromptId !== promptId;
  const statusChanged = data.status !== lastStatus;
  const phaseChanged = data.phase !== lastPhase;
  if (promptChanged && promptId !== undefined) forgetPausedPrompt();
  if (promptChanged || (statusChanged || phaseChanged) && !data.prompt || nextKeySignature !== keySignature && data.prompt?.kind === "key") {
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
      $("settings-content").replaceChildren(el("p", data.status === "running" ? vaultLabel ? "Complete the vault action in the secure dialog or Settings." : "Working… Accepted answers are saved as setup progresses." : "Choose an action to inspect its settings.", "empty-state"));
      if (data.status === "idle") $("action-content").replaceChildren(el("p", "Start the local setup to choose repository tasks.", "empty-state"));
      else if (data.status === "running") $("action-content").replaceChildren(vaultLabel ?
        loadingState(vaultLabel, "The next vault step will appear when it is ready.") : data.phase === "modules" ?
        loadingState("Discovering modules and apps", "Gradle is inspecting this repository. Available actions will appear under each module tab.") :
        loadingState("Preparing setup", "The next setup step will appear when the current operation is ready."));
      else if (data.status === "finished" || data.status === "error") {
        $("action-content").replaceChildren(el("p", vaultLabel ?
          `${vaultLabel} ${data.status === "finished" ? "finished" : "needs attention"}. Review Process output for details.` :
          data.status === "finished" ? "Setup finished. Review the results in Overview and Process output." : "Setup needs attention. Review Process output for recovery steps.", "empty-state"));
        activateTab($(vaultLabel || data.status === "error" ? "tab-output" : "tab-overview"));
      }
    }
  }
  lastStatus = data.status;
  lastPhase = data.phase;
  if (!data.prompt && data.status === "running") $("footer-status").textContent = vaultLabel ? `${vaultLabel} in progress…` : "Working on your repository…";
  else if (!data.prompt && data.status === "idle") $("footer-status").textContent = "Ready to set up this repository";
  else if (!data.prompt && data.status === "finished") $("footer-status").textContent = vaultLabel ? `${vaultLabel} finished; review Process output` : "Setup complete";
  else if (!data.prompt && data.status === "error") $("footer-status").textContent = vaultLabel ? `${vaultLabel} needs attention; review Process output` : "Setup needs attention; review the process output";
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
  if (isVaultPrompt(data.prompt) && !$("vault-dialog").open) showVaultPrompt(data.prompt);
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
      // Reloading this tab always requires the vault passphrase again before
      // restoring its prior setup question.
      await request("/api/lock", "POST", {});
      await refresh();
      poll();
      return;
    } catch (error) {
      if (!error.unauthorized) {
        setProblem("Connection interrupted. Retrying the vault lock…");
        setTimeout(bootstrap, 1000);
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
    // The server starts locked. Discovery begins only after this vault gate.
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
