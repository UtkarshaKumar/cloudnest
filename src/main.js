import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import "@fontsource/instrument-sans/400.css";
import "@fontsource/instrument-sans/500.css";
import "@fontsource/instrument-sans/600.css";
import "@fontsource/instrument-sans/700.css";
import "@fontsource/ibm-plex-mono/400.css";
import "material-symbols/rounded.css";

const app = document.getElementById("app");
const isTauri = "__TAURI_INTERNALS__" in window;

const state = {
  phase: "Welcome",
  deletedCount: 0,
  stats: { total: 0, restored: 0, failed: 0, failed_ids: [] },
  message: "Ready to recover deleted iCloud Drive files.",
  busy: false,
  detailsOpen: false,
  log: [],
};

const steps = [
  ["Welcome", "Start"],
  ["SigningIn", "Sign In"],
  ["Scanning", "Scan"],
  ["ReadyToRestore", "Review"],
  ["Restoring", "Restore"],
  ["Complete", "Done"],
];

const phaseToStepIndex = {
  Welcome: 0,
  SigningIn: 1,
  ReadyToScan: 2,
  Scanning: 2,
  ReadyToRestore: 3,
  Restoring: 4,
  Paused: 4,
  Complete: 5,
  Error: 4,
};

init().catch((error) => {
  state.message = readableError(error);
  state.phase = "Error";
  render();
});

async function init() {
  await safeListen("restore-event", (event) => {
    handleRestoreEvent(event.payload);
  });

  const snapshot = await safeInvoke("get_restore_state");
  mergeSnapshot(snapshot);
  render();
}

function handleRestoreEvent(event) {
  if (!event || !event.type) return;

  switch (event.type) {
    case "authStarted":
      state.phase = "SigningIn";
      state.message = "Waiting for iCloud sign-in...";
      break;
    case "authenticated":
      state.phase = "ReadyToScan";
      state.message = "Sign-in detected. Ready to scan deleted items.";
      break;
    case "scanProgress":
      state.phase = "Scanning";
      state.deletedCount = event.total;
      state.message = `Page ${event.page} scanned. ${event.total} items found.`;
      appendLog(`Scan page ${event.page}: ${event.pageCount} items, ${event.total} total.`);
      break;
    case "scanComplete":
      state.phase = "ReadyToRestore";
      state.deletedCount = event.total;
      state.message = `${event.total} deleted iCloud Drive items are ready to restore.`;
      appendLog(`Scan complete: ${event.total} deleted items found.`);
      break;
    case "restoreStarted":
      state.phase = "Restoring";
      state.stats.total = event.total;
      state.message = "Restoring your files...";
      appendLog(`Restore started for ${event.total} items.`);
      break;
    case "restoreProgress":
      state.phase = "Restoring";
      state.stats.total = event.total;
      state.stats.restored = event.restored;
      state.stats.failed = event.failed;
      state.message = event.message;
      appendLog(`${event.message} Restored ${event.restored}, failed ${event.failed}.`);
      break;
    case "retry":
      state.message = event.message;
      appendLog(`Batch ${event.batchNumber} retry ${event.attempt}: ${event.message}`);
      break;
    case "paused":
      state.phase = "Paused";
      state.message = event.message;
      appendLog(event.message);
      break;
    case "complete":
      state.phase = "Complete";
      state.stats = normalizeStats(event.stats);
      state.message =
        state.stats.failed > 0
          ? `Restored ${state.stats.restored} items. ${state.stats.failed} items need another try.`
          : `Recovery complete. ${state.stats.restored} items were restored to iCloud Drive.`;
      appendLog("Restore complete.");
      state.busy = false;
      break;
    case "error":
      state.phase = "Error";
      state.message = event.message;
      appendLog(`Error: ${event.message}`);
      state.busy = false;
      break;
  }

  render();
}

function mergeSnapshot(snapshot) {
  state.phase = snapshot.phase;
  state.deletedCount = snapshot.deleted_count ?? snapshot.deletedCount ?? 0;
  state.stats = normalizeStats(snapshot.stats);
  state.message = snapshot.message || state.message;
}

