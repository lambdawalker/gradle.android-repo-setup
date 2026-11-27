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
let disconnected = false;
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
const generatedSigningDrafts = new Map();
const regeneratingPasswords = new Set();
const signingDefaultsStatus = new Map();
const signingDefaultsPending = new Map();
const signingDefaultsTimers = new Map();
let signingDefaultsSequence = 0;
let settingsInputSequence = 0;
let reviewingModule = null;
let reviewingSelection = "";
let draftExecution = null;
function clearPreparedValues() {
  settingsDrafts.clear();
  generatedSigningDrafts.clear();
  regeneratingPasswords.clear();
  signingDefaultsStatus.clear();
  signingDefaultsPending.clear();
  for (const timer of signingDefaultsTimers.values()) clearTimeout(timer);
  signingDefaultsTimers.clear();
  reviewingModule = null;
  reviewingSelection = "";
  draftExecution = null;
  for (const input of $("workspace").querySelectorAll('input[type="password"]')) input.value = "";
  $("vault-credential").value = "";
  $("vault-confirmation").value = "";
}

const actionFields = {
  key: [
    {key: "keystore-path", label: "Keystore path", prompts: ["Keystore path"], help: "Outside the repository. Leave blank to use the saved or suggested path."},
    {key: "signing-key-alias", label: "Signing key alias", prompts: ["Signing key alias"], help: "Verify this alias matches an existing keystore; a new keystore gets an invented alias."},
    {key: "keystore-password", label: "Keystore password", prompts: ["Keystore password"], secret: true, help: "For a new keystore, a 25-word passphrase is generated. Save it in Bitwarden."},
    {key: "key-password", label: "Key password", prompts: ["Key password"], secret: true, help: "For a new keystore, a separate 25-word passphrase is generated. Save it in Bitwarden."},
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
  const ready = Boolean(session) && !pending && !closed && !closingRequested && !locked && !disconnected &&
    (state?.prompt?.kind === "menu" || ["idle", "finished", "error"].includes(state?.status));
  const reason = ready ? "Open vault controls" : locked || state?.status === "locked" ?
    "Unlock the vault to use these controls" : disconnected ?
      "Waiting for the local CLI to reconnect" : "Available while choosing tasks or after setup stops";
  for (const item of vaultMenuItems) {
    item.disabled = !ready;
    item.title = reason;
    item.setAttribute("aria-description", reason);
  }
  const note = $("menu-bitwarden-note");
  note.disabled = !session || pending || closed || closingRequested || locked || disconnected || regeneratingPasswords.size > 0 || state?.status === "locked";
  note.title = note.disabled ? "Unlock the wizard to save filled fields" : "Save filled setup fields to Bitwarden";
  const copy = $("menu-copy-json");
  copy.disabled = !session || pending || closed || closingRequested || locked || disconnected || regeneratingPasswords.size > 0 || state?.status === "locked";
  copy.title = copy.disabled ? "Unlock the wizard to copy setup values" : "Copy form and vault values as JSON";
  $("menu-close-wizard").disabled = !session || closed || closingRequested || locked || disconnected || state?.status === "closing" || state?.status === "locked";
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
    (event.shiftKey ? mainMenuButton : $("module-tabs").querySelector("button:not(:disabled)") || $("theme")).focus();
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
  const hidden = $("workspace").classList.toggle("sidebar-hidden");
  $("menu-toggle-panels").textContent = hidden ? "Show repository panels" : "Hide repository panels";
  mainMenuButton.focus();
});
$("menu-close-wizard").addEventListener("click", () => {
  closeMainMenu();
  $("close-button").click();
});
async function runVaultMenuAction(action) {
  const menuPrompt = state?.prompt?.kind === "menu";
  closeMainMenu();
  if (pending || locked || closed || closingRequested || disconnected || !session) return;
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
  } catch (error) { if (!disconnected) setProblem(error.message); }
  finally { pending = false; updateFooter(); }
}
$("menu-view-vault").addEventListener("click", () => runVaultMenuAction("view-vault"));
$("menu-remake-vault").addEventListener("click", () => runVaultMenuAction("remake-vault"));
$("menu-bitwarden-note").addEventListener("click", openBitwardenDialog);
$("menu-copy-json").addEventListener("click", openCopyJsonDialog);

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

function setFooterStatus(message, tone = "neutral") {
  const footer = $("footer-status");
  $("footer-status-text").textContent = message;
  footer.querySelector("use").setAttribute("href", "#icon-" + ({neutral: "info", warning: "warning", error: "error", success: "check"}[tone] || "info"));
  footer.dataset.tone = tone;
}
function setProblem(message) {
  setFooterStatus(message, "error");
}
function markDisconnected() {
  if (closed || !session) return;
  if (!disconnected) pauseCountdown();
  disconnected = true;
  closeMainMenu();
  setFooterStatus("Connection to the local CLI interrupted. Retrying… Keep the launching terminal open.", "warning");
  $("vault-submit").disabled = true;
  $("vault-cancel").disabled = true;
  updateFooter();
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
  closeBitwardenDialog(true);
  closeCopyJsonDialog(true);
  obscureLockedWorkspace(true);
  pauseCountdown();
  clearPreparedValues();
  vaultWorkerPromptId = null;
  const creating = data.vaultMode === "create";
  openVaultDialog(creating ? "Create local vault" : data.initial ? "Unlock local vault" : "Wizard locked",
    creating ? "Choose a vault passphrase of at least 12 characters. It encrypts saved setup answers on this device." :
      data.initial ? "Enter your vault passphrase before the wizard inspects this repository." :
        "Enter your vault passphrase to continue. The terminal recovery code restores the browser session only; vault actions will still need the passphrase.",
    creating ? "create" : "unlock", creating);
}
async function unlockWizard() {
  if (!locked || pending || disconnected) return;
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
    if (!disconnected) {
      vaultProblem(error.message);
      $("vault-credential").focus();
    }
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
  let response;
  try {
    response = await fetch(path, {method, headers, body: value === null ? undefined : JSON.stringify(value), cache: "no-store"});
  } catch (error) {
    // A POST may have succeeded before the socket failed. Wait for a fresh
    // state snapshot instead of replaying it against a stale prompt.
    // Activity reports are best effort; the next state poll decides whether
    // the actual wizard connection has been interrupted.
    if (path !== "/api/activity") markDisconnected();
    throw error;
  }
  if (response.status === 401 && path !== "/api/unlock") {
    forgetSession();
    closeCopyJsonDialog(true);
    clearPreparedValues();
    session = "";
    csrf = "";
    const error = new Error("The local session expired. Restart the CLI to open a new wizard.");
    error.unauthorized = true;
    throw error;
  }
  if (response.status === 409 && path === "/api/answer") throw new Error("This question changed. Review the current step and try again.");
  if (!response.ok) {
    let body;
    try { body = (await response.text()).trim(); }
    catch (error) { markDisconnected(); throw error; }
    let message = body;
    try { message = JSON.parse(body).error || body; } catch { /* Plain-text error. */ }
    throw new Error(message || "The local wizard is unavailable.");
  }
  if (response.status === 204) return null;
  try { return await response.json(); }
  catch (error) { markDisconnected(); throw error; }
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
  if (locked || pending || disconnected || !isVaultPrompt(prompt) || vaultWorkerPromptId !== prompt.id) return;
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
    if (!disconnected) {
      vaultProblem(error.message);
      $("vault-credential").focus();
    }
  } finally {
    pending = false;
    $("vault-submit").disabled = false;
    updateFooter();
  }
}
$("vault-cancel").addEventListener("click", () => answerVaultPrompt(true));

