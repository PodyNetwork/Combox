// Freelancer portal (/briefs). Human workers paste the brief token the owner
// issued (wk_…), see their briefs, and submit deliverables as links. The
// token stays in this tab's session storage only.
import { useCallback, useEffect, useRef, useState } from "react";
import { API, Breakthrough, Kinetic, Notice, Wordmark, human } from "./ui.jsx";

const KEY = "axom-worker-token";
const read = () => {
  try {
    return sessionStorage.getItem(KEY);
  } catch {
    return null;
  }
};
const write = (v) => {
  try {
    v ? sessionStorage.setItem(KEY, v) : sessionStorage.removeItem(KEY);
  } catch {
    /* storage unavailable: token lives in memory for this page only */
  }
};

async function call(token, path, { method = "GET", body } = {}) {
  let res;
  try {
    res = await fetch(`${API}${path}`, {
      method,
      headers: { Authorization: `Bearer ${token}`, ...(body ? { "Content-Type": "application/json" } : {}) },
      body: body ? JSON.stringify(body) : undefined,
    });
  } catch {
    throw new Error("We couldn't reach the Combox server. Check your connection and try again.");
  }
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
  return data;
}

export default function Portal() {
  const [token, setToken] = useState(read);
  const signOut = () => {
    write(null);
    setToken(null);
  };
  if (!token) return <TokenGate onToken={(t) => { write(t); setToken(t); }} />;
  return <Briefs token={token} onSignOut={signOut} />;
}

function TokenGate({ onToken }) {
  const [value, setValue] = useState("");
  const ok = value.trim().startsWith("wk_");
  return (
    <div className="lobby">
      <section className="lobby-art">
        <Wordmark />
        <Breakthrough word="WORK" />
        <h1>
          <Kinetic text="Your briefs." />
          <br />
          <em><Kinetic text="Your work." delay={12} /></em>
          <br />
          <span className="outline"><Kinetic text="Paid in NEAR." delay={22} /></span>
        </h1>
      </section>
      <section className="lobby-form">
        <form className="inner stack" onSubmit={(e) => { e.preventDefault(); onToken(value.trim()); }}>
          <div>
            <p className="eyebrow">Freelancer portal</p>
            <h2>Open your briefs</h2>
            <p className="muted" style={{ fontSize: 14 }}>Paste the brief token the team sent you. It starts with <code>wk_</code>. Keep it private; it's your key to this portal.</p>
          </div>
          <label className="field">
            Brief token
            <input value={value} onChange={(e) => setValue(e.target.value)} placeholder="wk_…" autoComplete="off" spellCheck="false" autoFocus />
            {value && !ok && <small>Brief tokens start with wk_</small>}
          </label>
          <button className="btn btn-lime btn-lg btn-block" disabled={!ok}>Continue →</button>
        </form>
      </section>
    </div>
  );
}

