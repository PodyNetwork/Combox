// The treasury's transaction history: deposits, payouts and Rhea moves,
// paged and filtered by the API (GET /admin/treasury/transactions). Used in
// full on the Wallet page and for the Rhea moves on the Treasury yield page.
import { useState } from "react";
import DateTimePicker from "../DateTimePicker.jsx";
import Dropdown from "../Dropdown.jsx";
import { Badge, Button, Empty, ErrorNote, Icon, Loading, Panel, PanelHead, Segmented, ago, shortId, useLoad, when } from "../ui.jsx";
import { NO_FILTERS, PAGE_SIZES, RHEA_KINDS, activeFilters, pageSize, rangeLabel, txQuery, withFilter } from "../txQuery.js";

const KIND = {
  deposit: "Deposit",
  payout: "Payout",
  register: "Registered on Rhea",
  supply: "Supplied to Rhea",
  withdraw: "Withdrew from Rhea",
  unwrap: "Unwrapped wNEAR",
};
const STATUS_TONE = { succeeded: "lime", submitted: "sky", failed: "ember", refunded: "amber" };
const MOVE_ICON = { register: "key", supply: "arrow", withdraw: "out", unwrap: "check" };

const SCOPES = {
  wallet: {
    kinds: null,
    directions: true,
    kindOptions: ["deposit", "payout", ...RHEA_KINDS],
    statuses: ["succeeded", "submitted", "failed", "refunded"],
    empty: "Deposits to the treasury, payouts to workers and the signer's Rhea moves show up here, each with a NearBlocks link.",
  },
  rhea: {
    kinds: RHEA_KINDS,
    directions: false,
    kindOptions: RHEA_KINDS,
    statuses: ["succeeded", "submitted", "failed"],
    empty: "Registering, supplying, withdrawing and unwrapping each show up here with a NearBlocks link.",
  },
};

// The reader's page size, remembered in this browser only.
const SIZE_KEY = "combox.txPageSize";
const savedSize = () => {
  try {
    return pageSize(localStorage.getItem(SIZE_KEY));
  } catch {
    return pageSize(null);
  }
};

const near = (s, digits = 4) => Number(s).toLocaleString(undefined, { maximumFractionDigits: digits });
const cap = (s) => s.charAt(0).toUpperCase() + s.slice(1);

/** History with its filter bar and pager. `scope`: "wallet" or "rhea". */
export function TxHistory({ scope = "wallet", eyebrow = "On chain", title = "Transactions", i = 0 }) {
  const cfg = SCOPES[scope];
  const [filters, setFilters] = useState(NO_FILTERS);
  // Cursors of the pages visited so far; going back reuses them.
  const [pages, setPages] = useState([null]);
  const [index, setIndex] = useState(0);
  const [size, setSize] = useState(savedSize);
  const resize = (v) => {
    const n = pageSize(v);
    setSize(n);
    setPages([null]);
    setIndex(0);
    try {
      localStorage.setItem(SIZE_KEY, String(n));
    } catch {
      /* not remembered; the choice still applies */
    }
  };
  const set = (k) => (v) => {
    setFilters((f) => withFilter(f, k, v));
    setPages([null]);
    setIndex(0);
  };
  const clear = () => {
    setFilters(NO_FILTERS);
    setPages([null]);
    setIndex(0);
  };
  const path = `/admin/treasury/transactions${txQuery(filters, { cursor: pages[index], kinds: cfg.kinds, limit: size })}`;
  const n = activeFilters(filters);
  return (
    <Panel i={i} className="p-2 sm:p-3">
      <PanelHead className="px-4 pt-4" eyebrow={eyebrow} title={title} />
      <div className="space-y-3 px-4 pb-4">
        <div className="flex flex-wrap items-center gap-3">
          {cfg.directions && (
            <Segmented
              ariaLabel="Direction"
              value={filters.direction}
              onChange={set("direction")}
              options={[{ value: "", label: "All" }, { value: "in", label: "In" }, { value: "out", label: "Out" }, { value: "internal", label: "Internal" }]}
            />
          )}
          {n > 0 && (
            <button type="button" onClick={clear} className="ml-auto inline-flex items-center gap-1 text-xs text-mute hover:text-fog">
              <Icon name="x" size={12} /> Clear {n === 1 ? "filter" : `${n} filters`}
            </button>
          )}
        </div>
        <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
          <Select label="Type" value={filters.kind} onChange={set("kind")} options={cfg.kindOptions.map((k) => ({ value: k, label: KIND[k] }))} all="All types" />
          <Select label="Status" value={filters.status} onChange={set("status")} options={cfg.statuses.map((s) => ({ value: s, label: cap(s) }))} all="Any status" />
          <DateTimePicker label="From" value={filters.from} onChange={set("from")} time={false} allowPast placeholder="Any day" />
          <DateTimePicker label="To" value={filters.to} onChange={set("to")} time={false} allowPast placeholder="Today" />
        </div>
      </div>
      <Page
        key={path}
        path={path}
        index={index}
        size={size}
        onSize={resize}
        empty={n ? "Nothing matches these filters." : cfg.empty}
        filtered={n > 0}
        onPrev={() => setIndex((x) => Math.max(0, x - 1))}
        onNext={(cursor) => {
          setPages((p) => [...p.slice(0, index + 1), cursor]);
          setIndex((x) => x + 1);
        }}
      />
    </Panel>
  );
}

