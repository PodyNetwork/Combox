// Shared UI primitives for the Combox control room, and the app context that
// tells every page which company it is looking at.
import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import { request } from "./api.js";

// ---------------------------------------------------------------- context

/**
 * Set by the shell: { session, platform, me, company, slug, base, reloadMe,
 * navigate(page), notify(text, tone), newCompany(), refreshStatus() }.
 * `base` is the company's API root (`<API>/t/<slug>` on the platform).
 */
export const AppCtx = createContext(null);
export const useApp = () => useContext(AppCtx);

/** Call the current company's API (path like "/admin/cap"). */
export function useCompanyApi() {
  const { session, base } = useApp();
  return useCallback((path, opts) => request(session, `${base}${path}`, opts), [session, base]);
}

/** Link to a page on the company's participant site (jobs board, freelancer portal). */
export function useSiteUrl() {
  const { platform, slug } = useApp();
  const root = (import.meta.env.VITE_AXOM_MEET_WEB_URL || "http://localhost:5173").replace(/\/$/, "");
  return (path) => `${root}${platform ? `/t/${slug}` : ""}${path}`;
}

export const usd = (v, digits = 2) =>
  "$" + Number(v ?? 0).toLocaleString(undefined, { minimumFractionDigits: digits, maximumFractionDigits: digits });

export const ago = (iso) => {
  if (!iso) return "";
  const s = (Date.now() - new Date(iso).getTime()) / 1000;
  const f = (n, u) => `${Math.round(n)}${u} ${s < 0 ? "from now" : "ago"}`;
  const a = Math.abs(s);
  if (a < 60) return "just now";
  if (a < 3600) return f(a / 60, "m");
  if (a < 86400) return f(a / 3600, "h");
  return f(a / 86400, "d");
};

export const human = (s) => String(s ?? "").replaceAll("_", " ");
export const label = human;
export const when = (v) => (v ? new Date(v).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "");
export const day = (v) => (v ? new Date(v).toLocaleDateString(undefined, { dateStyle: "medium" }) : "");

// Percentage of a whole, safe for zero/missing values (never NaN or Infinity).
export const pctOf = (part, whole) => {
  const w = Number(whole);
  const v = (Number(part) / w) * 100;
  return w > 0 && Number.isFinite(v) ? Math.min(100, Math.max(0, v)) : 0;
};
// NEAR equivalent of a USD amount, or null when there's no usable rate.
export const nearOf = (usdAmount, usdPerNear) => {
  const r = Number(usdPerNear);
  return r > 0 ? (Number(usdAmount) / r).toFixed(3) : null;
};
// Calendar dates from the API (YYYY-MM-DD) are local days, not UTC instants.
export const localDate = (ymd) => {
  const [y, m, d] = String(ymd).split("-").map(Number);
  return new Date(y, (m || 1) - 1, d || 1);
};

// Long machine identifiers (NEAR implicit accounts are 64 hex chars, tx
// hashes ~44 base58 chars) don't wrap and break layouts. Show them short;
// keep the full value in a tooltip and one click away on the clipboard.
const LONG_ID = /\b(?:[0-9a-fA-F]{32,}|[1-9A-HJ-NP-Za-km-z]{40,})\b/g;
export const shortId = (s, head = 8, tail = 6) => {
  const v = String(s ?? "");
  // Readable names (alice.testnet, forge-agent.testnet) stay whole; only
  // long machine IDs get shortened.
  return v.length > 28 && !/\s/.test(v) ? `${v.slice(0, head)}…${v.slice(-tail)}` : v;
};
// Shorten any long IDs embedded in free text (notification subjects, reasons).
export const tidy = (text) => String(text ?? "").replace(LONG_ID, (m) => shortId(m));

// ---------------------------------------------------------------- data

/**
 * Load a company endpoint; returns { data, error, loading, reload }. A null
 * path loads nothing. With { poll: ms } it refreshes on that interval; a
 * failed refresh keeps the data already on screen.
 */