function normalizeStats(stats = {}) {
  return {
    total: stats.total ?? 0,
    restored: stats.restored ?? 0,
    failed: stats.failed ?? 0,
    failed_ids: stats.failed_ids ?? stats.failedIds ?? [],
  };
}

function render() {
  const currentStepIndex = phaseToStepIndex[state.phase] ?? 0;
  app.innerHTML = `
    <section class="shell">
      <aside class="sidebar" aria-label="Recovery steps">
        <div class="brand">
          <div class="brand-mark" aria-hidden="true">${icon("cloud_sync", "brand-icon")}</div>
          <div>
            <p class="eyebrow">CloudNest</p>
            <h1>Soft iCloud recovery</h1>
          </div>
        </div>
        <ol class="stepper">
          ${steps
            .map(([, label], index) => {
              const status = index < currentStepIndex ? "done" : index === currentStepIndex ? "active" : "";
              return `<li class="${status}"><span>${index + 1}</span>${label}</li>`;
            })
            .join("")}
        </ol>
        <p class="privacy-note">Your Apple ID password and two-factor code stay inside Apple's iCloud sign-in page. Progress is saved locally on this Mac.</p>
      </aside>
      <section class="panel">
        ${renderPanel()}
      </section>
    </section>
  `;

  bindActions();
  syncProgressStyles();
}

function renderPanel() {
  if (state.phase === "SigningIn") return signInPanel();
  if (state.phase === "ReadyToScan" || state.phase === "Scanning") return scanPanel();
  if (state.phase === "ReadyToRestore") return reviewPanel();
  if (state.phase === "Restoring" || state.phase === "Paused") return restorePanel();
  if (state.phase === "Complete") return donePanel();
  if (state.phase === "Error") return errorPanel();
  return welcomePanel();
}

function welcomePanel() {
  return `
    <div class="hero-card">
      ${heroIcon("folder_open")}
      <p class="eyebrow">Calm file recovery</p>
      <h2>Recover deleted iCloud Drive files</h2>
      <p class="lede">CloudNest gently finds recently deleted items and brings them back home safely in batches.</p>
      <div class="actions">
        ${button("Start Recovery", "start-auth", "primary")}
      </div>
      <button class="link-button" data-action="privacy">How this keeps your data private</button>
      <div class="callout hidden" id="privacy-copy">
        Your Apple ID password and two-factor code stay inside Apple's iCloud sign-in page. Session credentials are kept in memory only and are not written to disk.
      </div>
    </div>
  `;
}

function signInPanel() {
  return `
    <div class="hero-card">
      ${heroIcon("lock")}
      <p class="eyebrow">Step 1</p>
      <h2>Sign in with Apple</h2>
      <p class="lede">A Chrome window will open for iCloud. Apple handles your password, Keychain, and two-factor code. This app only watches for the restore session needed to continue.</p>
      ${statusMessage()}
      <div class="actions">
        ${button(state.busy ? "Waiting for Sign In" : "Open iCloud Sign In", "start-auth", "primary", state.busy)}
        ${button("Cancel", "reset", "secondary")}
      </div>
    </div>
  `;
}

function scanPanel() {
  const scanning = state.phase === "Scanning" || state.busy;
  return `
    <div class="hero-card">
      ${heroIcon("cloud_sync")}
      <p class="eyebrow">Step 2</p>
      <h2>Finding deleted items</h2>
      <p class="lede">Scanning recently deleted iCloud Drive files and folders. Large accounts can take a few minutes.</p>
      ${statusMessage()}
      <div class="metric-row">
        ${metric("Items found", formatNumber(state.deletedCount))}
        ${metric("Mode", "Safe batch scan")}
      </div>
      <div class="actions">
        ${button(scanning ? "Scanning" : "Scan Deleted Items", "scan", "primary", scanning)}
        ${button("Cancel", "reset", "secondary")}
      </div>
      ${detailsLog()}
    </div>
  `;
}