function Select({ label, value, onChange, options, all }) {
  return <Dropdown label={label} value={value} onChange={onChange} options={[{ value: "", label: all }, ...options]} />;
}

/** One page of rows and the pager. Keyed by its path, so a new page or
 * filter starts from a loading state (or the last answer for that path). */
function Page({ path, index, size, onSize, empty, filtered, onPrev, onNext }) {
  // Only the first page refreshes on its own: later pages would shift under the reader.
  const { data, error } = useLoad(path, { poll: index === 0 ? 60000 : undefined });
  const rows = data?.transactions || [];
  const showTotals = data && (Number(data.totals?.in_near) > 0 || Number(data.totals?.out_near) > 0);
  return (
    <>
      <div className="px-4">
        <ErrorNote error={error} />
        {data?.deposits?.error && <p className="mb-3 text-xs text-amber first-letter:uppercase">{data.deposits.error}. Deposits shown may be out of date.</p>}
      </div>
      {!data && !error && <div className="px-4 pb-4"><Loading rows={4} /></div>}
      {data && rows.length === 0 && <div className="px-4 pb-4"><Empty title={filtered ? "No transactions" : "No transactions yet"} text={empty} /></div>}
      {rows.length > 0 && (
        <ul className="divide-y divide-white/7 border-t border-white/7">
          {rows.map((t) => <Row key={t.id} t={t} />)}
        </ul>
      )}
      {data && (rows.length > 0 || index > 0) && (
        <div className="flex flex-wrap items-center gap-3 border-t border-white/7 px-4 pb-2 pt-3 text-xs text-mute">
          <span className="num">{rangeLabel(index, rows.length, data.total, size)}</span>
          {showTotals && (
            <span className="num hidden sm:inline" title="Total of the succeeded transactions that match the filters, on every page">
              {near(data.totals.in_near)} NEAR received · {near(data.totals.out_near)} NEAR paid out
            </span>
          )}
          <span className="ml-auto flex items-center gap-2">
            <span className="flex items-center gap-1.5">
              <span aria-hidden="true">Rows</span>
              <Dropdown small label="Rows per page" value={String(size)} onChange={onSize} options={PAGE_SIZES.map((n) => ({ value: String(n), label: String(n) }))} />
            </span>
            <Button size="sm" variant="ghost" onClick={onPrev} disabled={index === 0} aria-label="Previous page">
              <Icon name="chevronLeft" size={14} /> Newer
            </Button>
            <Button size="sm" variant="ghost" onClick={() => onNext(data.next_cursor)} disabled={!data.next_cursor} aria-label="Next page">
              Older <Icon name="chevronRight" size={14} />
            </Button>
          </span>
        </div>
      )}
    </>
  );
}

function Row({ t }) {
  const sign = t.direction === "in" ? "+" : t.direction === "out" ? "−" : "";
  const who = t.label || shortId(t.counterparty);
  const title = t.kind === "deposit" ? `Deposit from ${who}` : t.kind === "payout" ? `Payout to ${who}` : KIND[t.kind] || t.kind;
  const icon =
    t.direction === "in" ? { name: "arrow", cls: "rotate-135 text-lime" } : t.direction === "out" ? { name: "arrow", cls: "-rotate-45" } : { name: MOVE_ICON[t.kind] || "bolt", cls: t.kind === "supply" ? "-rotate-45" : "" };
  return (
    <li className="flex items-start gap-3 px-4 py-3.5 sm:gap-4">
      <span className="mt-0.5 grid shrink-0 place-items-center rounded-xl bg-white/5 size-9 text-mute">
        <Icon name={icon.name} size={16} className={icon.cls} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span className="min-w-0 truncate text-sm font-medium" title={t.counterparty}>{title}</span>
          <Badge tone={STATUS_TONE[t.status] || "mute"} dot={t.status === "submitted"}>{t.status}</Badge>
        </span>
        {t.note && <span className="mt-0.5 block text-xs text-mute">{t.note}</span>}
        {t.error && <span className="mt-1 block break-words text-xs text-ember">{t.error}</span>}
      </span>
      <span className="shrink-0 text-right">
        <span className={`num block text-sm font-medium ${t.direction === "in" ? "text-lime" : ""}`} title={`${t.amount_near} NEAR`}>
          {sign}{near(t.amount_near)} <span className="text-xs font-normal text-mute">NEAR</span>
        </span>
        <a href={t.explorer_url} target="_blank" rel="noreferrer noopener" className="mt-0.5 inline-flex items-center gap-1 text-xs text-mute hover:text-fog" title={`${when(t.at)} · ${t.tx_hash}`}>
          {ago(t.at)} <Icon name="external" size={12} />
        </a>
      </span>
    </li>
  );
}