export function useLoad(path, { poll } = {}) {
  const call = useCompanyApi();
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  // Only the latest request may update state, so a slow poll that started
  // before an action can't overwrite the fresher result after it.
  const seq = useRef(0);
  const reload = useCallback(() => {
    if (!path) return;
    const id = ++seq.current;
    call(path).then(
      (d) => {
        if (id !== seq.current) return;
        setData(d);
        setError(null);
      },
      (e) => id === seq.current && setError(e.message),
    );
  }, [call, path]);
  useEffect(() => {
    reload();
    if (!poll || !path) return;
    const t = setInterval(reload, poll);
    return () => clearInterval(t);
  }, [reload, poll, path]);
  return { data, error, reload, loading: !!path && !data && !error };
}

/** The company's approval threshold in USD (its own, stricter rule if set). */
export function useThreshold() {
  const { data } = useLoad("/admin/policy");
  return data ? Number(data.effective.approval_threshold_usd) : 50;
}

// ---------------------------------------------------------------- toasts

const ToastCtx = createContext(() => {});
export const useToast = () => useContext(ToastCtx);

export function ToastProvider({ children }) {
  const [items, setItems] = useState([]);
  const push = useCallback((text, tone = "ok") => {
    const id = Math.random().toString(36).slice(2);
    setItems((x) => [...x, { id, text, tone }]);
    setTimeout(() => setItems((x) => x.filter((i) => i.id !== id)), 4200);
  }, []);
  return (
    <ToastCtx.Provider value={push}>
      {children}
      <div className="fixed bottom-4 right-4 z-50 flex max-w-sm flex-col gap-2" role="status" aria-live="polite">
        {items.map((t) => (
          <div key={t.id} className={`reveal rounded-2xl px-4 py-3 text-sm font-medium shadow-2xl ${t.tone === "error" ? "bg-ember text-ink" : t.tone === "warning" ? "bg-amber text-ink" : "bg-lime text-ink"}`}>
            {t.text}
          </div>
        ))}
      </div>
    </ToastCtx.Provider>
  );
}

// ---------------------------------------------------------------- layout

const REDUCED = typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
export const MOTION = !REDUCED;

// Panels carry a soft spotlight that follows the pointer.
export function Panel({ children, className = "", tone, i = 0, tilt, as: Tag = "section", style, ...rest }) {
  const ref = useRef(null);
  const move = (e) => {
    const el = ref.current;
    const r = el?.getBoundingClientRect();
    if (!r) return;
    el.style.setProperty("--mx", `${e.clientX - r.left}px`);
    el.style.setProperty("--my", `${e.clientY - r.top}px`);
  };
  const toneClass = tone === "lime" ? "panel-lime" : tone === "ember" ? "panel-ember" : "";
  return (
    <Tag ref={ref} onPointerMove={move} style={{ "--i": i, ...style }} className={`panel reveal ${toneClass} ${className}`} {...rest}>
      {children}
    </Tag>
  );
}

// Headline whose letters rise into place once.
export function KineticText({ text, className = "", delay = 0 }) {
  const words = String(text).split(" ");
  let n = delay;
  return (
    <span className={`kinetic ${className}`} aria-label={text}>
      {words.map((w, wi) => (
        <span key={wi} className="inline-block whitespace-nowrap" aria-hidden="true">
          {[...w].map((ch, ci) => (
            <span key={ci} className="kinetic-ch" style={{ "--c": n++ }}>{ch}</span>
          ))}
          {wi < words.length - 1 && <span className="kinetic-ch" style={{ "--c": n++ }}>&nbsp;</span>}
        </span>
      ))}
    </span>
  );
}

