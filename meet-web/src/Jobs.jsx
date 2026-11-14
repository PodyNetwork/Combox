// ForeMan Jobs: the public jobs board. /jobs lists open roles, /jobs/<id>
// shows one with the application form. Applications go to the ForeMan API
// (POST /jobs/<id>/apply); shortlisted applicants get an email invite to an
// AI interview. Hires are paid in NEAR after address and tax-form checks.
import { useEffect, useState } from "react";
import { API, PATH, PREFIX } from "./base.js";
import { Wordmark } from "./ui.jsx";

const label = (s) => s.replace(/_/g, " ");

async function getJson(url, opts) {
  let r;
  try {
    r = await fetch(url, opts);
  } catch {
    throw new Error("We couldn't reach the ForeMan server. Check your connection and try again.");
  }
  const body = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(body.error || `Request failed (HTTP ${r.status})`);
  return body;
}

export default function Jobs() {
  const id = PATH.split("/")[2];
  return (
    <div className="jobs">
      <header className="jobs-head">
        <a href={`${PREFIX}/jobs`} className="brand" aria-label="ForeMan Jobs"><Wordmark /><span className="eyebrow">Jobs</span></a>
        <span>Freelance work, paid in NEAR</span>
      </header>
      {id ? <JobPage id={id} /> : <JobList />}
    </div>
  );
}

function JobList() {
  const [jobs, setJobs] = useState(null);
  const [error, setError] = useState(null);
  useEffect(() => {
    getJson(`${API}/jobs`).then((b) => setJobs(b.jobs)).catch((e) => setError(e.message));
  }, []);
  if (error) return <p className="error">{error}</p>;
  if (!jobs) return <p className="muted">Loading open roles…</p>;
  if (jobs.length === 0) return <div className="empty">No open roles right now. Check back soon.</div>;
  return (
    <ul className="job-list">
      {jobs.map((j) => (
        <li key={j.id}>
          <a href={`${PREFIX}/jobs/${j.id}`}>
            <strong>{j.title}</strong>
            <span className="rate">${Number(j.rate_usd).toFixed(2)} per task</span>
            <Tags items={j.skills} />
          </a>
        </li>
      ))}
    </ul>
  );
}

function Tags({ items }) {
  if (!items?.length) return null;
  return (
    <span className="tags">
      {items.map((s) => (
        <span key={s} className="chip">{label(s)}</span>
      ))}
    </span>
  );
}

function JobPage({ id }) {
  const [job, setJob] = useState(null);
  const [error, setError] = useState(null);
  useEffect(() => {
    getJson(`${API}/jobs/${encodeURIComponent(id)}`).then((b) => setJob(b.job)).catch((e) => setError(e.message));
  }, [id]);
  if (error) return <p className="error">{error} <a href={`${PREFIX}/jobs`}>See open roles</a></p>;
  if (!job) return <p className="muted">Loading…</p>;
  return (
    <article className="job">
      <h1>{job.title}</h1>
      <p className="rate">${Number(job.rate_usd).toFixed(2)} per task, paid in NEAR</p>
      <Tags items={job.skills} />
      {job.description && (
        <>
          <h2>About the role</h2>
          <p className="desc">{job.description}</p>
        </>
      )}
      {job.kpis?.length > 0 && (
        <>
          <h2>How the work is measured</h2>
          <ul>{job.kpis.map((k) => <li key={k}>{label(k)}</li>)}</ul>
        </>
      )}
      <h2>How hiring works</h2>
      <ol>
        <li>Apply below. Shortlisting is by skill match.</li>
        <li>If shortlisted, you get an email with a link to a video interview with an AI interviewer. It tells you it's an AI and asks your consent before recording.</li>
        <li>The best match gets a contract to sign by email.</li>
        <li>Before your first payout: verify a NEAR address you own and submit a W-9 or W-8BEN.</li>
      </ol>
      <ApplyForm jobId={job.id} skills={job.skills} />
    </article>
  );
}

const MAX_CV_BYTES = 5 * 1024 * 1024;

function ApplyForm({ jobId, skills }) {
  const [form, setForm] = useState({ name: "", email: "", skills: "", message: "", accept_privacy: false });
  const [cv, setCv] = useState(null);
  const [state, setState] = useState({ sending: false, error: null, done: null });
  const set = (k) => (e) => setForm({ ...form, [k]: e.target.type === "checkbox" ? e.target.checked : e.target.value });
  const pickCv = (e) => {
    const f = e.target.files?.[0] || null;
    // Checked again on the server; this just saves a wasted upload.
    if (f && f.size > MAX_CV_BYTES) {
      setState({ sending: false, error: "Your CV must be at most 5 MB.", done: null });
      e.target.value = "";
      setCv(null);
      return;
    }
    if (f && !(f.type === "application/pdf" || f.name.toLowerCase().endsWith(".pdf"))) {
      setState({ sending: false, error: "Please upload your CV as a PDF.", done: null });
      e.target.value = "";
      setCv(null);
      return;
    }
    setState((s) => ({ ...s, error: null }));
    setCv(f);
  };

  async function submit(e) {
    e.preventDefault();
    if (!cv) {
      setState({ sending: false, error: "Please attach your CV as a PDF.", done: null });
      return;
    }
    setState({ sending: true, error: null, done: null });
    try {
      const details = { ...form, skills: form.skills.split(",").map((s) => s.trim()).filter(Boolean) };
      const body = new FormData();
      body.append("application", JSON.stringify(details));
      body.append("cv", cv, cv.name);
      // The browser sets the multipart Content-Type (with its boundary).
      const r = await getJson(`${API}/jobs/${jobId}/apply`, { method: "POST", body });
      setState({ sending: false, error: null, done: r.next });
    } catch (err) {
      setState({ sending: false, error: err.message, done: null });
    }
  }

  if (state.done) return <p className="success">Application received. {state.done}</p>;
  return (
    <form className="apply" onSubmit={submit}>
      <h2>Apply</h2>
      <label className="field">Name<input required maxLength={120} value={form.name} onChange={set("name")} autoComplete="name" /></label>
      <label className="field">Email<input required type="email" maxLength={254} value={form.email} onChange={set("email")} autoComplete="email" /></label>
      <label className="field">
        Your skills, comma-separated
        <input value={form.skills} onChange={set("skills")} placeholder={skills.map(label).join(", ")} />
      </label>
      <label className="field">
        CV (PDF, up to 5 MB)
        <input required type="file" accept="application/pdf,.pdf" onChange={pickCv} />
        {cv && <small>{cv.name} · {cv.size < 1024 * 1024 ? `${Math.max(1, Math.round(cv.size / 1024))} KB` : `${(cv.size / 1024 / 1024).toFixed(1)} MB`}</small>}
      </label>
      <label className="field">Short note (optional)<textarea maxLength={5000} rows={5} value={form.message} onChange={set("message")} /></label>
      <label className="check">
        <input type="checkbox" checked={form.accept_privacy} onChange={set("accept_privacy")} />
        <span>
          I agree that ForeMan stores my application and CV, may invite me by email to an AI-led video interview (recorded only with my consent),
          and deletes my data after 180 days if I'm not hired. I can ask for deletion sooner.
        </span>
      </label>
      {state.error && <p className="error">{state.error}</p>}
      <button className="btn btn-lime" disabled={state.sending || !form.accept_privacy}>{state.sending ? "Sending…" : "Send application"}</button>
    </form>
  );
}
