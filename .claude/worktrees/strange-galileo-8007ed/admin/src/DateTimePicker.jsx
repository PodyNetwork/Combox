// Compact date (and optional time) picker. The popover floats under its field
// with fixed positioning (flipping above when there is no room), so it is
// never clipped by a scrolling modal and never pushes the form around. It
// stays inside the field's DOM, so a modal's focus trap and backdrop logic
// treat it as part of the dialog. Values use the same strings as the native
// inputs it replaces: "YYYY-MM-DDTHH:mm" (local) with time, "YYYY-MM-DD" without.
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Icon } from "./ui.jsx";

const pad = (n) => String(n).padStart(2, "0");
const toDateStr = (d) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
const parse = (v) => {
  if (!v) return null;
  const [date, time = "00:00"] = v.split("T");
  const [y, m, d] = date.split("-").map(Number);
  const [hh, mm] = time.split(":").map(Number);
  const out = new Date(y, m - 1, d, hh || 0, mm || 0);
  return Number.isNaN(out.getTime()) ? null : out;
};
const sameDay = (a, b) => a && b && a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
const startOfDay = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate());
const nowHHMM = () => `${pad(new Date().getHours())}:${pad(new Date().getMinutes())}`;
const SLOTS = Array.from({ length: 96 }, (_, i) => `${pad(Math.floor(i / 4))}:${pad((i % 4) * 15)}`);
const WEEK = ["M", "T", "W", "T", "F", "S", "S"];
const GAP = 6;
const EDGE = 8;

