// node --test: the wallet history's query strings.
import assert from "node:assert/strict";
import { test } from "node:test";
import { NO_FILTERS, PAGE_SIZES, RHEA_KINDS, activeFilters, localDayIso, pageSize, rangeLabel, txQuery, withFilter } from "./txQuery.js";

const params = (q) => Object.fromEntries(new URLSearchParams(q.slice(1)));

test("no filters asks for the first page only", () => {
  assert.deepEqual(params(txQuery()), { limit: "10" });
});

test("filters become parameters, and the 'to' day is included whole", () => {
  const p = params(txQuery({ ...NO_FILTERS, direction: "in", status: "succeeded", from: "2026-11-01", to: "2026-11-15" }, { cursor: "abc", limit: 5 }));
  assert.equal(p.direction, "in");
  assert.equal(p.status, "succeeded");
  assert.equal(p.cursor, "abc");
  assert.equal(p.limit, "5");
  // Local midnights, whatever the time zone the test runs in.
  assert.equal(new Date(p.from).getTime(), new Date(2026, 10, 1).getTime());
  assert.equal(new Date(p.to).getTime(), new Date(2026, 10, 16).getTime());
});

test("a fixed set of kinds applies unless a kind is picked", () => {
  assert.equal(params(txQuery(NO_FILTERS, { kinds: RHEA_KINDS })).kind, "register,supply,withdraw,unwrap");
  assert.equal(params(txQuery({ ...NO_FILTERS, kind: "supply" }, { kinds: RHEA_KINDS })).kind, "supply");
});

test("bad dates are ignored, month ends roll over", () => {
  assert.equal(localDayIso("2026-13-45x"), null);
  assert.equal(localDayIso(""), null);
  assert.equal(new Date(localDayIso("2026-11-30", 1)).getTime(), new Date(2026, 11, 1).getTime());
  assert.equal(params(txQuery({ ...NO_FILTERS, from: "nope" })).from, undefined);
});

test("the date range stays in order", () => {
  const f = { ...NO_FILTERS, from: "2026-11-10", to: "2026-11-12" };
  assert.deepEqual([withFilter(f, "from", "2026-11-14").from, withFilter(f, "from", "2026-11-14").to], ["2026-11-14", "2026-11-14"]);
  assert.deepEqual([withFilter(f, "to", "2026-11-01").from, withFilter(f, "to", "2026-11-01").to], ["2026-11-01", "2026-11-01"]);
  assert.deepEqual(withFilter(f, "to", ""), { ...f, to: "" });
  assert.equal(withFilter(f, "direction", "in").direction, "in");
});

test("page sizes: only the offered ones, else 10", () => {
  assert.deepEqual(PAGE_SIZES.map(pageSize), [10, 25, 50, 100]);
  for (const bad of [null, undefined, "", "abc", 0, 7, 1000, "25x"]) assert.equal(pageSize(bad), 10);
  assert.equal(pageSize("50"), 50);
  assert.equal(params(txQuery(NO_FILTERS, { limit: 50 })).limit, "50");
});

test("labels", () => {
  assert.equal(activeFilters(NO_FILTERS), 0);
  assert.equal(activeFilters({ ...NO_FILTERS, direction: "in", from: "2026-11-01", to: "2026-11-02" }), 2);
  assert.equal(rangeLabel(0, 10, 57), "1–10 of 57");
  assert.equal(rangeLabel(5, 7, 57), "51–57 of 57");
  assert.equal(rangeLabel(0, 0, 0), "0 of 0");
  assert.equal(rangeLabel(1, 25, 60, 25), "26–50 of 60");
});
