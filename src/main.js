import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import "@fontsource/instrument-sans/400.css";
import "@fontsource/instrument-sans/500.css";
import "@fontsource/instrument-sans/600.css";
import "@fontsource/instrument-sans/700.css";
import "@fontsource/ibm-plex-mono/400.css";
import "material-symbols/rounded.css";
import { formatNumber, messageText, normalizeMessage, t } from "./i18n/index.js";

const app = document.getElementById("app");
const isTauri = "__TAURI_INTERNALS__" in window;

const state = {
  phase: "Welcome",
  deletedCount: 0,
  stats: { total: 0, restored: 0, failed: 0, failed_ids: [] },
  message: { id: "status.ready", params: {} },
  busy: false,
  detailsOpen: false,
  log: [],
};

const steps = [
  ["Welcome", "step.start"],
  ["SigningIn", "step.signIn"],
  ["Scanning", "step.scan"],
  ["ReadyToRestore", "step.review"],
  ["Restoring", "step.restore"],
  ["Complete", "step.done"],
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
      state.message = { id: "status.signingIn", params: {} };
      break;
    case "authenticated":
      state.phase = "ReadyToScan";
      state.message = { id: "status.authenticated", params: {} };
      break;
    case "scanProgress":
      state.phase = "Scanning";
      state.deletedCount = event.total;
      state.message = { id: "status.scanProgress", params: eventParams(event) };
      appendLog("log.scanProgress", eventParams(event));
      break;
    case "scanComplete":
      state.phase = "ReadyToRestore";
      state.deletedCount = event.total;
      state.message = { id: "status.scanComplete", params: eventParams(event) };
      appendLog("log.scanComplete", eventParams(event));
      break;
    case "scanPaused":
      state.phase = "ReadyToScan";
      state.deletedCount = event.partialTotal ?? event.partial_total ?? 0;
      state.busy = false;
      state.message = {
        id: "status.scanCancelledSave",
        params: { count: formatNumber(state.deletedCount) },
      };
      appendLog("log.scanCancelled", { count: formatNumber(state.deletedCount) });
      break;
    case "restoreStarted":
      state.phase = "Restoring";
      state.stats.total = event.total;
      state.message = { id: "status.restoring", params: {} };
      appendLog("log.restoreStarted", eventParams(event));
      break;
    case "restoreProgress":
      state.phase = "Restoring";
      state.stats.total = event.total;
      state.stats.restored = event.restored;
      state.stats.failed = event.failed;
      state.message = normalizeMessage(event.message);
      appendLog("log.restoreProgress", {
        ...eventParams(event),
        message: messageText(event.message),
      });
      break;
    case "retry":
      state.message = normalizeMessage(event.message);
      appendLog("log.retry", {
        ...eventParams(event),
        batch: event.batchNumber,
        message: messageText(event.message),
      });
      break;
    case "paused":
      state.phase = "Paused";
      state.message = normalizeMessage(event.message);
      appendLog(event.message);
      break;
    case "complete":
      state.phase = "Complete";
      state.stats = normalizeStats(event.stats);
      state.message =
        state.stats.failed > 0
          ? { id: "status.partialComplete", params: statsParams(state.stats) }
          : { id: "status.complete", params: statsParams(state.stats) };
      appendLog("log.complete");
      state.busy = false;
      break;
    case "error":
      state.phase = "Error";
      state.message = normalizeMessage(event.message);
      appendLog("log.error", { message: messageText(event.message) });
      state.busy = false;
      break;
  }

  render();
}