// Numbers that count up (eased) when they first render or change.
export function CountUp({ value, format = (v) => String(Math.round(v)), duration = 1100 }) {
  const [shown, setShown] = useState(MOTION ? 0 : Number(value));
  const from = useRef(0);
  useEffect(() => {
    const target = Number(value) || 0;
    if (!MOTION) return setShown(target);
    const start = performance.now();
    const a = from.current;
    let raf;
    const tick = (t) => {
      const k = Math.min(1, (t - start) / duration);
      const e = 1 - Math.pow(1 - k, 4);
      setShown(a + (target - a) * e);
      if (k < 1) raf = requestAnimationFrame(tick);
      else from.current = target;
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [value, duration]);
  return <>{format(shown)}</>;
}

export function Eyebrow({ children, className = "" }) {
  return <p className={`font-mono text-[11px] uppercase tracking-[0.18em] opacity-75 ${className}`}>{children}</p>;
}

export function PanelHead({ eyebrow, title, action, className = "" }) {
  return (
    <header className={`mb-4 flex items-start gap-3 ${className}`}>
      <div className="mr-auto min-w-0">
        {eyebrow && <Eyebrow>{eyebrow}</Eyebrow>}
        {title && <h2 className="mt-1 font-display text-lg font-medium tracking-tight">{title}</h2>}
      </div>
      {action}
    </header>
  );
}

export function PageHead({ kicker, title, children }) {
  return (
    <div className="reveal mb-6 flex flex-wrap items-end gap-4">
      <div className="mr-auto">
        <Eyebrow className="text-lime opacity-100">{kicker}</Eyebrow>
        <h1 className="mt-2 font-display text-4xl font-semibold leading-none tracking-tight sm:text-6xl"><KineticText text={title} /></h1>
      </div>
      {children}
    </div>
  );
}

export function Orb({ className = "", halted = false }) {
  return (
    <div className={`orb ${halted ? "orb-halted" : ""} ${className}`} aria-hidden="true">
      <div className="orb-halo" />
      <div className="orb-ring" />
      <div className="orb-core" />
    </div>
  );
}

// A giant word with the orb punching through it: solid letters behind the
// orb, an outlined copy in front, so the sphere looks embedded in the type.
export function Breakthrough({ word = "COMBOX", halted = false, className = "" }) {
  // 27cqw fits four letters; longer words shrink so they still fill the width.
  return (
    <div className={`breakthrough select-none ${className}`} style={{ "--bt-size": `${Math.min(27, 108 / word.length)}cqw` }} aria-hidden="true">
      <p className="bt-word text-fog/95">{word}</p>
      <Orb halted={halted} className="absolute left-1/2 top-1/2 w-[46%] -translate-1/2" />
      <p className="bt-word bt-front">{word}</p>
    </div>
  );
}

// ---------------------------------------------------------------- controls

const BTN = {
  primary: "bg-lime text-ink hover:bg-[#e4ff7a] hover:shadow-[0_8px_30px_-6px_rgba(212,255,58,0.6)]",
  danger: "bg-ember text-ink hover:bg-[#ff7a45] hover:shadow-[0_8px_30px_-6px_rgba(255,90,31,0.6)]",
  ghost: "bg-white/5 text-fog ring-1 ring-inset ring-white/10 hover:bg-white/10",
  ink: "bg-ink text-fog hover:bg-black",
  link: "px-0! text-mute hover:text-fog underline-offset-4 hover:underline",
};

export function Button({ children, variant = "primary", size = "md", className = "", ...rest }) {
  const sz = size === "lg" ? "px-6 py-3.5 text-base" : size === "sm" ? "px-3 py-1.5 text-xs" : "px-4 py-2.5 text-sm";
  return (
    <button
      className={`inline-flex items-center justify-center gap-2 rounded-full font-medium transition-all duration-200 active:scale-[0.97] disabled:pointer-events-none disabled:opacity-40 ${sz} ${BTN[variant]} ${className}`}
      {...rest}
    >
      {children}
    </button>
  );
}

const FIELD = "mt-1.5 w-full rounded-xl border border-white/10 bg-black/40 px-3.5 py-2.5 text-sm text-fog placeholder:text-dim transition focus:border-lime/60 focus:outline-none focus:ring-4 focus:ring-lime/10";

export function Input({ label, value, onChange, type = "text", hint, className = "", ...rest }) {
  return (
    <label className={`block text-sm ${className}`}>
      <span className="text-mute">{label}</span>
      <input type={type} value={value} onChange={(e) => onChange(e.target.value)} className={FIELD} {...rest} />
      {hint && <span className="mt-1 block text-xs text-dim">{hint}</span>}
    </label>
  );
}

export function Textarea({ label, value, onChange, rows = 4, hint, ...rest }) {
  return (
    <label className="block text-sm">
      <span className="text-mute">{label}</span>
      <textarea rows={rows} value={value} onChange={(e) => onChange(e.target.value)} className={`${FIELD} resize-y`} {...rest} />
      {hint && <span className="mt-1 block text-xs text-dim">{hint}</span>}
    </label>
  );
}

// label is shown above the pills; ariaLabel names the group without showing it.
export function Segmented({ options, value, onChange, label, ariaLabel }) {
  return (
    <div className="text-sm">
      {label && <span className="block text-mute">{label}</span>}
      <div role="group" aria-label={label || ariaLabel} className={`${label ? "mt-1.5 " : ""}inline-flex rounded-full bg-black/40 p-1 ring-1 ring-white/10`}>
        {options.map((o) => (
          <button
            type="button"
            key={o.value}
            aria-pressed={value === o.value}
            onClick={() => onChange(o.value)}
            className={`rounded-full px-3 py-1.5 text-xs font-medium transition ${value === o.value ? "bg-lime text-ink" : "text-mute hover:text-fog"}`}
          >
            {o.label}
          </button>
        ))}
      </div>
    </div>
  );
}

const BADGE = {
  lime: "bg-lime/15 text-lime ring-lime/30",
  ember: "bg-ember/15 text-ember ring-ember/30",
  amber: "bg-amber/15 text-amber ring-amber/30",
  violet: "bg-violet/15 text-violet ring-violet/30",
  sky: "bg-sky/15 text-sky ring-sky/30",
  mute: "bg-white/5 text-mute ring-white/10",
};

export function Badge({ children, tone = "mute", dot = false, className = "" }) {
  return (
    <span className={`inline-flex items-center gap-1.5 whitespace-nowrap rounded-full px-2.5 py-0.5 text-[11px] font-medium ring-1 ring-inset ${BADGE[tone] || BADGE.mute} ${className}`}>
      {dot && <span className="pulse-dot rounded-full bg-current size-1.5" />}
      {children}
    </span>
  );
}

export const STATUS_TONE = {
  // tasks
  queued: "mute", assigned: "sky", in_progress: "violet", blocked: "amber", done: "lime", failed: "ember",
  // workers
  candidate: "sky", active: "lime", on_probation: "amber", suspended: "ember", fired: "ember",
  // meetings
  scheduled: "sky", dispatched: "violet", completed: "lime", consent_declined: "ember", cancelled: "mute", expired: "mute",
  // briefs
  open: "sky", submitted: "violet", accepted: "lime", returned: "amber",
  // payouts
  pending_approval: "amber", approved: "sky", sent: "violet", finalized: "lime", refunded: "amber",
  rejected: "ember", expired: "mute", cap_exceeded: "ember",
};

export function StatusBadge({ status, dot }) {
  return <Badge tone={STATUS_TONE[status] || "mute"} dot={dot}>{human(status)}</Badge>;
}

export function Chip({ children }) {
  return <span className="rounded-md bg-white/5 px-2 py-0.5 font-mono text-[11px] text-mute">{children}</span>;
}

export function Loading({ rows = 3 }) {
  return (
    <div className="space-y-2" aria-label="Loading">
      {Array.from({ length: rows }).map((_, i) => (
        <div key={i} className="shimmer h-10 rounded-xl" />
      ))}
    </div>
  );
}

export function ErrorNote({ error }) {
  if (!error) return null;
  return <p className="rounded-xl bg-ember/10 px-3 py-2 text-sm text-ember ring-1 ring-ember/30">{error}</p>;
}

export function Empty({ title, text }) {
  return (
    <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-white/10 px-6 py-10 text-center">
      <div className="mb-3 rounded-full bg-linear-to-br from-violet/60 to-lime/40 blur-[1px] size-10" />
      <p className="font-display text-sm">{title}</p>
      {text && <p className="mt-1 max-w-xs text-xs text-mute">{text}</p>}
    </div>
  );
}

export function Modal({ open, onClose, title, kicker, children, wide = false, backdropClose = true }) {
  const dialog = useRef(null);
  // Close on a backdrop click only when the press also started on the
  // backdrop: a drag or a click that began inside the dialog (say, while a
  // date picker shrinks it) must not close it.
  const pressedBackdrop = useRef(false);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => {
    if (!open) return;
    const opener = document.activeElement;
    // Lock page scroll behind the dialog (keep the scrollbar's width).
    const { overflow, paddingRight } = document.body.style;
    const bar = window.innerWidth - document.documentElement.clientWidth;
    document.body.style.overflow = "hidden";
    if (bar > 0) document.body.style.paddingRight = `${bar}px`;
    // Focus the dialog unless a field inside asked for autofocus.
    requestAnimationFrame(() => {
      if (!dialog.current?.contains(document.activeElement)) dialog.current?.focus();
    });
    const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';
    const key = (e) => {
      if (e.key === "Escape") return closeRef.current();
      if (e.key !== "Tab" || !dialog.current) return;
      // Keep Tab inside the dialog.
      const items = [...dialog.current.querySelectorAll(FOCUSABLE)].filter((el) => el.offsetParent !== null);
      if (!items.length) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && (document.activeElement === first || document.activeElement === dialog.current)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("keydown", key);
      document.body.style.overflow = overflow;
      document.body.style.paddingRight = paddingRight;
      if (opener && document.contains(opener)) opener.focus?.();
    };
  }, [open]);
  if (!open) return null;
  return (
    <div
      className="fixed inset-0 z-40 flex items-end justify-center bg-black/70 p-0 backdrop-blur-sm sm:items-center sm:p-6"
      onPointerDown={(e) => (pressedBackdrop.current = e.target === e.currentTarget)}
      onClick={(e) => {
        if (backdropClose && pressedBackdrop.current && e.target === e.currentTarget) onClose();
        pressedBackdrop.current = false;
      }}
    >
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
        className={`reveal max-h-[92vh] w-full overflow-y-auto overscroll-contain rounded-t-3xl outline-none border border-white/10 bg-panel p-6 shadow-2xl sm:rounded-3xl ${wide ? "sm:max-w-3xl" : "sm:max-w-lg"}`}
      >
        <div className="mb-5 flex items-start gap-3">
          <div className="mr-auto min-w-0">
            {kicker && <Eyebrow className="text-lime opacity-100">{kicker}</Eyebrow>}
            <h2 className="mt-1 font-display text-xl font-semibold tracking-tight">{title}</h2>
          </div>
          <button onClick={onClose} className="rounded-full p-2 text-mute hover:bg-white/5 hover:text-fog" aria-label="Close">
            <Icon name="x" />
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}

export function Avatar({ name = "?", kind, size = 40, tag = true }) {
  const initials = name.split(/\s+/).map((p) => p[0]).slice(0, 2).join("").toUpperCase();
  const ai = kind === "ai_agent";
  return (
    <span
      className="relative inline-flex shrink-0 items-center justify-center rounded-full font-display text-[11px] font-semibold text-ink"
      style={{
        width: size,
        height: size,
        background: ai
          ? "radial-gradient(circle at 30% 25%, #fff 0, #c9bbff 18%, #8b6cff 50%, #2a1f66 100%)"
          : "radial-gradient(circle at 30% 25%, #fff 0, #c5f1ff 18%, #5fd4ff 50%, #0c3a4d 100%)",
      }}
      title={ai ? "AI agent" : "Human freelancer"}
    >
      {initials}
      {ai && tag && <span className="absolute -bottom-0.5 -right-0.5 rounded-full bg-ink px-1 font-mono text-[8px] text-violet ring-1 ring-violet/50">AI</span>}
    </span>
  );
}

// An address or ID: short form, full value on hover, copy on click.
export function Addr({ value, className = "" }) {
  const toast = useToast();
  const full = String(value ?? "");
  const copy = () => navigator.clipboard?.writeText(full).then(() => toast("Copied"), () => toast("Copy failed", "error"));
  return (
    <span className={`inline-flex min-w-0 max-w-full items-center gap-1.5 font-mono ${className}`} title={full}>
      <span className="truncate">{shortId(full)}</span>
      {full.length > 28 && (
        <button type="button" onClick={copy} className="shrink-0 rounded-md p-1 text-dim transition hover:bg-white/10 hover:text-fog" aria-label="Copy full address">
          <Icon name="copy" size={13} />
        </button>
      )}
    </span>
  );
}

export function CopyField({ value, label }) {
  const toast = useToast();
  const copy = () => navigator.clipboard?.writeText(value).then(() => toast("Copied"), () => toast("Copy failed", "error"));
  return (
    <div className="flex items-center gap-2 rounded-xl bg-black/40 p-1.5 pl-3 ring-1 ring-white/10">
      {label && <span className="shrink-0 text-xs text-mute">{label}</span>}
      <code className="min-w-0 flex-1 truncate font-mono text-xs">{value}</code>
      <Button size="sm" variant="ghost" onClick={copy}>Copy</Button>
    </div>
  );
}

// A value to send: shown in full (so it can be checked), copied in one click.
export function CopyBlock({ label, value, big = false }) {
  const toast = useToast();
  const [copied, setCopied] = useState(false);
  const timer = useRef(null);
  useEffect(() => () => clearTimeout(timer.current), []);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      clearTimeout(timer.current);
      timer.current = setTimeout(() => setCopied(false), 1500);
    } catch {
      toast("Copy failed", "error");
    }
  };
  return (
    <div className="flex items-center gap-3 rounded-2xl bg-black/40 p-4 ring-1 ring-white/10">
      <div className="min-w-0 flex-1">
        <p className="text-xs text-mute">{label}</p>
        <p className={`mt-1 break-all font-mono text-fog ${big ? "text-2xl font-semibold tracking-tight" : "text-sm"}`}>{value}</p>
      </div>
      <Button size="sm" variant="ghost" onClick={copy}>
        <Icon name={copied ? "check" : "copy"} size={14} />
        {copied ? "Copied" : "Copy"}
        <span className="sr-only">{label}</span>
      </Button>
    </div>
  );
}