function reviewPanel() {
  return `
    <div class="hero-card">
      ${heroIcon("restore_from_trash")}
      <p class="eyebrow">Step 3</p>
      <h2>${formatNumber(state.deletedCount)} items ready to restore</h2>
      <p class="lede">They will be restored to iCloud Drive using Apple's own recovery endpoint.</p>
      ${statusMessage()}
      <div class="actions">
        ${button(`Restore ${formatNumber(state.deletedCount)} Items`, "restore", "primary", state.deletedCount === 0)}
        ${button("Back", "scan", "secondary")}
        ${button("Cancel", "reset", "secondary")}
      </div>
    </div>
  `;
}

function restorePanel() {
  const total = state.stats.total || state.deletedCount || 1;
  const completed = state.stats.restored + state.stats.failed;
  const percent = Math.min(100, Math.round((completed / total) * 100));
  const paused = state.phase === "Paused";

  return `
    <div class="hero-card wide">
      ${heroIcon("cloud_sync")}
      <p class="eyebrow">Step 4</p>
      <h2>${paused ? "Restore paused" : "Restoring your files"}</h2>
      ${statusMessage()}
      <div class="progress-wrap" aria-label="Restore progress">
        <div class="progress-label"><span>${percent}% complete</span><span>${formatNumber(completed)} of ${formatNumber(total)}</span></div>
        <div class="progress-track"><div class="progress-fill" data-progress="${percent}"></div></div>
      </div>
      <div class="metric-row">
        ${metric("Restored", formatNumber(state.stats.restored), "success")}
        ${metric("Need another try", formatNumber(state.stats.failed), state.stats.failed > 0 ? "warning" : "")}
        ${metric("Total", formatNumber(total))}
      </div>
      <div class="actions">
        ${
          paused
            ? button("Resume Restore", "restore", "primary")
            : button("Pause After Current Batch", "pause", "primary", state.busy && false)
        }
        ${button("Cancel and Save Progress", "pause", "secondary")}
      </div>
      ${detailsLog()}
    </div>
  `;
}

function donePanel() {
  const partial = state.stats.failed > 0;
  return `
    <div class="hero-card">
      ${heroIcon(partial ? "restore_from_trash" : "check_circle")}
      <p class="eyebrow">${partial ? "Partial success" : "Complete"}</p>
      <h2>${partial ? "Mostly recovered" : "Recovery complete"}</h2>
      <p class="lede">${escapeHtml(state.message)}</p>
      <div class="metric-row">
        ${metric("Restored", formatNumber(state.stats.restored), "success")}
        ${metric("Need another try", formatNumber(state.stats.failed), partial ? "warning" : "")}
      </div>
      <div class="actions">
        ${partial ? button("Retry Failed Items", "retry", "primary") : ""}
        ${button("Done", "reset", partial ? "secondary" : "primary")}
      </div>
      ${detailsLog()}
    </div>
  `;
}

function errorPanel() {
  return `
    <div class="hero-card">
      ${heroIcon("error")}
      <p class="eyebrow danger">Needs attention</p>
      <h2>Recovery needs your help</h2>
      ${statusMessage("error")}
      <div class="actions">
        ${button("Try Again", "start-auth", "primary")}
        ${button("Open Chrome Download", "chrome-download", "secondary")}
      </div>
      ${detailsLog()}
    </div>
  `;
}

function statusMessage(kind = "info") {
  return `<div class="status ${kind}">${escapeHtml(state.message)}</div>`;
}

function metric(label, value, tone = "") {
  return `
    <div class="metric ${tone}">
      <span>${label}</span>
      <strong>${value}</strong>
    </div>
  `;
}

function detailsLog() {
  const body = state.log.length
    ? state.log.map((entry) => `<li>${escapeHtml(entry)}</li>`).join("")
    : "<li>No technical details yet.</li>";
  return `
    <div class="details">
      <button class="details-toggle" data-action="toggle-details">${state.detailsOpen ? "Hide" : "Show"} Details</button>
      <ul class="log ${state.detailsOpen ? "" : "hidden"}">${body}</ul>
    </div>
  `;
}

