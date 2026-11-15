// Query strings for the treasury's transaction history
// (GET /admin/treasury/transactions). Paging and filtering happen on the
// server; this only turns the filter bar into parameters. Plain JS so
// `npm test` (node --test) can check it without a browser.

export const PAGE_SIZE = 10;
/** Page sizes the reader can pick (the API allows up to 100). */
export const PAGE_SIZES = [10, 25, 50, 100];

/** A remembered or requested page size, or the default if it isn't one of PAGE_SIZES. */
export function pageSize(v) {
  const n = Number(v);
  return PAGE_SIZES.includes(n) ? n : PAGE_SIZE;
}
export const RHEA_KINDS = ["register", "supply", "withdraw", "unwrap"];

/** Empty filters: everything. Dates are local "YYYY-MM-DD" strings. */
export const NO_FILTERS = Object.freeze({ direction: "", kind: "", status: "", from: "", to: "" });

const pad = (n) => String(n).padStart(2, "0");

/** Local midnight of a "YYYY-MM-DD" day, plus `days`, as an RFC 3339 instant. */
export function localDayIso(ymd, days = 0) {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(ymd || "");
  if (!m) return null;
  const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]) + days);
  return Number.isNaN(d.getTime()) ? null : d.toISOString();
}

/** Today as a local "YYYY-MM-DD". */
export function todayYmd(now = new Date()) {
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

/**
 * The query string for one page. `kinds` limits the history to those kinds
 * (the yield page's Rhea moves) unless a kind filter is chosen; the "to" day
 * is included whole (the server's `to` is exclusive).
 */
export function txQuery(filters = NO_FILTERS, { cursor, limit = PAGE_SIZE, kinds } = {}) {
  const p = new URLSearchParams();
  p.set("limit", String(limit));
  if (filters.direction) p.set("direction", filters.direction);
  if (filters.kind) p.set("kind", filters.kind);
  else if (kinds?.length) p.set("kind", kinds.join(","));
  if (filters.status) p.set("status", filters.status);
  const from = localDayIso(filters.from);
  const to = localDayIso(filters.to, 1);
  if (from) p.set("from", from);
  if (to) p.set("to", to);
  if (cursor) p.set("cursor", cursor);
  return `?${p}`;
}

/**
 * Set one filter. The dates stay in order: a "from" after "to" moves "to"
 * to that day, and a "to" before "from" moves "from", so the range is never
 * empty by accident. ("YYYY-MM-DD" strings compare in date order.)
 */
export function withFilter(filters, key, value) {
  const f = { ...filters, [key]: value };
  if (f.from && f.to && f.from > f.to) {
    if (key === "from") f.to = f.from;
    else f.from = f.to;
  }
  return f;
}

/** How many filters are set (dates count once as a range). */
export function activeFilters(filters) {
  return ["direction", "kind", "status"].filter((k) => filters[k]).length + (filters.from || filters.to ? 1 : 0);
}

/** "21–40 of 57" for page `index` (0-based). */
export function rangeLabel(index, shown, total, size = PAGE_SIZE) {
  if (!total || !shown) return "0 of 0";
  const first = index * size + 1;
  return `${first}–${first + shown - 1} of ${total}`;
}
