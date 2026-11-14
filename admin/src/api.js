// Client for the Combox API. Every call carries the Supabase access token.
// On the multi-tenant platform, company endpoints live under /t/<company>.
export const API = (import.meta.env.VITE_AXOM_API_URL || "http://localhost:8080").replace(/\/$/, "");

// Fired when the API rejects the session (expired or revoked token), so the
// shell can ask the user to sign in again instead of every panel erroring.
export const UNAUTHORIZED_EVENT = "axom:unauthorized";

export async function request(session, url, { method = "GET", body } = {}) {
  const headers = { ...(session ? { Authorization: `Bearer ${session.access_token}` } : {}), ...(body ? { "Content-Type": "application/json" } : {}) };
  let res;
  try {
    res = await fetch(url, { method, headers, body: body ? JSON.stringify(body) : undefined });
  } catch {
    // fetch only throws on network failure (server down, CORS, offline).
    throw new Error(`Can't reach the Combox API at ${API}. Check that it's running and that this site is allowed by AXOM_CORS_ORIGINS.`);
  }
  const data = await res.json().catch(() => ({}));
  if (res.status === 401 && session) window.dispatchEvent(new CustomEvent(UNAUTHORIZED_EVENT));
  if (!res.ok) throw new Error(data.error || `Request failed (HTTP ${res.status})`);
  return data;
}

/** Download a file the API serves (signed in), saving it as `filename`. */
export async function download(session, url, filename) {
  let res;
  try {
    res = await fetch(url, { headers: session ? { Authorization: `Bearer ${session.access_token}` } : {} });
  } catch {
    throw new Error(`Can't reach the Combox API at ${API}.`);
  }
  if (res.status === 401 && session) window.dispatchEvent(new CustomEvent(UNAUTHORIZED_EVENT));
  if (!res.ok) {
    const data = await res.json().catch(() => ({}));
    throw new Error(data.error || `Download failed (HTTP ${res.status})`);
  }
  const href = URL.createObjectURL(await res.blob());
  const a = Object.assign(document.createElement("a"), { href, download: filename });
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(href), 10_000);
}

/** Platform endpoints (/platform/...). */
export const platform = (session, path, opts) => request(session, `${API}/platform${path}`, opts);

/** "multi-tenant" or "single" (one company per deployment). */
export async function detectMode() {
  let r;
  try {
    r = await fetch(`${API}/health`);
  } catch {
    return "unreachable";
  }
  // Something answered, but not the API (e.g. the URL is this dashboard).
  const h = await r.json().catch(() => null);
  if (!r.ok || !h) return "not-api";
  return h.mode === "multi-tenant" ? "multi-tenant" : "single";
}