function mergeSnapshot(snapshot) {
  state.phase = snapshot.phase;
  state.deletedCount = snapshot.deleted_count ?? snapshot.deletedCount ?? 0;
  state.stats = normalizeStats(snapshot.stats);
  state.message = normalizeMessage(snapshot.message || state.message);
  if (
    state.message.id === "status.scanCancelledSave" &&
    state.message.params?.count !== undefined &&
    state.message.params.count !== ""
  ) {
    const raw = state.message.params.count;
    const n = typeof raw === "number" ? raw : Number(String(raw).replace(/,/g, ""));
    state.message = {
      ...state.message,
      params: {
        ...state.message.params,
        count: Number.isFinite(n) ? formatNumber(n) : String(raw),
      },
    };
  }
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
      <aside class="sidebar" aria-label="${t("app.recoverySteps")}">
        <div class="brand">
          <div class="brand-mark" aria-hidden="true">${icon("cloud_sync", "brand-icon")}</div>
          <div>
            <p class="eyebrow">${t("app.productName")}</p>
            <h1>${t("app.title")}</h1>
          </div>
        </div>
        <ol class="stepper">
          ${steps
            .map(([, labelId], index) => {
              const status = index < currentStepIndex ? "done" : index === currentStepIndex ? "active" : "";
              return `<li class="${status}"><span>${index + 1}</span>${t(labelId)}</li>`;
            })
            .join("")}
        </ol>
        <p class="privacy-note">${t("app.privacyNote")}</p>
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
      <p class="eyebrow">${t("welcome.eyebrow")}</p>
      <h2>${t("welcome.title")}</h2>
      <p class="lede">${t("welcome.lede")}</p>
      <div class="actions">
        ${button(t("welcome.start"), "start-auth", "primary")}
      </div>
    </div>
  `;
}

function signInPanel() {
  return `
    <div class="hero-card">
      ${heroIcon("lock")}
      <p class="eyebrow">${t("signIn.eyebrow")}</p>
      <h2>${t("signIn.title")}</h2>
      <p class="lede">${t("signIn.lede")}</p>
      ${statusMessage()}
      <div class="actions">
        ${button(state.busy ? t("signIn.waiting") : t("signIn.open"), "start-auth", "primary", state.busy)}
        ${button(t("common.cancel"), "reset", "secondary")}
      </div>
    </div>
  `;
}

function scanPanel() {
  const scanning = state.phase === "Scanning" || state.busy;
  return `
    <div class="hero-card">
      ${heroIcon("cloud_sync")}
      <p class="eyebrow">${t("scan.eyebrow")}</p>
      <h2>${t("scan.title")}</h2>
      <p class="lede">${t("scan.lede")}</p>
      ${statusMessage()}
      <div class="metric-row metric-row-two">
        ${metric(t("scan.itemsFound"), formatNumber(state.deletedCount))}
        ${metric(t("scan.mode"), t("scan.modeValue"))}
      </div>
      <div class="actions">
        ${button(scanning ? t("scan.scanning") : t("scan.action"), "scan", "primary", scanning)}
        ${button(scanning ? t("scan.cancelSaveProgress") : t("common.cancel"), scanning ? "cancel-scan" : "reset", "secondary")}
      </div>
      ${detailsLog()}
    </div>
  `;
}

function reviewPanel() {
  return `
    <div class="hero-card">
      ${heroIcon("restore_from_trash")}
      <p class="eyebrow">${t("review.eyebrow")}</p>
      <h2>${t("review.title", { count: formatNumber(state.deletedCount) })}</h2>
      <p class="lede">${t("review.lede")}</p>
      ${statusMessage()}
      <div class="actions">
        ${button(t("review.restoreAction", { count: formatNumber(state.deletedCount) }), "restore", "primary", state.deletedCount === 0)}
        ${button(t("common.back"), "scan", "secondary")}
        ${button(t("common.cancel"), "reset", "secondary")}
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
      <p class="eyebrow">${t("restore.eyebrow")}</p>
      <h2>${paused ? t("restore.titlePaused") : t("restore.titleActive")}</h2>
      ${statusMessage()}
      <div class="progress-wrap" aria-label="${t("restore.progressLabel")}">
        <div class="progress-label"><span>${t("restore.percentComplete", { percent })}</span><span>${t("restore.countProgress", { completed: formatNumber(completed), total: formatNumber(total) })}</span></div>
        <div class="progress-track"><div class="progress-fill" data-progress="${percent}"></div></div>
      </div>
      <div class="metric-row">
        ${metric(t("restore.restored"), formatNumber(state.stats.restored), "success")}
        ${metric(t("restore.failed"), formatNumber(state.stats.failed), state.stats.failed > 0 ? "warning" : "")}
        ${metric(t("restore.total"), formatNumber(total))}
      </div>
      <div class="actions">
        ${
          paused
            ? button(t("restore.resume"), "restore", "primary")
            : button(t("restore.pause"), "pause", "primary", state.busy && false)
        }
        ${button(t("restore.cancelAndSave"), "pause", "secondary")}
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
      <p class="eyebrow">${partial ? t("done.partialEyebrow") : t("done.completeEyebrow")}</p>
      <h2>${partial ? t("done.partialTitle") : t("done.completeTitle")}</h2>
      <p class="lede">${escapeHtml(messageText(state.message))}</p>
      <div class="metric-row">
        ${metric(t("restore.restored"), formatNumber(state.stats.restored), "success")}
        ${metric(t("restore.failed"), formatNumber(state.stats.failed), partial ? "warning" : "")}
      </div>
      <div class="actions">
        ${partial ? button(t("done.retryFailed"), "retry", "primary") : ""}
        ${button(t("done.done"), "reset", partial ? "secondary" : "primary")}
      </div>
      ${detailsLog()}
    </div>
  `;
}

function errorPanel() {
  return `
    <div class="hero-card">
      ${heroIcon("error")}
      <p class="eyebrow danger">${t("error.eyebrow")}</p>
      <h2>${t("error.title")}</h2>
      ${statusMessage("error")}
      <div class="actions">
        ${button(t("error.tryAgain"), "start-auth", "primary")}
        ${button(t("error.chromeDownload"), "chrome-download", "secondary")}
      </div>
      ${detailsLog()}
    </div>
  `;
}

function statusMessage(kind = "info") {
  return `<div class="status ${kind}">${escapeHtml(messageText(state.message))}</div>`;
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
    : `<li>${t("details.empty")}</li>`;
  return `
    <div class="details">
      <button class="details-toggle" data-action="toggle-details">${state.detailsOpen ? t("details.hide") : t("details.show")}</button>
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
  if (action === "cancel-scan") {
    try {
      await safeInvoke("cancel_scan");
    } catch (error) {
      state.message = readableError(error);
      render();
    }
    return;
  }

  if (state.busy && !["toggle-details", "pause"].includes(action)) return;

  try {
    if (action === "toggle-details") {
      state.detailsOpen = !state.detailsOpen;
      render();
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
    appendLog("log.error", { message: messageText(state.message) });
  } finally {
    if (!["SigningIn", "Scanning", "Restoring"].includes(state.phase)) {
      state.busy = false;
    }
    render();
  }
}

function appendLog(message, params = {}) {
  const text = typeof message === "string" ? t(message, params) : messageText(message);
  state.log.push(`${new Date().toLocaleTimeString()} - ${text}`);
  if (state.log.length > 200) state.log.shift();
}

function readableError(error) {
  if (typeof error === "string") return normalizeMessage(error);
  if (error?.message) return normalizeMessage(error.message);
  return { id: "error.unknown", params: {} };
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
      message: { id: "status.ready", params: {} },
      can_resume: false,
    };
  }

  if (command === "reset_session") {
    return {
      phase: "Welcome",
      deleted_count: 0,
      stats: { total: 0, restored: 0, failed: 0, failed_ids: [] },
      message: { id: "status.ready", params: {} },
      can_resume: false,
    };
  }

  if (command === "cancel_scan") {
    return;
  }

  throw new Error(JSON.stringify({ id: "error.desktopOnly", params: {} }));
}

function resetLocalState() {
  Object.assign(state, {
    phase: "Welcome",
    deletedCount: 0,
    stats: { total: 0, restored: 0, failed: 0, failed_ids: [] },
    message: { id: "status.ready", params: {} },
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

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

function eventParams(event = {}) {
  return {
    page: event.page,
    pageCount: event.pageCount ?? event.page_count,
    total: event.total,
    restored: event.restored,
    failed: event.failed,
    attempt: event.attempt,
  };
}

function statsParams(stats = {}) {
  return {
    total: formatNumber(stats.total),
    restored: formatNumber(stats.restored),
    failed: formatNumber(stats.failed),
  };
}
