import { en } from "./en.js";

const catalogs = { en };
const activeLocale = "en";

export function t(id, params = {}) {
  const template = catalogs[activeLocale]?.[id] ?? catalogs.en[id] ?? id;
  return template.replace(/\{(\w+)\}/g, (_, key) => String(params[key] ?? `{${key}}`));
}

export function messageText(message, fallbackId = "error.unknown") {
  const normalized = normalizeMessage(message, fallbackId);
  return t(normalized.id, normalized.params);
}

export function normalizeMessage(message, fallbackId = "error.unknown") {
  if (!message) return { id: fallbackId, params: {} };
  if (typeof message === "object") {
    return {
      id: message.id || fallbackId,
      params: message.params || {},
    };
  }
  if (typeof message === "string") {
    try {
      return normalizeMessage(JSON.parse(message), fallbackId);
    } catch {
      return { id: message || fallbackId, params: {} };
    }
  }
  return { id: fallbackId, params: {} };
}

export function formatNumber(value) {
  return new Intl.NumberFormat(activeLocale).format(value || 0);
}
