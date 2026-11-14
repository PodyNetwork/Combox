// Notification bell: unread count, and a quick-look panel of the latest
// owner notifications. The full list lives on the Feed page.
// The API has no "mark as read" endpoint yet, so "seen" is remembered per
// browser (localStorage) only to quiet the badge after you've looked.
import { useEffect, useRef, useState } from "react";
import { Icon, ago, human, tidy } from "./ui.jsx";

const SEEN_KEY = "axom-notifications-seen-at";
const readSeen = () => {
  try {
    return Number(localStorage.getItem(SEEN_KEY)) || 0;
  } catch {
    return 0;
  }
};
const writeSeen = (t) => {
  try {
    localStorage.setItem(SEEN_KEY, String(t));
  } catch {
    /* storage unavailable: the badge just won't remember */
  }
};

export const toneOf = (kind) =>
  /fired|blocked|failed|rejected|exceeded|kill_switch/.test(kind) ? "ember" : /held|probation|pending/.test(kind) ? "amber" : /meeting|summary|interview/.test(kind) ? "violet" : /sent|approved|accepted/.test(kind) ? "lime" : "mute";
const DOT = { ember: "bg-ember", amber: "bg-amber", violet: "bg-violet", lime: "bg-lime", mute: "bg-white/30" };

// Unread = not read on the server and newer than when you last opened the panel.
export function useUnread(feed) {
  const [seen, setSeen] = useState(readSeen);
  const list = feed.data?.notifications || [];
  const unread = list.filter((n) => !n.read_at && new Date(n.created_at).getTime() > seen).length;
  const markSeen = () => {
    const t = Date.now();
    writeSeen(t);
    setSeen(t);
  };
  return { list, unread, markSeen };
}

export function Bell({ feed, unread, list, markSeen, onOpenAll, align = "right" }) {
  const [open, setOpen] = useState(false);
  const ref = useRef(null);
  useEffect(() => {
    if (!open) return;
    const away = (e) => !ref.current?.contains(e.target) && setOpen(false);
    const esc = (e) => e.key === "Escape" && setOpen(false);
    document.addEventListener("pointerdown", away);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("pointerdown", away);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);

  const toggle = () => {
    if (!open) markSeen();
    setOpen(!open);
  };
  const go = () => {
    setOpen(false);
    onOpenAll();
  };

  return (
    <div ref={ref} className="relative">
      <button
        onClick={toggle}
        aria-label={unread ? `Notifications, ${unread} new` : "Notifications"}
        aria-expanded={open}
        className={`relative flex size-9 items-center justify-center rounded-full transition ${open ? "bg-white/10 text-fog" : "text-mute hover:bg-white/5 hover:text-fog"}`}
      >
        <Icon name="feed" size={18} />
        {unread > 0 && (
          <span className="num absolute -right-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-lime px-1 text-[10px] font-semibold text-ink ring-2 ring-ink">
            {unread > 9 ? "9+" : unread}
          </span>
        )}
      </button>

      {open && (
        <div
          role="dialog"
          aria-label="Notifications"
          className={`reveal absolute z-50 mt-2 w-[min(24rem,calc(100vw-2rem))] overflow-hidden rounded-2xl border border-white/10 bg-panel shadow-2xl ${
            align === "right" ? "right-0 top-full" : "left-full top-0 ml-3 mt-0"
          }`}
        >
          <header className="flex items-center justify-between border-b border-white/5 px-4 py-3">
            <p className="text-sm font-semibold">Notifications</p>
            <button onClick={go} className="text-xs text-mute hover:text-fog">Open feed →</button>
          </header>
          {feed.error && <p className="px-4 py-6 text-sm text-ember">{feed.error}</p>}
          {!feed.data && !feed.error && <p className="px-4 py-6 text-sm text-mute">Loading…</p>}
          {feed.data && list.length === 0 && <p className="px-4 py-8 text-center text-sm text-mute">Nothing yet. Firings, blocked payouts and contracts show up here.</p>}
          <ul className="max-h-[min(26rem,70vh)] overflow-y-auto">
            {list.slice(0, 8).map((n) => {
              const tone = toneOf(n.kind);
              return (
                <li key={n.id}>
                  <button onClick={go} className="flex w-full items-start gap-3 px-4 py-3 text-left transition hover:bg-white/4">
                    <span className={`mt-1.5 size-2 shrink-0 rounded-full ${DOT[tone]}`} />
                    <span className="min-w-0 flex-1">
                      <span className={`line-clamp-2 text-sm ${n.read_at ? "text-fog/80" : "font-medium text-fog"}`}>{tidy(n.subject)}</span>
                      <span className="mt-0.5 block truncate text-xs text-mute">
                        {human(n.kind)}
                        {n.body?.reason ? ` · ${tidy(n.body.reason)}` : ""}
                      </span>
                    </span>
                    <span className="shrink-0 text-[11px] text-dim">{ago(n.created_at)}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </div>
  );
}
