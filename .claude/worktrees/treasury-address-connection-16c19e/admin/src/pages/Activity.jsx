// The owner notification feed: firings, blocked payouts, contracts, digests.
import { useEffect, useMemo, useState } from "react";
import { Empty, ErrorNote, Icon, Loading, PageHead, Panel, ago, human, tidy, useApp, when } from "../ui.jsx";

// What each notification is about, and how loud it should be. Unknown kinds
// fall through to "other" / neutral.
const categoryOf = (kind) =>
  /^(payout|cap|signer)/.test(kind) ? "money" : /^meeting|interview/.test(kind) ? "interviews" : /worker|hire|fire|contract/.test(kind) ? "people" : "other";
const toneOf = (kind) =>
  /fired|blocked|failed|rejected|exceeded|kill_switch/.test(kind) ? "ember" : /held|probation|pending/.test(kind) ? "amber" : /meeting|summary|interview/.test(kind) ? "violet" : /sent|approved|accepted/.test(kind) ? "lime" : "mute";

// "worker_fired" -> "Worker fired".
const kindLabel = (k) => {
  const s = human(k);
  return s.charAt(0).toUpperCase() + s.slice(1);
};

const ICON = { money: "payroll", people: "user", interviews: "meetings", other: "feed" };
const iconFor = (kind) => (kind === "kill_switch" ? "kill" : ICON[categoryOf(kind)]);
const TILE = {
  ember: "bg-ember/12 text-ember ring-ember/25",
  amber: "bg-amber/12 text-amber ring-amber/25",
  violet: "bg-violet/12 text-violet ring-violet/25",
  lime: "bg-lime/10 text-lime ring-lime/20",
  mute: "bg-white/5 text-mute ring-white/10",
};
const TEXT = { ember: "text-ember", amber: "text-amber", violet: "text-violet", lime: "text-lime", mute: "text-mute" };

function dayLabel(iso) {
  const d = new Date(iso);
  const today = new Date();
  const start = (x) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((start(today) - start(d)) / 86400000);
  if (diff === 0) return "Today";
  if (diff === 1) return "Yesterday";
  return d.toLocaleDateString(undefined, { weekday: "long", month: "short", day: "numeric" });
}

// Wide screens show a detail panel beside the list; narrow ones expand rows inline.
const isWide = () => typeof window !== "undefined" && !!window.matchMedia?.("(min-width: 1280px)").matches;