let lastActivity = 0;
function reportActivity(event) {
  if (!event.isTrusted || !session || closed || closingRequested || locked || disconnected || document.hidden || Date.now() - lastActivity < 1000) return;
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
  setFooterStatus(paused ? "Countdown paused. Review the value and continue when ready." :
    `The displayed default will be used in ${seconds} seconds.`);
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
      if (!item?.name || !item.inputLabel) continue;
      // A GPG identity is chosen and reviewed by the CLI; it is not a text
      // credential that can safely be staged in this form.
      if (["SIGNING_IN_MEMORY_KEY", "SIGNING_IN_MEMORY_KEY_PASSWORD"].includes(item.name)) continue;
      fields.push({key: `${destination.environment}/${item.name}`, label: `${destination.environment} / ${item.name}`,
        prompts: [item.inputLabel], secret: Boolean(item.secret),
        help: item.required ? `Required when this ${item.secret ? "secret" : "variable"} is absent in GitHub.` :
          "Optional. Leave blank to preserve an existing value."});
    }
  }
  return fields;
}
function draftKey(moduleId, actionId, fieldKey) { return `${moduleId}\u0000${actionId}\u0000${fieldKey}`; }
function setSigningDraft(moduleId, fieldKey, value, generated = false, replaceGenerated = false) {
  if (!value) return;
  const key = draftKey(moduleId, "key", fieldKey);
  const previous = settingsDrafts.get(key);
  if (previous && !(replaceGenerated && generatedSigningDrafts.get(key) === previous)) return;
  settingsDrafts.set(key, value);
  if (generated) generatedSigningDrafts.set(key, value);
  for (const input of $("prepared-settings")?.querySelectorAll(".settings-field input[data-draft-key]") || []) {
    if (input.dataset.draftKey === key) input.value = value;
  }
}
function clearGeneratedSigningPassword(moduleId, fieldKey) {
  const key = draftKey(moduleId, "key", fieldKey);
  if (settingsDrafts.get(key) === generatedSigningDrafts.get(key)) {
    settingsDrafts.delete(key);
    for (const input of $("prepared-settings")?.querySelectorAll(".settings-field input[data-draft-key]") || []) {
      if (input.dataset.draftKey === key) input.value = "";
    }
  }
  generatedSigningDrafts.delete(key);
}
function signingDefaultsMessage(moduleId) {
  const status = signingDefaultsStatus.get(moduleId);
  const path = settingsDrafts.get(draftKey(moduleId, "key", "keystore-path")) || "";
  if (!status || status.requestedPath !== path) return "Checking signing defaults for this app…";
  if (status.error) return status.error;
  return status.data.keystoreExists ?
    "Existing keystore: verify the signing alias. Blank passwords reuse matching saved vault values when available." :
    `New keystore: an invented alias and two 25-word passphrases are ready. Review and save them to Bitwarden.${
      activeModule === moduleId && selectedMenuCards().some(card => card.dataset.operationId === "github") &&
        !selectedMenuCards().some(card => card.dataset.operationId === "key") ?
        " Select APK signing key too; GitHub signing alone requires an existing keystore." : ""}`;
}
function updateSigningDefaultsMessage(moduleId) {
  if (activeModule !== moduleId) return;
  const message = $("prepared-settings")?.querySelector(".signing-default-status");
  if (message) {
    message.textContent = signingDefaultsMessage(moduleId);
    message.classList.toggle("signing-default-error", Boolean(signingDefaultsStatus.get(moduleId)?.error));
  }
  const path = settingsDrafts.get(draftKey(moduleId, "key", "keystore-path")) || "";
  const status = signingDefaultsStatus.get(moduleId);
  const canGenerate = Boolean(status?.data && status.requestedPath === path && !status.data.keystoreExists &&
    !locked && !closed && !disconnected && state?.prompt?.kind === "menu");
  for (const button of $("prepared-settings")?.querySelectorAll(".field-regenerate") || []) {
    button.disabled = !canGenerate || regeneratingPasswords.has(draftKey(moduleId, "key", button.dataset.fieldKey));
    button.title = canGenerate ? `Generate a new ${button.dataset.fieldKey === "key-password" ? "key" : "keystore"} passphrase` :
      "Available when creating a new keystore";
  }
}
async function regenerateSigningPassword(moduleId, fieldKey, button) {
  const path = settingsDrafts.get(draftKey(moduleId, "key", "keystore-path")) || "";
  const status = signingDefaultsStatus.get(moduleId);
  const key = draftKey(moduleId, "key", fieldKey);
  if (!path || !status?.data || status.requestedPath !== path || status.data.keystoreExists ||
      regeneratingPasswords.has(key) || locked || closed || disconnected || state?.prompt?.kind !== "menu") return;
  const previous = settingsDrafts.get(key);
  regeneratingPasswords.add(key);
  button.classList.add("regenerating");
  updateSigningDefaultsMessage(moduleId);
  updateFooter();
  try {
    const result = await request("/api/signing-passphrase", "POST", {module: moduleId, field: fieldKey, path});
    if (locked || closed || disconnected || state?.prompt?.kind !== "menu" ||
        settingsDrafts.get(draftKey(moduleId, "key", "keystore-path")) !== path ||
        settingsDrafts.get(key) !== previous || signingDefaultsStatus.get(moduleId) !== status) return;
    const passphrase = result?.passphrase;
    if (typeof passphrase !== "string" || passphrase.trim().split(/\s+/).length !== 25) {
      throw new Error("The new passphrase was incomplete. Try again.");
    }
    settingsDrafts.set(key, passphrase);
    generatedSigningDrafts.set(key, passphrase);
    status.data[fieldKey === "keystore-password" ? "storePassword" : "keyPassword"] = passphrase;
    for (const input of $("prepared-settings")?.querySelectorAll(".settings-field input[data-draft-key]") || []) {
      if (input.dataset.draftKey === key) input.value = passphrase;
    }
    setFooterStatus(`${fieldKey === "key-password" ? "Key" : "Keystore"} passphrase regenerated. Save it before the session locks.`, "success");
  } catch (error) {
    if (!disconnected) setProblem(error.message);
  } finally {
    regeneratingPasswords.delete(key);
    button.classList.remove("regenerating");
    updateSigningDefaultsMessage(moduleId);
    updateFooter();
  }
}
async function ensureSigningDefaults(moduleId, regenerate = false) {
  if (!session || locked || disconnected || closed) return null;
  const path = settingsDrafts.get(draftKey(moduleId, "key", "keystore-path")) || "";
  const cached = signingDefaultsStatus.get(moduleId);
  if (!regenerate && cached?.requestedPath === path && !cached.error) return cached.data;
  const current = signingDefaultsPending.get(moduleId);
  if (!regenerate && current?.path === path) return current.promise;
  const sequence = ++signingDefaultsSequence;
  const promise = (async () => {
    try {
      const data = await request("/api/signing-defaults", "POST", {module: moduleId, ...(path ? {path} : {})});
      if (signingDefaultsPending.get(moduleId)?.sequence !== sequence ||
          (settingsDrafts.get(draftKey(moduleId, "key", "keystore-path")) || "") !== path) return null;
      const resolvedPath = String(data.keystorePath || "");
      // Use the server's resolved absolute path even if the user entered a
      // relative one. The saved-password availability flags describe this
      // exact path, and the worker should receive the same value.
      const pathKey = draftKey(moduleId, "key", "keystore-path");
      if (resolvedPath) {
        settingsDrafts.set(pathKey, resolvedPath);
        if (!path || generatedSigningDrafts.get(pathKey) === path) generatedSigningDrafts.set(pathKey, resolvedPath);
        for (const input of $("prepared-settings")?.querySelectorAll(".settings-field input[data-draft-key]") || []) {
          if (input.dataset.draftKey === pathKey) input.value = resolvedPath;
        }
      }
      const normalized = {
        ...data, keystorePath: resolvedPath,
        applicationIds: Array.isArray(data.applicationIds) ? data.applicationIds.join(", ") : String(data.applicationIds || ""),
      };
      signingDefaultsStatus.set(moduleId, {requestedPath: resolvedPath || path, data: normalized});
      setSigningDraft(moduleId, "signing-key-alias", String(data.alias || ""), true, regenerate || Boolean(cached && cached.requestedPath !== path));
      setSigningDraft(moduleId, "application-ids", normalized.applicationIds, true);
      if (data.keystoreExists) {
        clearGeneratedSigningPassword(moduleId, "keystore-password");
        clearGeneratedSigningPassword(moduleId, "key-password");
      } else {
        const replace = regenerate || Boolean(cached && cached.requestedPath !== path);
        setSigningDraft(moduleId, "keystore-password", String(data.storePassword || ""), true, replace);
        setSigningDraft(moduleId, "key-password", String(data.keyPassword || ""), true, replace);
      }
      updateSigningDefaultsMessage(moduleId);
      return normalized;
    } catch (error) {
      if (signingDefaultsPending.get(moduleId)?.sequence === sequence) {
        signingDefaultsStatus.set(moduleId, {requestedPath: path, error: error.message});
        updateSigningDefaultsMessage(moduleId);
      }
      return null;
    } finally {
      if (signingDefaultsPending.get(moduleId)?.sequence === sequence) signingDefaultsPending.delete(moduleId);
      updateFooter();
    }
  })();
  signingDefaultsPending.set(moduleId, {path, sequence, promise});
  updateFooter();
  return promise;
}
function renderSettingsGroups(data) {
  // Prepared inputs belong to the action menu. During execution the worker
  // supplies a live question in this panel, so no staged form is shown twice.
  if (data?.prompt?.kind !== "menu") return;
  const options = data.prompt.options;
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
  const selectedValues = menuSelections.get(module.id);
  const applicable = options.filter(option => operationAppliesToModule(option.id, module) &&
    (reviewingModule !== module.id || !selectedValues || selectedValues.has(String(option.value))));
  if (!applicable.length) {
    groups.append(el("p", "This module is excluded from setup. Use the terminal module selector to include it.", "empty-state"));
    return;
  }
  const allFields = applicable.map(option => ({option, fields: fieldsForAction(option.id, data)}));
  const byDraftKey = new Map();
  for (const {option, fields} of allFields) {
    for (const item of fields) {
      const key = draftKey(module.id, item.draftAction || option.id, item.key);
      if (!byDraftKey.has(key)) byDraftKey.set(key, {item: {...item, draftAction: item.draftAction || option.id}, actions: new Set()});
      const entry = byDraftKey.get(key);
      entry.actions.add(option.id);
      // Prefer the original action's help over conditional wording from an
      // action that may reuse the same credential.
      if ((item.draftAction || option.id) === option.id) entry.item = {...item, draftAction: option.id};
    }
  }
  const commonFields = [...byDraftKey.values()].filter(entry => entry.actions.size > 1).map(entry => entry.item);
  const withFields = allFields.map(({option, fields}) => {
    const seen = new Set();
    return {option, fields: fields.filter(item => {
      const key = draftKey(module.id, item.draftAction || option.id, item.key);
      if (byDraftKey.get(key).actions.size > 1 || seen.has(key)) return false;
      seen.add(key);
      return true;
    })};
  }).filter(({fields}) => fields.length);
  if (!commonFields.length && !withFields.length) {
    groups.append(el("p", `No advance inputs for ${module.path}. The wizard will ask for confirmations when needed.`, "empty-state"));
    return;
  }
  groups.append(el("p", reviewingModule === module.id ?
    `Review ${module.path} values for the selected tasks. Blank fields reuse matching saved vault values when available; fill any missing values before running. You can export entered and generated values to Bitwarden.` :
    `Prepare ${module.path} values here. Blank fields reuse matching saved vault values when available. Values stay in this tab until the session locks or closes; saved vault values are never shown.`, "settings-note"));
  if (module.kind === "application" && applicable.some(({id}) => id === "key" || id === "github")) {
    const status = el("p", signingDefaultsMessage(module.id), "settings-note signing-default-status");
    status.classList.toggle("signing-default-error", Boolean(signingDefaultsStatus.get(module.id)?.error));
    groups.append(status);
  }
  const sections = commonFields.length ? [
    {option: {id: "common", label: "Common fields", description: "Used by multiple setup actions for this module."}, fields: commonFields},
    ...withFields,
  ] : withFields;
  for (const {option, fields} of sections) {
    const group = el("section", "", "settings-group");
    group.dataset.operationId = option.id;
    group.append(el("h2", option.label), el("p", actionNotes[option.id] || option.description || ""));
    if (option.id === "publishing" && data.context?.publishing?.registryPresent === false) {
      group.append(el("p", "Add .project/setup/repositories.yml before configuring publishing environments.", "problem"));
    }
    if (option.id === "publishing" && data.context?.publishing?.error) {
      group.append(el("p", `Publishing configuration needs attention: ${data.context.publishing.error}`, "problem"));
    }
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
      input.id = `settings-input-${++settingsInputSequence}`;
      input.value = settingsDrafts.get(key) || "";
      input.placeholder = "Leave blank to review when requested";
      const label = el("label", item.label);
      label.htmlFor = input.id;
      const shell = el("div", "", "field-input-shell");
      shell.append(input);
      wrapper.append(label, shell);
      if (item.help) wrapper.append(el("small", item.help));
      input.addEventListener("input", () => {
        if (input.value) settingsDrafts.set(key, input.value);
        else settingsDrafts.delete(key);
        generatedSigningDrafts.delete(key);
        for (const other of groups.querySelectorAll(".settings-field input")) {
          if (other !== input && other.dataset.draftKey === key) other.value = input.value;
        }
        if (item.key === "keystore-path") {
          updateSigningDefaultsMessage(module.id);
          clearTimeout(signingDefaultsTimers.get(module.id));
          signingDefaultsTimers.set(module.id, setTimeout(() => ensureSigningDefaults(module.id), 350));
        }
      });
      if (item.secret) {
        shell.classList.add("has-tools");
        const tools = el("div", "", "field-input-tools");
        const reveal = el("button", "Show", "field-reveal");
        reveal.type = "button";
        reveal.setAttribute("aria-label", `Show ${item.label}`);
        reveal.addEventListener("click", () => {
          const visible = input.type === "password";
          input.type = visible ? "text" : "password";
          reveal.textContent = visible ? "Hide" : "Show";
          reveal.setAttribute("aria-label", `${visible ? "Hide" : "Show"} ${item.label}`);
        });
        tools.append(reveal);
        if (["keystore-password", "key-password"].includes(item.key) && module.kind === "application") {
          shell.classList.add("has-regenerate");
          const regenerate = el("button", "", "field-regenerate");
          regenerate.type = "button";
          regenerate.dataset.fieldKey = item.key;
          regenerate.setAttribute("aria-label", `Regenerate ${item.label.toLowerCase()}`);
          const icon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
          icon.classList.add("icon");
          icon.setAttribute("viewBox", "0 0 24 24");
          icon.setAttribute("aria-hidden", "true");
          const stroke = document.createElementNS(icon.namespaceURI, "path");
          stroke.setAttribute("d", "M20 7v5h-5M4 17v-5h5M5.5 9A7 7 0 0 1 18 7l2 5M4 12l2 5a7 7 0 0 0 12.5-2");
          icon.append(stroke);
          regenerate.append(icon);
          regenerate.addEventListener("click", () => regenerateSigningPassword(module.id, item.key, regenerate));
          tools.append(regenerate);
        }
        shell.append(tools);
      }
      list.append(wrapper);
    }
    group.append(list);
    groups.append(group);
  }
  if (module.kind === "application" && applicable.some(({id}) => id === "key" || id === "github")) {
    updateSigningDefaultsMessage(module.id);
    void ensureSigningDefaults(module.id);
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
      if (value) {
        for (const promptLabel of item.prompts || []) answers.set(promptLabel, value);
        // One password input is enough. The CLI still asks for a confirmation
        // when creating/changing the key, so supply the same staged value.
        if (item.key === "keystore-password") answers.set("Confirm keystore password", value);
        if (item.key === "key-password") answers.set("Confirm key password", value);
      }
    }
  }
  return answers;
}
function filledBitwardenEntries() {
  const labels = new Map();
  const options = lastMenuOptions || Object.keys(actionNotes).map(id => ({id, label: id}));
  for (const module of availableModules) {
    for (const option of options) {
      if (!operationAppliesToModule(option.id, module)) continue;
      for (const item of fieldsForAction(option.id, state)) {
        const key = draftKey(module.id, item.draftAction || option.id, item.key);
        if (!labels.has(key)) labels.set(key, {module: module.path, label: item.label, action: option.label, actions: new Set()});
        const metadata = labels.get(key);
        metadata.actions.add(option.id);
        if ((item.draftAction || option.id) === option.id) metadata.action = option.label;
      }
    }
  }
  const entries = [];
  const seen = new Map();
  const append = entry => {
    const key = `${entry.module}\u0000${entry.label.toLowerCase().trim()}`;
    if (seen.has(key)) entries[seen.get(key)] = entry;
    else { seen.set(key, entries.length); entries.push(entry); }
  };
  for (const [key, value] of settingsDrafts) {
    if (!value) continue;
    const [moduleId, actionId, fieldKey] = key.split("\u0000");
    if (fieldKey?.startsWith("confirm-")) continue;
    const metadata = labels.get(key);
    const module = availableModules.find(item => item.id === moduleId);
    append({module: metadata?.module || module?.path || moduleId,
      action: metadata?.actions.size > 1 ? "Common fields" : metadata?.action || actionId,
      label: metadata?.label || fieldKey, value});
  }
  const prompt = state?.prompt;
  if ((prompt?.kind === "text" || prompt?.kind === "form") && !isVaultPrompt(prompt)) {
    const inputs = [...$("settings-content").querySelectorAll(".form-field input")];
    for (const [index, input] of inputs.entries()) {
      const label = prompt.kind === "form" ? prompt.fields?.[index]?.label : prompt.label;
      if (!input.value || !label || /^confirm(?:ation)?\b/i.test(label)) continue;
      append({module: availableModules.find(item => item.id === activeModule)?.path || "Repository",
        action: "Current setup question", label, value: input.value});
    }
  }
  return entries;
}