function button(label, action, style = "secondary", disabled = false) {
  return `<button class="btn ${style}" data-action="${action}" ${disabled ? "disabled" : ""}>${label}</button>`;
}

function heroIcon(name) {
  return `<div class="hero-icon">${icon(name)}</div>`;
}

function icon(name, className = "") {
  return `<span class="material-symbols-rounded ${className}" aria-hidden="true">${name}</span>`;
}

function bindActions() {
  app.querySelectorAll("[data-action]").forEach((element) => {
    element.addEventListener("click", () => dispatch(element.dataset.action));
  });
}

function syncProgressStyles() {
  app.querySelectorAll("[data-progress]").forEach((element) => {
    const progress = Math.min(100, Math.max(0, Number(element.dataset.progress) || 0));
    element.style.width = `${progress}%`;
  });
}

async function dispatch(action) {
  if (state.busy && !["toggle-details", "privacy", "pause"].includes(action)) return;

  try {
    if (action === "toggle-details") {
      state.detailsOpen = !state.detailsOpen;
      render();
      return;
    }
    if (action === "privacy") {
      document.getElementById("privacy-copy")?.classList.toggle("hidden");
      return;
    }
    if (action === "chrome-download") {
      await safeOpenUrl("https://www.google.com/chrome/");
      return;
    }
    if (action === "reset") {
      const snapshot = await safeInvoke("reset_session");
      resetLocalState();
      mergeSnapshot(snapshot);
      render();
      return;
    }

    state.busy = true;
    render();

    const commandByAction = {
      "start-auth": "start_auth",
      scan: "scan_deleted_items",
      restore: "start_restore",
      pause: "pause_restore",
      retry: "retry_failed",
    };
    const command = commandByAction[action];
    if (!command) return;

    const snapshot = await safeInvoke(command);
    mergeSnapshot(snapshot);
  } catch (error) {
    state.phase = "Error";
    state.message = readableError(error);
    appendLog(`Error: ${state.message}`);
  } finally {
    if (!["SigningIn", "Scanning", "Restoring"].includes(state.phase)) {
      state.busy = false;
    }
    render();
  }
}

function appendLog(entry) {
  state.log.push(`${new Date().toLocaleTimeString()} - ${entry}`);
  if (state.log.length > 200) state.log.shift();
}

function readableError(error) {
  if (typeof error === "string") return error;
  if (error?.message) return error.message;
  return "Something went wrong. Progress is saved when possible.";
}

async function safeListen(eventName, handler) {
  if (!isTauri) return () => {};
  return listen(eventName, handler);
}

async function safeInvoke(command) {
  if (isTauri) return invoke(command);

  if (command === "get_restore_state") {
    return {
      phase: "Welcome",
      deleted_count: 0,
      stats: { total: 0, restored: 0, failed: 0, failed_ids: [] },
      message: "Ready to recover deleted iCloud Drive files.",
      can_resume: false,
    };
  }

  if (command === "reset_session") {
    return {
      phase: "Welcome",
      deleted_count: 0,
      stats: { total: 0, restored: 0, failed: 0, failed_ids: [] },
      message: "Ready to recover deleted iCloud Drive files.",
      can_resume: false,
    };
  }

  throw new Error("Desktop-only action. Open CloudNest as a macOS app to continue.");
}

function resetLocalState() {
  Object.assign(state, {
    phase: "Welcome",
    deletedCount: 0,
    stats: { total: 0, restored: 0, failed: 0, failed_ids: [] },
    message: "Ready to recover deleted iCloud Drive files.",
    busy: false,
    log: [],
  });
}

async function safeOpenUrl(url) {
  if (isTauri) {
    await openUrl(url);
    return;
  }

  window.open(url, "_blank", "noopener,noreferrer");
}

function formatNumber(value) {
  return new Intl.NumberFormat().format(value || 0);
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}