export default function DateTimePicker({ label, value, onChange, time = true, allowPast = false, placeholder, hint }) {
  const labelId = useId();
  const [open, setOpen] = useState(false);
  const selected = parse(value);
  const [view, setView] = useState(() => {
    const base = selected || new Date();
    return new Date(base.getFullYear(), base.getMonth(), 1);
  });
  const [pos, setPos] = useState(null);
  const ref = useRef(null);
  const fieldRef = useRef(null);
  const popRef = useRef(null);
  const slotsRef = useRef(null);
  const today = startOfDay(new Date());
  const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const hhmm = selected ? `${pad(selected.getHours())}:${pad(selected.getMinutes())}` : "10:00";

  // Place the popover under the field, or above it when it would run off the
  // bottom of the window; keep it inside the window horizontally.
  const place = () => {
    const field = fieldRef.current?.getBoundingClientRect();
    const pop = popRef.current;
    if (!field || !pop) return;
    const w = pop.offsetWidth;
    const h = pop.offsetHeight;
    const roomBelow = window.innerHeight - field.bottom - GAP - EDGE;
    const roomAbove = field.top - GAP - EDGE;
    const top = roomBelow >= h || roomAbove < h ? field.bottom + GAP : field.top - GAP - h;
    const left = Math.max(EDGE, Math.min(field.left, window.innerWidth - w - EDGE));
    setPos({ top, left });
  };

  useLayoutEffect(() => {
    if (!open) return setPos(null);
    place();
  }, [open, view]);

  useEffect(() => {
    if (!open) return;
    const esc = (e) => e.key === "Escape" && (e.stopPropagation(), setOpen(false));
    const away = (e) => !ref.current?.contains(e.target) && setOpen(false);
    document.addEventListener("keydown", esc, true);
    document.addEventListener("pointerdown", away);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    // Bring the chosen time (rounded down to its 15-minute slot), or 10:00, into view.
    const list = slotsRef.current;
    const [h, m] = hhmm.split(":").map(Number);
    const slot = list?.querySelector(`[data-slot="${pad(h)}:${pad(Math.floor(m / 15) * 15)}"]`);
    if (slot) list.scrollTop = slot.offsetTop - list.clientHeight / 2 + slot.offsetHeight / 2;
    return () => {
      document.removeEventListener("keydown", esc, true);
      document.removeEventListener("pointerdown", away);
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open]);

  // The next free 15-minute slot today, so "today" never lands in the past.
  const nextSlot = () => {
    const n = new Date();
    const mins = Math.ceil((n.getHours() * 60 + n.getMinutes() + 1) / 15) * 15;
    return mins >= 24 * 60 ? "23:45" : `${pad(Math.floor(mins / 60))}:${pad(mins % 60)}`;
  };
  const emit = (d, t = hhmm) => {
    let at = t;
    if (!allowPast && sameDay(d, today) && at <= nowHHMM()) at = nextSlot();
    onChange(time ? `${toDateStr(d)}T${at}` : toDateStr(d));
  };
  const pickDay = (d) => {
    emit(d);
    if (!time) setOpen(false);
  };
  // A time finishes the choice: close once it is picked.
  const pickTime = (t) => {
    emit(selected || today, t);
    setOpen(false);
  };
  const goToday = () => {
    setView(new Date(today.getFullYear(), today.getMonth(), 1));
    pickDay(today);
  };

  const days = useMemo(() => {
    const first = new Date(view.getFullYear(), view.getMonth(), 1);
    const offset = (first.getDay() + 6) % 7; // Monday first
    // Only as many weeks as the month needs, so the grid stays short.
    const weeks = Math.ceil((offset + new Date(view.getFullYear(), view.getMonth() + 1, 0).getDate()) / 7);
    return Array.from({ length: weeks * 7 }, (_, i) => new Date(view.getFullYear(), view.getMonth(), i - offset + 1));
  }, [view]);

  const dateText = (d) => d.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short", year: "numeric" });
  const display = selected ? (time ? `${dateText(selected)} · ${hhmm}` : dateText(selected)) : null;
  const isPast = (d) => !allowPast && startOfDay(d) < today;
  const slotPast = (t) => !allowPast && selected && sameDay(selected, today) && t < nowHHMM();

  return (
    <div ref={ref} className="text-sm">
      <span id={labelId} className="text-mute">{label}</span>
      <div ref={fieldRef} className="relative mt-1.5">
        <button
          type="button"
          onClick={() => setOpen(!open)}
          aria-expanded={open}
          aria-haspopup="dialog"
          aria-labelledby={`${labelId} ${labelId}-value`}
          className={`flex w-full items-center gap-2.5 rounded-xl border bg-black/40 px-3.5 py-2.5 text-left transition ${open ? "border-lime/60 ring-4 ring-lime/10" : "border-white/10 hover:border-white/20"}`}
        >
          <Icon name="calendar" size={15} className={selected ? "text-lime" : "text-dim"} />
          <span id={`${labelId}-value`} className={`num min-w-0 flex-1 truncate pr-6 ${selected ? "text-fog" : "text-dim"}`}>
            {display || placeholder || (time ? "Pick a date and time" : "Pick a date")}
          </span>
        </button>
        {selected && (
          <button
            type="button"
            aria-label="Clear date"
            onClick={() => onChange("")}
            className="absolute right-2.5 top-1/2 -translate-y-1/2 rounded-full p-1 text-dim hover:bg-white/10 hover:text-fog"
          >
            <Icon name="x" size={14} />
          </button>
        )}
      </div>
      {hint && <span className="mt-1 block text-xs text-dim">{hint}</span>}

      {open && (
        <div
          ref={popRef}
          role="dialog"
          aria-label={time ? "Choose date and time" : "Choose date"}
          style={{ top: pos?.top ?? 0, left: pos?.left ?? 0, visibility: pos ? "visible" : "hidden" }}
          className="pop fixed z-50 flex overflow-hidden rounded-xl border border-white/10 bg-raised shadow-[0_16px_48px_-12px_rgb(0_0_0/0.75)]"
        >
          <div className="w-[15rem] p-2.5">
            <div className="mb-1 flex items-center gap-0.5">
              <p className="mr-auto pl-1.5 text-[13px] font-medium text-fog">
                {view.toLocaleString(undefined, { month: "long" })} <span className="text-dim">{view.getFullYear()}</span>
              </p>
              <button type="button" aria-label="Previous month" onClick={() => setView(new Date(view.getFullYear(), view.getMonth() - 1, 1))} className="rounded-md p-1 text-dim hover:bg-white/10 hover:text-fog">
                <Icon name="chevronLeft" size={14} />
              </button>
              <button type="button" aria-label="Next month" onClick={() => setView(new Date(view.getFullYear(), view.getMonth() + 1, 1))} className="rounded-md p-1 text-dim hover:bg-white/10 hover:text-fog">
                <Icon name="chevronRight" size={14} />
              </button>
            </div>
            <div className="grid grid-cols-7 gap-y-0.5 text-center">
              {WEEK.map((w, i) => (
                <span key={i} className="pb-1 text-[10px] font-medium text-dim">{w}</span>
              ))}
              {days.map((d) => {
                const inMonth = d.getMonth() === view.getMonth();
                const sel = sameDay(d, selected);
                const isToday = sameDay(d, today);
                const past = isPast(d);
                return (
                  <button
                    key={d.toISOString()}
                    type="button"
                    disabled={past}
                    onClick={() => pickDay(d)}
                    aria-pressed={sel}
                    aria-current={isToday ? "date" : undefined}
                    aria-label={d.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" })}
                    className={`num relative mx-auto flex size-7.5 items-center justify-center rounded-lg text-xs transition-colors ${
                      sel
                        ? "bg-lime font-semibold text-ink"
                        : past
                          ? "cursor-default text-dim/30"
                          : `${inMonth ? "text-fog" : "text-dim/60"} hover:bg-white/8`
                    }`}
                  >
                    {d.getDate()}
                    {isToday && !sel && <span className="absolute bottom-[3px] size-1 rounded-full bg-lime" />}
                  </button>
                );
              })}
            </div>
            <div className="mt-1.5 flex items-center gap-2 border-t border-white/5 px-1.5 pt-2">
              <button type="button" onClick={goToday} className="text-[11px] font-medium text-lime hover:underline">
                Today
              </button>
              {time && <span className="ml-auto truncate text-[10px] text-dim" title="Your time zone">{tz}</span>}
            </div>
          </div>

          {time && (
            // The list is absolutely placed so the calendar alone sets the height.
            <div className="relative w-[4.5rem] border-l border-white/5">
              <p className="pt-3 text-center text-[10px] font-medium text-dim">Time</p>
              <div ref={slotsRef} className="scrollbar-none absolute inset-x-0 bottom-0 top-8 overflow-y-auto px-1.5 pb-2">
                {SLOTS.map((t) => {
                  const on = selected && t === hhmm;
                  const off = slotPast(t);
                  return (
                    <button
                      key={t}
                      type="button"
                      data-slot={t}
                      disabled={off}
                      onClick={() => pickTime(t)}
                      className={`num block w-full rounded-md py-1 text-center text-xs transition-colors ${on ? "bg-lime font-semibold text-ink" : off ? "cursor-default text-dim/30" : "text-mute hover:bg-white/8 hover:text-fog"}`}
                    >
                      {t}
                    </button>
                  );
                })}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