let bitwardenBusy = false;
let bitwardenAccount = "";
let bitwardenCreated = false;
function bitwardenProblem(message) {
  $("bitwarden-error").textContent = message;
  $("bitwarden-error").hidden = !message;
}
function bitwardenControls() {
  $("bitwarden-copy-json").disabled = bitwardenBusy;
  $("bitwarden-cancel").disabled = bitwardenBusy;
  $("bitwarden-check").disabled = bitwardenBusy || bitwardenCreated;
  $("bitwarden-create").disabled = bitwardenBusy || bitwardenCreated || !bitwardenAccount;
  $("bitwarden-note-name").disabled = bitwardenBusy;
  $("bitwarden-session-key").disabled = bitwardenBusy;
}
function closeBitwardenDialog(force = false) {
  if (bitwardenBusy && !force) return;
  const dialog = $("bitwarden-dialog");
  if (dialog.open) dialog.close();
  $("bitwarden-session-key").value = "";
  bitwardenAccount = "";
  bitwardenCreated = false;
  bitwardenProblem("");
}
function openBitwardenDialog() {
  closeMainMenu();
  if (!session || pending || closed || closingRequested || locked || disconnected || bitwardenBusy) return;
  const entries = filledBitwardenEntries();
  if (!entries.length) {
    setFooterStatus("Enter setup values in Settings before creating a Bitwarden note", "warning");
    return;
  }
  bitwardenAccount = "";
  bitwardenCreated = false;
  $("bitwarden-cancel").textContent = "Cancel";
  $("bitwarden-check").hidden = false;
  $("bitwarden-create").hidden = false;
  $("bitwarden-note-name").value = `Android setup · ${state?.context?.repository || "repository"}`;
  $("bitwarden-session-key").value = "";
  $("bitwarden-count").textContent = `${entries.length} filled field${entries.length === 1 ? "" : "s"} across module tabs.`;
  $("bitwarden-account").textContent = "Check the target account before creating the note.";
  bitwardenProblem("");
  bitwardenControls();
  $("bitwarden-dialog").showModal();
  $("bitwarden-note-name").focus();
}
$("bitwarden-dialog").addEventListener("cancel", event => {
  event.preventDefault();
  closeBitwardenDialog();
});
$("bitwarden-cancel").addEventListener("click", () => closeBitwardenDialog());
$("bitwarden-copy-json").addEventListener("click", () => {
  if (bitwardenBusy) return;
  closeBitwardenDialog(); // Clears any pasted BW_SESSION before opening the export dialog.
  openCopyJsonDialog();
});
$("bitwarden-session-key").addEventListener("input", () => {
  bitwardenAccount = "";
  $("bitwarden-account").textContent = "Check the target account again after changing the session key.";
  bitwardenControls();
});
$("bitwarden-check").addEventListener("click", async () => {
  if (bitwardenBusy || bitwardenCreated || locked || disconnected) return;
  bitwardenBusy = true;
  bitwardenProblem("");
  $("bitwarden-account").textContent = "Checking Bitwarden CLI account…";
  bitwardenControls();
  try {
    const result = await request("/api/bitwarden-status", "POST", {sessionKey: $("bitwarden-session-key").value});
    if (!$("bitwarden-dialog").open || locked || closed) return;
    if (result.status !== "unlocked" || !result.email) throw new Error("Unlock the Bitwarden CLI and check the account again.");
    bitwardenAccount = result.email;
    $("bitwarden-account").textContent = `Target Bitwarden account: ${result.email}`;
  } catch (error) {
    if (!session) closeBitwardenDialog(true);
    else if ($("bitwarden-dialog").open && !locked && !closed) {
      $("bitwarden-account").textContent = "Account not verified.";
      bitwardenProblem(error.message);
    }
  } finally {
    bitwardenBusy = false;
    if ($("bitwarden-dialog").open) bitwardenControls();
  }
});
$("bitwarden-form").addEventListener("submit", async event => {
  event.preventDefault();
  if (bitwardenBusy || bitwardenCreated || !bitwardenAccount || locked || disconnected) return;
  const name = $("bitwarden-note-name").value.trim();
  const entries = filledBitwardenEntries();
  if (!name || !entries.length) {
    bitwardenProblem(entries.length ? "Enter a note name." : "The filled fields were cleared. Enter them again before creating a note.");
    return;
  }
  bitwardenBusy = true;
  bitwardenProblem("");
  const expectedEmail = bitwardenAccount;
  $("bitwarden-account").textContent = `Creating note in ${expectedEmail}…`;
  bitwardenControls();
  try {
    const result = await request("/api/bitwarden-note", "POST", {
      sessionKey: $("bitwarden-session-key").value, expectedEmail, name, entries,
    });
    if (!$("bitwarden-dialog").open || locked || closed) return;
    $("bitwarden-session-key").value = "";
    $("bitwarden-account").textContent = `Created “${result.name || name}” in ${expectedEmail}${result.id ? ` (item ${result.id})` : ""}.`;
    bitwardenCreated = true;
    $("bitwarden-cancel").textContent = "Close";
    $("bitwarden-check").hidden = true;
    $("bitwarden-create").hidden = true;
    setFooterStatus("Bitwarden secure note created", "success");
  } catch (error) {
    if (!session) closeBitwardenDialog(true);
    else if ($("bitwarden-dialog").open && !locked && !closed) {
      $("bitwarden-account").textContent = `Target Bitwarden account: ${expectedEmail}`;
      bitwardenProblem(disconnected ? "Connection interrupted. Check Bitwarden for the note before retrying." : error.message);
    }
  } finally {
    bitwardenBusy = false;
    if ($("bitwarden-dialog").open) bitwardenControls();
  }
});