// ---------------------------------------------------------------- icons

const PATHS = {
  tools: "M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.5 2.5-2.4-.6-.6-2.4z",
  hiring: "M3 8h18v12H3zM8 8V5h8v3M3 13h18",
  integrations: "M9 3v5M15 3v5M6 8h12v3a6 6 0 0 1-12 0zM12 17v4",
  billing: "M6 3h12v18l-3-2-3 2-3-2-3 2zM9 8h6M9 12h6",
  members: "M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM2 21v-1a6 6 0 0 1 12 0v1M16 3.5a4 4 0 0 1 0 7.5M18 15a6 6 0 0 1 4 5v1",
  settings: "M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12M14 4v4M8 10v4M16 16v4",
  platform: "M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z",
  switch: "M8 9l4-4 4 4M8 15l4 4 4-4",
  building: "M4 21V5l8-2v18M12 9h8v12M7 8h2M7 12h2M7 16h2M15 13h2M15 17h2M2 21h20",
  external: "M14 4h6v6M20 4l-9 9M18 14v6H4V6h6",
  download: "M12 4v11M7 10l5 5 5-5M5 20h14",
  overview: "M3 3h7v9H3zM14 3h7v5h-7zM14 12h7v9h-7zM3 16h7v5H3z",
  org: "M12 3v5M5 13v-2h14v2M5 13v3M12 8v8M19 13v3M3 16h4v5H3zM10 16h4v5h-4zM17 16h4v5h-4zM9 3h6v5H9z",
  tasks: "M4 4h4v16H4zM10 4h4v10h-4zM16 4h4v7h-4z",
  meetings: "M3 7h12v10H3zM15 10l6-3v10l-6-3",
  payroll: "M3 7h18v12H3zM3 11h18M7 15h3M16 4H6",
  cap: "M4 18a8 8 0 1 1 16 0M12 18l4-6M8 18h8",
  feed: "M6 16V11a6 6 0 1 1 12 0v5l2 2H4zM10 21h4",
  kill: "M12 3v8M6.3 6.3a8 8 0 1 0 11.4 0",
  x: "M6 6l12 12M18 6L6 18",
  plus: "M12 5v14M5 12h14",
  arrow: "M5 12h14M13 6l6 6-6 6",
  check: "M5 12l5 5L20 7",
  out: "M15 4h4v16h-4M10 8l-4 4 4 4M6 12h10",
  bolt: "M13 3L5 14h6l-1 7 8-11h-6z",
  user: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21a8 8 0 0 1 16 0",
  key: "M15 7a4 4 0 1 1-3.5 6L4 20.5V17h3v-3h3l1.5-1.5A4 4 0 0 1 15 7z",
  menu: "M4 7h16M4 12h16M4 17h16",
  copy: "M9 9h11v11H9zM5 15H4V4h11v1",
  wallet: "M4 7h14a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a1 1 0 0 1-1-1V6a2 2 0 0 1 2-2h10v3M16 13.5h1",
  calendar: "M4 6h16v15H4zM4 10h16M8 3v4M16 3v4",
  clock: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 7v5l3 2",
  chevronLeft: "M15 6l-6 6 6 6",
  chevronRight: "M9 6l6 6-6 6",
  minus: "M5 12h14",
  fit: "M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5",
  search: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM20 20l-4-4",
};

export function Icon({ name, size = 18, className = "" }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" className={className} aria-hidden="true">
      <path d={PATHS[name]} />
    </svg>
  );
}