export default function Activity() {
  // The shell keeps the feed fresh (polled) for the bell; read it from there.
  const { status } = useApp();
  const { data, error, loading } = status.feed;
  const [kind, setKind] = useState(null);
  const [selected, setSelected] = useState(null);
  const [wide, setWide] = useState(isWide);
  const all = data?.notifications || [];
  const kinds = useMemo(() => [...new Set(all.map((n) => n.kind))], [all]);
  const list = useMemo(() => all.filter((n) => !kind || n.kind === kind), [all, kind]);
  const groups = useMemo(() => {
    const out = [];
    list.forEach((n) => {
      const label = dayLabel(n.created_at);
      if (!out.length || out[out.length - 1].label !== label) out.push({ label, items: [] });
      out[out.length - 1].items.push(n);
    });
    return out;
  }, [list]);

  useEffect(() => {
    const mq = window.matchMedia?.("(min-width: 1280px)");
    if (!mq) return;
    const on = () => setWide(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  // On wide screens something is always selected (the newest by default).
  useEffect(() => {
    if (selected && list.some((n) => n.id === selected)) return;
    setSelected(wide && list.length ? list[0].id : null);
  }, [list, selected, wide]);
  const current = all.find((n) => n.id === selected);

  const filters = [{ key: null, label: "All", count: all.length }, ...kinds.map((k) => ({ key: k, label: kindLabel(k), count: all.filter((n) => n.kind === k).length }))];

  return (
    <>
      <PageHead kicker="Every decision the owner should see" title="Feed" />
      <div className="scrollbar-none reveal -mx-4 mb-6 flex gap-1 overflow-x-auto px-4 sm:mx-0 sm:flex-wrap sm:px-0" role="group" aria-label="Filter by kind">
        {filters.map((f) => {
          const on = kind === f.key;
          return (
            <button
              key={f.key ?? "all"}
              aria-pressed={on}
              onClick={() => setKind(f.key)}
              className={`flex shrink-0 items-center gap-2 rounded-full px-3.5 py-1.5 text-sm transition ${on ? "bg-white/10 text-fog ring-1 ring-white/15" : "text-mute hover:text-fog"}`}
            >
              {f.label}
              <span className={`num rounded-full px-1.5 text-[11px] ${on ? "bg-lime text-ink" : "bg-white/5 text-dim"}`}>{f.count}</span>
            </button>
          );
        })}
      </div>

      <ErrorNote error={error} />
      {loading && <Loading rows={6} />}
      {data && list.length === 0 && <Empty title="Nothing yet" text="Everything Combox tells you about shows up here: firings, blocked payouts, contracts, and daily digests." />}

      {list.length > 0 && (
        <div className="grid grid-cols-[minmax(0,1fr)] gap-4 xl:grid-cols-[minmax(0,1fr)_26rem]">
          <div className="min-w-0 space-y-6">
            {groups.map((g) => (
              <section key={g.label}>
                <h2 className="mb-2 px-1 font-mono text-[11px] uppercase tracking-[0.18em] text-dim">{g.label}</h2>
                <Panel className="p-1.5">
                  <ul className="space-y-0.5">
                    {g.items.map((n) => (
                      <li key={n.id}>
                        <Row n={n} active={n.id === selected} onClick={() => setSelected(n.id === selected && !wide ? null : n.id)} />
                        {n.id === selected && !wide && (
                          <div className="px-3 pb-4">
                            <Detail n={n} />
                          </div>
                        )}
                      </li>
                    ))}
                  </ul>
                </Panel>
              </section>
            ))}
          </div>
          {wide && (
            <aside>
              <div className="sticky top-6">
                {current ? (
                  <Panel className="p-6">
                    <Detail n={current} large />
                  </Panel>
                ) : (
                  <Panel className="p-6 text-sm text-mute">Select an event to see its details.</Panel>
                )}
              </div>
            </aside>
          )}
        </div>
      )}
    </>
  );
}

function Row({ n, active, onClick }) {
  const tone = toneOf(n.kind);
  return (
    <button
      onClick={onClick}
      aria-expanded={active}
      className={`flex w-full items-center gap-3.5 rounded-2xl px-3 py-3 text-left transition ${active ? "bg-white/7" : "hover:bg-white/3.5"}`}
    >
      <span className={`flex shrink-0 items-center justify-center rounded-xl ring-1 ring-inset size-10 ${TILE[tone]}`}>
        <Icon name={iconFor(n.kind)} size={18} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="line-clamp-2 text-[15px] text-fog">{tidy(n.subject)}</span>
        <span className="mt-0.5 flex min-w-0 items-center gap-2 text-xs text-mute">
          <span className={`shrink-0 font-medium ${TEXT[tone]}`}>{kindLabel(n.kind)}</span>
          {n.body?.reason && <span className="truncate">· {tidy(n.body.reason)}</span>}
        </span>
      </span>
      <span className="shrink-0 text-xs text-dim" title={when(n.created_at)}>{ago(n.created_at)}</span>
      <Icon name="chevronRight" size={16} className={`shrink-0 text-dim transition xl:hidden ${active ? "rotate-90" : ""}`} />
    </button>
  );
}

function Detail({ n, large }) {
  const { navigate } = useApp();
  const tone = toneOf(n.kind);
  const metrics = n.body?.metrics && typeof n.body.metrics === "object" ? Object.entries(n.body.metrics) : [];
  const cat = categoryOf(n.kind);
  return (
    <div className={large ? "" : "rounded-2xl bg-black/25 p-4 ring-1 ring-white/5"}>
      {large && (
        <div className="mb-4 flex items-center gap-3">
          <span className={`flex items-center justify-center rounded-xl ring-1 ring-inset size-10 ${TILE[tone]}`}>
            <Icon name={iconFor(n.kind)} size={18} />
          </span>
          <div>
            <p className={`text-xs font-medium ${TEXT[tone]}`}>{kindLabel(n.kind)}</p>
            <p className="text-xs text-dim">{when(n.created_at)}</p>
          </div>
        </div>
      )}
      {large && <h2 className="font-display text-lg/snug font-semibold tracking-tight">{tidy(n.subject)}</h2>}
      {!large && <p className="mb-2 text-xs text-dim">{when(n.created_at)}</p>}
      {n.body?.reason && <p className={`${large ? "mt-3" : ""} text-sm/relaxed text-fog/85`}>{tidy(n.body.reason)}</p>}

      {metrics.length > 0 && (
        <dl className="mt-5 grid grid-cols-2 gap-2">
          {metrics.map(([k, v]) => (
            <div key={k} className="min-w-0 rounded-xl bg-white/4 px-3 py-2.5 ring-1 ring-white/5">
              <dt className="truncate text-[11px] text-mute">{human(k)}</dt>
              <dd className="num mt-0.5 text-base text-fog">{tidy(typeof v === "object" ? JSON.stringify(v) : String(v))}</dd>
            </div>
          ))}
        </dl>
      )}

      <dl className="mt-5 space-y-2 border-t border-white/5 pt-4 text-sm">
        <div className="flex justify-between gap-4">
          <dt className="text-mute">Owner email</dt>
          <dd className="text-right">{n.email_sent_at ? `Sent ${ago(n.email_sent_at)}` : n.attempts ? `Retrying (${n.attempts} attempts)` : "Not sent yet"}</dd>
        </div>
        {n.audit_id && (
          <div className="flex justify-between gap-4">
            <dt className="text-mute">Audit record</dt>
            <dd className="font-mono text-xs text-fog/85" title={n.audit_id}>{n.audit_id.slice(0, 8)}…{n.audit_id.slice(-4)}</dd>
          </div>
        )}
      </dl>

      {/* Everything the notification carries, as sent. */}
      <details className="group mt-4 rounded-xl bg-black/30 ring-1 ring-white/5">
        <summary className="flex cursor-pointer list-none items-center gap-2 rounded-xl px-3 py-2 text-xs text-mute transition hover:text-fog [&::-webkit-details-marker]:hidden">
          <Icon name="chevronRight" size={14} className="transition group-open:rotate-90" />
          Raw details
        </summary>
        <pre className="max-h-80 overflow-auto border-t border-white/5 px-3 py-3 font-mono text-[11px]/relaxed text-fog/85">{JSON.stringify(n.body, null, 2)}</pre>
      </details>

      {(cat === "money" || cat === "people" || cat === "interviews") && (
        <div className="mt-5 flex flex-wrap gap-2">
          {cat === "money" && <Jump onClick={() => navigate("approvals")}>Open payroll</Jump>}
          {cat === "money" && <Jump onClick={() => navigate("spending")}>Monthly cap</Jump>}
          {cat === "people" && <Jump onClick={() => navigate("team")}>Open org chart</Jump>}
          {cat === "interviews" && <Jump onClick={() => navigate("meetings")}>Open interviews</Jump>}
        </div>
      )}
      {n.kind === "kill_switch" && (
        <div className="mt-5 flex flex-wrap gap-2">
          <Jump onClick={() => navigate("kill")}>Kill switch</Jump>
        </div>
      )}
    </div>
  );
}

function Jump({ onClick, children }) {
  return (
    <button onClick={onClick} className="inline-flex items-center gap-1.5 rounded-full bg-white/5 px-3.5 py-1.5 text-xs font-medium ring-1 ring-white/10 transition hover:bg-white/10">
      {children} <Icon name="arrow" size={14} />
    </button>
  );
}