let copyJsonBusy = false;
let preparedCopyJson = null;
function copyJsonProblem(message) {
  $("copy-json-error").textContent = message;
  $("copy-json-error").hidden = !message;
}
function copyJsonControls() {
  $("copy-json-cancel").disabled = copyJsonBusy;
  $("copy-json-submit").disabled = copyJsonBusy;
}
function closeCopyJsonDialog(force = false) {
  if (copyJsonBusy && !force) return;
  if ($("copy-json-dialog").open) $("copy-json-dialog").close();
  $("copy-json-credential").value = "";
  preparedCopyJson = null;
  copyJsonProblem("");
}
function openCopyJsonDialog() {
  closeMainMenu();
  if (!session || pending || closed || closingRequested || locked || disconnected || regeneratingPasswords.size > 0 || copyJsonBusy) return;
  preparedCopyJson = null;
  $("copy-json-credential").value = "";
  $("copy-json-credential").disabled = false;
  $("copy-json-credential").required = true;
  $("copy-json-credential-group").hidden = false;
  $("copy-json-ready").hidden = true;
  $("copy-json-submit").textContent = "Verify passphrase";
  const filled = filledBitwardenEntries().length;
  $("copy-json-count").textContent = `${filled} filled field${filled === 1 ? "" : "s"} across module tabs; saved vault values are added after verification.`;
  copyJsonProblem("");
  copyJsonControls();
  $("copy-json-dialog").showModal();
  $("copy-json-credential").focus();
}
function mergedSetupJsonEntries(saved, current) {
  if (!Array.isArray(saved)) throw new Error("The vault returned an invalid export.");
  const fieldName = entry => String(entry.label || "").trim().toLocaleLowerCase("en-US");
  const enteredLabels = new Set(current.map(fieldName));
  const rows = new Map();
  const insert = entry => {
    if (!entry || typeof entry.module !== "string" || typeof entry.action !== "string" ||
        typeof entry.label !== "string" || typeof entry.value !== "string" || !entry.label || !entry.value) return;
    rows.set(`${entry.module}\u0000${fieldName(entry)}`, {
      module: entry.module, action: entry.action, label: entry.label, value: entry.value,
    });
  };
  for (const entry of saved) {
    // Vault signing and publishing values apply to the repository. A filled
    // module field is the explicit replacement for the same shared value.
    if (entry.module === "Repository" && enteredLabels.has(fieldName(entry))) continue;
    insert(entry);
  }
  for (const entry of current) insert(entry);
  return [...rows.values()].sort((a, b) =>
    a.module.localeCompare(b.module) || a.label.localeCompare(b.label));
}
$("copy-json-dialog").addEventListener("cancel", event => {
  event.preventDefault();
  closeCopyJsonDialog();
});
$("copy-json-cancel").addEventListener("click", () => closeCopyJsonDialog());
$("copy-json-form").addEventListener("submit", async event => {
  event.preventDefault();
  if (copyJsonBusy || !session || locked || closed || disconnected) return;
  const credential = preparedCopyJson ? null : $("copy-json-credential").value;
  $("copy-json-credential").value = "";
  if (!preparedCopyJson && !credential) {
    copyJsonProblem("Enter the vault passphrase.");
    $("copy-json-credential").focus();
    return;
  }
  copyJsonBusy = true;
  copyJsonProblem("");
  copyJsonControls();
  try {
    if (!preparedCopyJson) {
      const response = await request("/api/export-json", "POST", {credential});
      if (!$('copy-json-dialog').open || locked || closed || disconnected) return;
      const fields = mergedSetupJsonEntries(response.saved, filledBitwardenEntries());
      if (!fields.length) throw new Error("There are no filled or saved setup values to copy.");
      preparedCopyJson = JSON.stringify({repository: state?.context?.repository || "", fields}, null, 2);
      $("copy-json-credential-group").hidden = true;
      $("copy-json-credential").required = false;
      $("copy-json-credential").disabled = true;
      $("copy-json-ready").hidden = false;
      $("copy-json-submit").textContent = "Copy JSON";
      $("copy-json-count").textContent = `${fields.length} setup field${fields.length === 1 ? "" : "s"} ready to copy.`;
      return;
    }
    // A fresh click is required for reliable clipboard permissions after the
    // vault decryption request. The JSON lives only in this open dialog.
    await navigator.clipboard.writeText(preparedCopyJson);
    if (!$('copy-json-dialog').open || locked || closed) return;
    closeCopyJsonDialog(true);
    setFooterStatus("Setup JSON copied to clipboard", "success");
  } catch (error) {
    if (!session) closeCopyJsonDialog(true);
    else if ($('copy-json-dialog').open && !locked && !closed) {
      copyJsonProblem(preparedCopyJson ? "Clipboard access was blocked. Choose Copy JSON again and allow access." : error.message);
      if (!preparedCopyJson) $("copy-json-credential").focus();
    }
  } finally {
    copyJsonBusy = false;
    if ($('copy-json-dialog').open) copyJsonControls();
  }
});
function validatePreparedPasswords(moduleId, selectedActions) {
  if (!selectedActions.includes("key") && !selectedActions.includes("github")) return true;
  for (const [password, label] of [
    ["keystore-password", "keystore"],
    ["key-password", "key"],
  ]) {
    const entered = settingsDrafts.get(draftKey(moduleId, "key", password));
    if (entered && (Array.from(entered).length < 6 || /[\x00-\x1f\x7f]/.test(entered))) {
      setProblem(`The ${label} password needs at least six characters and no control characters.`);
      activateTab($("tab-settings"));
      return false;
    }
  }
  return true;
}
function selectedMenuSignature() {
  return selectedMenuCards().map(card => card.querySelector("input").value).sort().join(",");
}
function focusPreparedField(moduleId, actionId, fieldKey, message) {
  activateTab($("tab-settings"));
  setFooterStatus(message, "warning");
  const key = draftKey(moduleId, actionId, fieldKey);
  const input = [...$("prepared-settings")?.querySelectorAll(".settings-field input[data-draft-key]") || []]
    .find(item => item.dataset.draftKey === key);
  input?.focus();
  return false;
}
function validateSelectedForm(moduleId, selectedActions) {
  if (!validatePreparedPasswords(moduleId, selectedActions)) return false;
  const has = (action, key) => Boolean(settingsDrafts.get(draftKey(moduleId, action, key))?.trim());
  const data = signingDefaultsStatus.get(moduleId)?.data;
  if (selectedActions.includes("github") && !has("github", "github-token") && !data?.savedGitHubToken) {
    return focusPreparedField(moduleId, "github", "github-token", "Enter a GitHub token or use a saved vault token.");
  }
  if (!selectedActions.includes("key")) return true;
  if (!data) {
    return focusPreparedField(moduleId, "key", "keystore-path", "Signing defaults are not ready. Review the keystore path and try again.");
  }
  if (!has("key", "keystore-path")) return focusPreparedField(moduleId, "key", "keystore-path", "Enter a keystore path outside this repository.");
  if (!has("key", "signing-key-alias")) return focusPreparedField(moduleId, "key", "signing-key-alias", "Enter a signing key alias.");
  const pathMatches = settingsDrafts.get(draftKey(moduleId, "key", "keystore-path")) === data.keystorePath;
  const aliasMatches = settingsDrafts.get(draftKey(moduleId, "key", "signing-key-alias")) === data.alias;
  if (!has("key", "keystore-password") && !(data.keystoreExists && pathMatches && aliasMatches && data.savedStorePassword)) {
    return focusPreparedField(moduleId, "key", "keystore-password", "Enter the keystore passphrase or use a matching saved vault value.");
  }
  if (!has("key", "key-password") && !(data.keystoreExists && pathMatches && aliasMatches && data.savedKeyPassword)) {
    return focusPreparedField(moduleId, "key", "key-password", "Enter the key passphrase or use a matching saved vault value.");
  }
  if (!has("key", "application-ids")) return focusPreparedField(moduleId, "key", "application-ids", "Enter at least one Android application ID.");
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
      reviewingModule = null;
      reviewingSelection = "";
      menuSelections.clear();
      for (const module of availableModules) {
        menuSelections.set(module.id, new Set(lastMenuOptions.filter(option =>
          option.selected && operationAppliesToModule(option.id, module)).map(option => String(option.value))));
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
  // A single text input already labels itself with the question.
  if (prompt.kind !== "menu" && prompt.kind !== "text") container.append(el("h2", title, "prompt-title"));
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
      else {
        saveMenuSelection();
        reviewingModule = null;
        reviewingSelection = "";
        renderMenuCards();
        renderSettingsGroups(state);
        renderOverview(state);
        updateFooter();
      }
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
  if (!prompt || pending || closed || disconnected) return;
  const value = action === "cancel" && prompt.kind === "confirm" ? false : promptValue(prompt, action);
  if (action === "primary" && ((["menu", "modules"].includes(prompt.kind) && !value.length) ||
      (prompt.kind === "key" && !value) || (prompt.kind === "confirm" && !value))) return;
  const menuModule = prompt.kind === "menu" && action === "primary" ? activeModule : null;
  if (prompt.kind === "menu" && !menuModule) draftExecution = null;
  const selectedActions = menuModule ? selectedMenuCards().map(card => card.dataset.operationId) : [];
  const needsForm = menuModule && selectedActions.some(id => fieldsForAction(id, state).length);
  if (needsForm && (reviewingModule !== menuModule || reviewingSelection !== selectedMenuSignature())) {
    saveMenuSelection();
    reviewingModule = menuModule;
    reviewingSelection = selectedMenuSignature();
    renderSettingsGroups(state);
    activateTab($("tab-settings"));
    setFooterStatus("Review all selected setup fields, then run the tasks.");
    updateFooter();
    return;
  }
  if (needsForm && (selectedActions.includes("key") || selectedActions.includes("github"))) {
    pending = true;
    updateFooter();
    let defaults;
    try { defaults = await ensureSigningDefaults(menuModule); }
    finally { pending = false; updateFooter(); }
    if (!defaults) {
      return focusPreparedField(menuModule, "key", "keystore-path", "Could not inspect signing defaults. Check the keystore path and try again.");
    }
  }
  if (needsForm && !validateSelectedForm(menuModule, selectedActions)) return;
  if (prompt.countdownSeconds && !prompt.secret) rememberPausedPrompt(prompt.id);
  stopCountdown();
  pending = true;
  updateFooter();
  if (menuModule) draftExecution = {module: menuModule, answers: preparedAnswers(menuModule, selectedActions), used: new Set()};
  try {
    await request("/api/answer", "POST", action === "cancel" && prompt.kind !== "confirm" ?
      {id: prompt.id, cancel: true} : {id: prompt.id, value, ...(menuModule ? {
        module: menuModule, prepared: Object.fromEntries(draftExecution.answers),
      } : {})});
    forgetPausedPrompt();
    setFooterStatus("Continuing setup…");
    await refresh();
  } catch (error) {
    if (menuModule) draftExecution = null;
    if (!disconnected) setProblem(error.message);
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
  if (closed || closingRequested || disconnected) return;
  if (state?.status === "running" && !window.confirm("Stop the current setup? Accepted answers remain saved for a later run.")) return;
  closingRequested = true;
  stopCountdown();
  updateFooter();
  try {
    await request("/api/close", "POST", {});
    // Keep the scoped session until cleanup is confirmed, including if this
    // page reloads while the worker is still stopping.
    setFooterStatus("Stop requested; waiting for cleanup…");
    $("action-content").replaceChildren(el("p", "Stop requested; waiting for local work to finish and release its locks.", "empty-state"));
    await refresh();
  } catch (error) {
    closingRequested = false;
    if (!disconnected) setProblem(error.message);
  } finally { updateFooter(); }
});
function finishClose() {
  if (locked) {
    locked = false;
  }
  closeVaultDialog();
  closeBitwardenDialog(true);
  closeCopyJsonDialog(true);
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
  setFooterStatus("Session closed after cleanup", "success");
  $("back-button").hidden = true;
  $("continue-button").hidden = true;
  $("close-button").disabled = true;
  syncMainMenu();
}
async function startSetup() {
  if (pending || closed || disconnected) return;
  pending = true;
  updateFooter();
  try { await request("/api/start", "POST", {action: "setup"}); await refresh(); }
  catch (error) { if (!disconnected) setProblem(error.message); }
  finally { pending = false; updateFooter(); }
}
function updateFooter() {
  const prompt = state?.prompt;
  const primary = $("continue-button");
  syncMainMenu();
  if (disconnected) {
    primary.disabled = true;
    $("back-button").disabled = true;
    $("pause-button").disabled = true;
    $("close-button").disabled = true;
    $("vault-submit").disabled = true;
    $("vault-cancel").disabled = true;
    return;
  }
  $("back-button").disabled = false;
  $("pause-button").disabled = false;
  $("vault-cancel").disabled = false;
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
  if (prompt.kind === "menu") {
    const selected = selectedMenuCards().map(card => card.dataset.operationId);
    const hasFields = selected.some(id => fieldsForAction(id, state).length);
    primary.textContent = hasFields && reviewingModule === activeModule && reviewingSelection === selectedMenuSignature() ?
      "Run selected tasks" : "Setup selected tasks";
  }
  primary.disabled = ["menu", "modules"].includes(prompt.kind) && choices === 0 ||
    prompt.kind === "menu" && regeneratingPasswords.size > 0 ||
    prompt.kind === "menu" && selectedMenuCards().some(card => ["key", "github"].includes(card.dataset.operationId)) &&
      Boolean(signingDefaultsPending.get(activeModule)) ||
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
    .filter(option => selectedValues ? selectedValues.has(String(option.value)) : option.selected && operationAppliesToModule(option.id, module))
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
  if (data.status === "finished" || data.status === "error") {
    // Keep the filled form values in memory until the user exports a Bitwarden
    // note or closes/locks the page. Discard only the per-run answer queue.
    draftExecution = null;
    lastMenuOptions = null;
  }
  if (data.status === "locked") {
    showLock(data);
    setFooterStatus(data.vaultMode === "create" ? "Create a vault passphrase to begin" :
      data.initial ? "Unlock the vault to begin" : "Wizard locked after 20 minutes of inactivity", "warning");
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
    setFooterStatus("Stop requested; waiting for cleanup…");
  }
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
  if (!data.prompt && data.status === "running") setFooterStatus(vaultLabel ? `${vaultLabel} in progress…` : "Working on your repository…");
  else if (!data.prompt && data.status === "idle") setFooterStatus("Ready to set up this repository");
  else if (!data.prompt && data.status === "finished") setFooterStatus(vaultLabel ? `${vaultLabel} finished; review Process output` : "Setup complete", "success");
  else if (!data.prompt && data.status === "error") setFooterStatus(vaultLabel ? `${vaultLabel} needs attention; review Process output` : "Setup needs attention; review the process output", "error");
  else if (data.status === "closing") setFooterStatus("Stop requested; waiting for cleanup…");
  else if (data.prompt && paused) updateCountdownLabel();
  else if (data.prompt) setFooterStatus("Ready for the current setup step.");
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
  const recovered = disconnected;
  disconnected = false;
  if (Number(data.revision) !== lastRevision || recovered) {
    lastRevision = Number(data.revision);
    // Rendering an unchanged prompt does not replace its inputs. It does
    // restore the status badge after a transient connection failure.
    renderState(data);
  }
  if (recovered) {
    if (!pending) $("vault-submit").disabled = false;
    $("vault-cancel").disabled = false;
    updateFooter();
    if (data.prompt && !paused && !closed) setFooterStatus(data.prompt.kind === "menu" ?
      "Connection restored. Choose actions for this module." : "Connection restored. Continue the current step.", "success");
  }
}
async function poll() {
  if (closed) return;
  try { await refresh(); }
  catch (error) {
    if (error.unauthorized) {
      setProblem(error.message);
      closed = true;
      stopCountdown();
      $("continue-button").disabled = true;
      $("close-button").disabled = true;
    } else markDisconnected();
  }
  if (!closed) setTimeout(poll, disconnected ? 2000 : 500);
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
