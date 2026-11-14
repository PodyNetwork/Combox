// A "you're signed in" hint for the public website, the way many sites use
// a logged_in cookie: the website reads it and sends signed-in visitors
// straight to the dashboard. It holds no secret (the session itself stays in
// this app's storage), so reading it grants nothing.
//
// Cookies are shared across ports, so on localhost the website (:5175) sees
// what the dashboard (:5174) sets. In production, set
// VITE_SESSION_COOKIE_DOMAIN to the parent domain (e.g. example.com) so
// app.example.com and example.com share it.
import { JUDGE_MODE } from "./Auth.jsx";

const NAME = "axom_signed_in";
const DOMAIN = import.meta.env.VITE_SESSION_COOKIE_DOMAIN;

export function markSignedIn(on) {
  const parts = [`${NAME}=${on ? "1" : ""}`, "path=/", "SameSite=Lax"];
  if (DOMAIN) parts.push(`domain=${DOMAIN}`);
  if (window.location.protocol === "https:") parts.push("Secure");
  // Judge sessions live in this tab only, so the hint ends with the browser
  // session; real sessions refresh, so keep the hint for 30 days.
  if (!on) parts.push("max-age=0");
  else if (!JUDGE_MODE) parts.push(`max-age=${60 * 60 * 24 * 30}`);
  try {
    document.cookie = parts.join("; ");
  } catch {
    /* cookies blocked: the website just won't redirect */
  }
}
