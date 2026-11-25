const jsonHeaders = { "Content-Type": "application/json" };

export class ApiError extends Error {
  constructor(message, status) {
    super(message);
    this.status = status;
  }
}

export async function api(path, options = {}) {
  const response = await fetch(path, {
    credentials: "same-origin",
    ...options,
    headers: {
      ...(options.body ? jsonHeaders : {}),
      ...(options.headers || {}),
    },
  });
  const text = await response.text();
  let data = null;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = null;
    }
  }
  if (response.status === 401 && path !== "/api/pair") {
    window.dispatchEvent(new Event("tokenpulse-unauthorized"));
  }
  if (!response.ok) {
    throw new ApiError(data?.error || response.statusText || "request failed", response.status);
  }
  return data;
}

export const getState = () => api("/api/state");
export const getSettings = () => api("/api/settings");
export const patchSettings = (updates) =>
  api("/api/settings", { method: "PATCH", body: JSON.stringify(updates) });
export const getUsage = (params) => api(`/api/usage?${new URLSearchParams(params)}`);
export const getDay = (date, agents) =>
  api(`/api/usage/day?${new URLSearchParams({ date, agents })}`);
export const getModel = (model, from, to, agents) =>
  api(`/api/usage/model?${new URLSearchParams({ model, from, to, agents })}`);
export const getQuota = () => api("/api/quota");
export const refresh = (usage = true, quota = true) =>
  api("/api/refresh", { method: "POST", body: JSON.stringify({ usage, quota }) });
export const getKeeper = () => api("/api/keeper");
export const pingKeeper = (agent, group) =>
  api("/api/keeper/ping", { method: "POST", body: JSON.stringify(group ? { agent, group } : { agent }) });
export const getPushKey = () => api("/api/push/key");
export const addPushSubscription = (subscription) =>
  api("/api/push/subscription", { method: "POST", body: JSON.stringify(subscription) });
export const removePushSubscription = (endpoint) =>
  api("/api/push/subscription", { method: "DELETE", body: JSON.stringify({ endpoint }) });

export async function pair(token) {
  return api("/api/pair", { method: "POST", body: JSON.stringify({ token }) });
}