function Briefs({ token, onSignOut }) {
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  const [active, setActive] = useState(null);
  const [toast, setToast] = useState(null);
  const loaded = useRef(false);
  const load = useCallback(() => {
    call(token, "/intake/briefs").then(
      (d) => {
        loaded.current = true;
        setData(d);
      },
      // A failed refresh keeps what's on screen; only a failed first load is fatal.
      (e) => (loaded.current ? setToast(`Couldn't refresh your briefs: ${e.message}`) : setError(e.message)),
    );
  }, [token]);
  useEffect(load, [load]);
  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 3500);
    return () => clearTimeout(t);
  }, [toast]);

  if (error)
    return (
      <Notice tone="error" title="Can't open your briefs" text={error}>
        <button className="btn btn-ghost" style={{ marginTop: 28 }} onClick={onSignOut}>Use a different token</button>
      </Notice>
    );

  const briefs = data?.briefs || [];
  const count = (s) => briefs.filter((b) => b.status === s).length;
  const todo = briefs.filter((b) => b.status === "open" || b.status === "returned");
  const rest = briefs.filter((b) => !todo.includes(b));

  return (
    <div className="portal">
      <div className="portal-top">
        <Wordmark />
        <span className="spacer" />
        <button className="btn btn-ghost" onClick={onSignOut}>Sign out</button>
      </div>
      <header className="portal-head">
        <div>
          <p className="eyebrow">Freelancer portal</p>
          <h1 style={{ marginTop: 10 }}>
            <Kinetic key={data ? todo.length : "x"} text={data ? (todo.length ? `${todo.length} brief${todo.length > 1 ? "s need" : " needs"} you.` : "All caught up.") : "Loading…"} />
          </h1>
        </div>
        <div className="stats">
          <div className="stat"><b>{count("open") + count("returned")}</b><span>to do</span></div>
          <div className="stat"><b>{count("submitted")}</b><span>in review</span></div>
          <div className="stat"><b style={{ color: "var(--lime)" }}>{count("accepted")}</b><span>accepted</span></div>
        </div>
      </header>

      {data && briefs.length === 0 && <div className="empty">No briefs yet. You'll see them here when the team assigns you work.</div>}
      <div className="briefs">
        {[...todo, ...rest].map((b, i) => (
          <article key={b.task_id} className={`panel brief ${todo.includes(b) ? "open" : ""}`} style={{ "--i": i }}>
            <div className="brief-top">
              <span className={`badge badge-${b.status}`}>{human(b.status)}</span>
              <span className="when">{new Date(b.created_at).toLocaleDateString(undefined, { month: "short", day: "numeric" })}</span>
            </div>
            <p className="text">{b.brief}</p>
            {b.required_skills?.length > 0 && <div className="chips">{b.required_skills.map((s) => <span key={s} className="chip">{s}</span>)}</div>}
            {b.review_note && <p className="note"><b>Reviewer:</b> {b.review_note}</p>}
            {b.deliverable?.url && b.status !== "returned" && <p className="sent" title={b.deliverable.url}>↗ {b.deliverable.url}</p>}
            {todo.includes(b) && (
              <div className="actions">
                <button className="btn btn-lime btn-block" onClick={() => setActive(b)}>{b.status === "returned" ? "Resubmit deliverable" : "Submit deliverable"}</button>
              </div>
            )}
          </article>
        ))}
      </div>
      {active && (
        <Submit
          brief={active}
          token={token}
          onClose={() => setActive(null)}
          onDone={() => {
            setActive(null);
            setToast("Submitted. The team will review it.");
            load();
          }}
        />
      )}
      {toast && <div className="toast" role="status">{toast}</div>}
    </div>
  );
}

function Submit({ brief, token, onClose, onDone }) {
  const [url, setUrl] = useState("");
  const [notes, setNotes] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(null);
  const valid = /^https?:\/\/\S+$/.test(url.trim()) && url.length <= 2000 && notes.length <= 5000;
  const sheet = useRef(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  // Esc closes; Tab stays inside the sheet; focus returns to the opener.
  useEffect(() => {
    const opener = document.activeElement;
    document.body.style.overflow = "hidden";
    const k = (e) => {
      if (e.key === "Escape") return closeRef.current();
      if (e.key !== "Tab" || !sheet.current) return;
      const items = [...sheet.current.querySelectorAll("button:not([disabled]), input, textarea")];
      if (!items.length) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", k);
    return () => {
      window.removeEventListener("keydown", k);
      document.body.style.overflow = "";
      opener?.focus?.();
    };
  }, []);
  const submit = async (e) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await call(token, `/intake/briefs/${brief.task_id}/deliverables`, { method: "POST", body: { url: url.trim(), notes } });
      onDone();
    } catch (err) {
      setError(err.message);
    }
    setBusy(false);
  };
  return (
    <div className="sheet-bg" onClick={onClose}>
      <form ref={sheet} className="sheet stack" role="dialog" aria-modal="true" aria-label="Submit deliverable" onClick={(e) => e.stopPropagation()} onSubmit={submit}>
        <div>
          <p className="eyebrow">Submit deliverable</p>
          <h2>{brief.brief.split("\n")[0]}</h2>
        </div>
        <label className="field">
          Link to your work
          <input type="url" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://…" required autoFocus />
          <small>Figma, Drive, GitHub, anything the team can open.</small>
        </label>
        <label className="field">
          Notes (optional)
          <textarea rows={4} value={notes} onChange={(e) => setNotes(e.target.value)} placeholder="What's included, anything to look at first" />
        </label>
        {error && <p className="error">{error}</p>}
        <div className="row">
          <button type="button" className="btn btn-ghost" onClick={onClose}>Cancel</button>
          <button className="btn btn-lime" disabled={busy || !valid}>{busy ? "Sending…" : "Submit"}</button>
        </div>
      </form>
    </div>
  );
}
