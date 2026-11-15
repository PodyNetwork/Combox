// Sign-in with Supabase Auth, then TOTP two-factor authentication. The
// dashboard is usable only once the session reaches aal2. Judge mode (no
// Supabase) accepts a locally minted session token from ./judge/token.sh.
import { useEffect, useState } from "react";
import { createClient, isAuthRetryableFetchError } from "@supabase/supabase-js";
import { Breakthrough, Button, ErrorNote, Eyebrow, Input, KineticText, Segmented } from "./ui.jsx";

export const JUDGE_MODE = import.meta.env.VITE_JUDGE_MODE === "true";
// Both are baked in at build time; without them sign-in can't work.
export const SUPABASE_CONFIGURED = Boolean(import.meta.env.VITE_SUPABASE_URL && import.meta.env.VITE_SUPABASE_ANON_KEY);

export const supabase = createClient(
  import.meta.env.VITE_SUPABASE_URL || "http://localhost:54321",
  import.meta.env.VITE_SUPABASE_ANON_KEY || "public-anon-key",
);

function judgeSession() {
  try {
    const t = sessionStorage.getItem("axom-judge-token");
    return t ? { access_token: t } : null;
  } catch {
    return null;
  }
}

// ./scripts/start demo opens the dashboard with the judge session token in the URL
// fragment (#judge-token=..., never sent to a server). Keep it for this tab
// and drop it from the address bar and history.
function takeJudgeTokenFromLink() {
  const m = window.location.hash.match(/^#judge-token=([\w.-]+)$/);
  if (!m) return;
  try {
    sessionStorage.setItem("axom-judge-token", m[1]);
  } catch {
    /* storage unavailable: fall back to pasting */
  }
  window.history.replaceState(null, "", window.location.pathname + window.location.search);
}

function useJudgeSession() {
  const [session, setSession] = useState(() => {
    takeJudgeTokenFromLink();
    return judgeSession();
  });
  return { session, aal: session ? { currentLevel: "aal2" } : null, refresh: () => setSession(judgeSession()) };
}

// session is undefined while loading, null when signed out. offline is set
// when Supabase can't be reached, so the app can say so instead of showing
// the sign-in form (which would only fail with a network error).
function useSupabaseSession() {
  const [session, setSession] = useState(undefined);
  const [aal, setAal] = useState(null);
  const [offline, setOffline] = useState(false);
  const refresh = async () => {
    // supabase-js retries a failed token refresh for about 30 seconds; say
    // it's unreachable after 8 and carry on by itself if it answers later.
    const slow = setTimeout(() => setOffline(true), 8000);
    const { data, error } = await supabase.auth.getSession();
    clearTimeout(slow);
    if (isAuthRetryableFetchError(error)) {
      setOffline(true);
      return;
    }
    setOffline(false);
    setSession(data.session);
    if (data.session) {
      const level = await supabase.auth.mfa.getAuthenticatorAssuranceLevel();
      setAal(level.data);
    } else {
      setAal(null);
    }
  };
  useEffect(() => {
    refresh();
    // Defer: calling auth methods inside this callback can deadlock supabase-js.
    const { data } = supabase.auth.onAuthStateChange(() => setTimeout(refresh, 0));
    return () => data.subscription.unsubscribe();
  }, []);
  return { session, aal, refresh, offline };
}

export function useAal2Session() {
  // Judge mode is fixed at build time, so the hook order never changes.
  return JUDGE_MODE ? useJudgeSession() : useSupabaseSession();
}

export async function signOut() {
  if (JUDGE_MODE) {
    try {
      sessionStorage.removeItem("axom-judge-token");
    } catch {
      /* ignore */
    }
    window.location.reload();
    return;
  }
  // Sign out everywhere if Supabase answers. When it can't be reached, or the
  // saved token has expired and can't be refreshed, supabase-js keeps the
  // saved session and returns an error, which left "Sign in again" doing
  // nothing. So wait briefly, then drop the session from this browser anyway.
  await Promise.race([supabase.auth.signOut().catch(() => {}), new Promise((r) => setTimeout(r, 3000))]);
  try {
    for (const k of Object.keys(localStorage)) if (k.startsWith(supabase.auth.storageKey)) localStorage.removeItem(k);
  } catch {
    /* storage unavailable */
  }
  window.location.reload();
}

export function Wordmark({ className = "" }) {
  return (
    <a href="/" className={`inline-flex items-center gap-2.5 ${className}`} aria-label="Combox home">
      <svg className="size-7 shrink-0" viewBox="0 0 32 32" aria-hidden="true">
        <circle cx="16" cy="16" r="15" fill="#d4ff3a" />
        <path d="M8.5 18.5a7.5 7.5 0 0 1 15 0z" fill="#07070a" />
        <rect x="5.5" y="18.5" width="21" height="3" rx="1.5" fill="#07070a" />
        <rect x="14.6" y="10.2" width="2.8" height="6.4" rx="1.4" fill="#d4ff3a" />
      </svg>
      <span className="font-display text-xl font-semibold tracking-tight">Combox</span>
    </a>
  );
}

// Split screen: the orb punching through "COMBOX" on one side, the form on the other.
export function AuthShell({ step, title, children }) {
  return (
    <div className="relative grid min-h-screen lg:grid-cols-[1.15fr_1fr]">
      <div className="relative hidden overflow-hidden border-r border-white/5 p-12 lg:flex lg:flex-col">
        <Wordmark />
        <div className="relative my-auto">
          <Breakthrough className="-ml-2" />
          <p className="relative mt-10 font-mono text-xs uppercase tracking-[0.18em] text-lime">Run your company in a box</p>
          <h1 className="relative mt-4 font-display text-4xl font-semibold leading-none tracking-tight xl:text-5xl">
            <KineticText text="Agents do the work." />
            <br />
            <span className="text-lime"><KineticText text="You hold the money." delay={19} /></span>
          </h1>
          <p className="relative mt-6 max-w-md text-mute">
            Every payout, hire, firing and contract passes a deterministic policy engine. Big payouts wait for you, and a monthly cap and a kill switch keep you in control.
          </p>
        </div>
        <div className="grid grid-cols-3 gap-6 border-t border-white/5 pt-6 font-mono text-xs text-mute">
          <span><b className="block font-display text-2xl text-fog">2FA</b>on every account</span>
          <span><b className="block font-display text-2xl text-fog">Cap</b>on monthly payouts</span>
          <span><b className="block font-display text-2xl text-fog">1</b>kill switch</span>
        </div>
      </div>
      <div className="flex flex-col p-6 sm:p-12">
        <div className="lg:hidden"><Wordmark /></div>
        <div className="reveal mx-auto my-auto w-full max-w-sm py-12">
          {step && <Eyebrow className="text-lime opacity-100">{step}</Eyebrow>}
          <h2 className="mt-2 mb-8 font-display text-3xl font-semibold tracking-tight">{title}</h2>
          {children}
        </div>
      </div>
    </div>
  );
}

/** A green confirmation line, the counterpart of ErrorNote. */
function OkNote({ text }) {
  return text ? <p className="rounded-xl bg-lime/10 px-3 py-2 text-sm text-lime ring-1 ring-lime/30">{text}</p> : null;
}

export function JudgeSignIn({ onDone }) {
  const [token, setToken] = useState("");
  const submit = (e) => {
    e.preventDefault();
    try {
      sessionStorage.setItem("axom-judge-token", token.trim());
    } catch {
      /* storage unavailable */
    }
    onDone();
  };
  return (
    <AuthShell step="Judge mode" title="Paste your session">
      <form onSubmit={submit} className="space-y-5">
        <p className="text-sm text-mute">
          <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-fog">./scripts/start demo</code> signs you in automatically. Otherwise paste the token printed by <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-fog">./judge/token.sh</code>. It stands in for a Supabase session with two-factor verified; the API checks it on every request.
        </p>
        <Input label="Session token" value={token} onChange={setToken} autoFocus />
        <Button type="submit" size="lg" className="w-full" disabled={!token.trim()}>Enter the control room</Button>
      </form>
    </AuthShell>
  );
}

// Browsers word a failed request differently: "Failed to fetch" (Chrome),
// "Load failed" (Safari), "NetworkError when attempting..." (Firefox).
const friendly = (e) =>
  /fetch|network|load failed/i.test(e.message)
    ? `Can't reach Supabase at ${import.meta.env.VITE_SUPABASE_URL}. Check your internet connection and DNS; if those are fine, check VITE_SUPABASE_URL in .env and rebuild the dashboard.`
    : e.message;

export function SignIn({ onDone }) {
  const [mode, setMode] = useState("in");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState(null);
  const [ok, setOk] = useState(null);
  const [busy, setBusy] = useState(false);

  if (!SUPABASE_CONFIGURED) {
    return (
      <AuthShell step="Setup needed" title="Sign-in isn't set up yet">
        <div className="space-y-4 text-sm text-mute">
          <p>This dashboard was built without your Supabase settings, so there is nothing to sign in against.</p>
          <ol className="list-decimal space-y-2 pl-5">
            <li>Set <code className="font-mono text-fog">VITE_SUPABASE_URL</code> and <code className="font-mono text-fog">VITE_SUPABASE_ANON_KEY</code> in <code className="font-mono text-fog">.env</code>.</li>
            <li>Rebuild the dashboard: <code className="font-mono text-fog">docker compose up --build admin</code>, or restart <code className="font-mono text-fog">./scripts/dev.sh</code>.</li>
            <li>Reload this page.</li>
          </ol>
          <p>Full steps: <code className="font-mono text-fog">docs/SETUP.md</code>.</p>
        </div>
      </AuthShell>
    );
  }

  const run = async (fn) => {
    setBusy(true);
    setError(null);
    setOk(null);
    try {
      await fn();
    } catch (e) {
      setError(friendly(e));
    }
    setBusy(false);
  };
  const signIn = () =>
    run(async () => {
      const { error } = await supabase.auth.signInWithPassword({ email, password });
      if (error) throw error;
      onDone();
    });
  const signUp = () =>
    run(async () => {
      const { data, error } = await supabase.auth.signUp({ email, password, options: { emailRedirectTo: window.location.origin } });
      if (error) throw error;
      if (data.session) onDone();
      else setOk("Check your email to confirm your address, then sign in.");
    });
  const reset = () =>
    run(async () => {
      if (!email) throw new Error("Enter your email first.");
      const { error } = await supabase.auth.resetPasswordForEmail(email, { redirectTo: window.location.origin });
      if (error) throw error;
      setOk("If that address has an account, a reset link is on its way.");
    });
  const creating = mode === "up";

  return (
    <AuthShell step="Step 1 of 2" title={creating ? "Create your account" : "Sign in"}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          creating ? signUp() : signIn();
        }}
        className="space-y-4"
      >
        <Segmented
          ariaLabel="Sign in or create an account"
          value={mode}
          onChange={(v) => {
            setMode(v);
            setError(null);
            setOk(null);
          }}
          options={[{ value: "in", label: "Sign in" }, { value: "up", label: "Create account" }]}
        />
        <p className="text-sm text-mute">{creating ? "Start your free trial. You'll set up two-factor authentication next." : "Welcome back. Sign in to manage your company."}</p>
        <Input label="Email" type="email" value={email} onChange={setEmail} autoComplete="email" autoFocus required />
        <Input
          label="Password"
          type="password"
          value={password}
          onChange={setPassword}
          autoComplete={creating ? "new-password" : "current-password"}
          hint={creating ? "At least 8 characters." : undefined}
          required
        />
        <ErrorNote error={error} />
        <OkNote text={ok} />
        <Button type="submit" size="lg" className="w-full" disabled={busy || !email || password.length < (creating ? 8 : 1)}>
          {busy ? "One moment…" : creating ? "Create account" : "Continue"}
        </Button>
        {!creating && (
          <button type="button" onClick={reset} className="block w-full text-center text-sm text-mute transition hover:text-fog">
            Forgot password?
          </button>
        )}
      </form>
    </AuthShell>
  );
}

export function Mfa({ onDone }) {
  const [factor, setFactor] = useState(null);
  const [enroll, setEnroll] = useState(null);
  const [code, setCode] = useState("");
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    supabase.auth.mfa.listFactors().then(({ data, error }) => {
      if (error) return setError(error.message);
      const verified = data?.totp?.find((f) => f.status === "verified");
      if (verified) setFactor(verified);
      else {
        // Drop half-finished enrollments from earlier visits so they don't pile up.
        const stale = (data?.all || data?.totp || []).filter((f) => f.factor_type === "totp" && f.status !== "verified");
        Promise.all(stale.map((f) => supabase.auth.mfa.unenroll({ factorId: f.id })))
          .catch(() => {})
          .then(() => supabase.auth.mfa.enroll({ factorType: "totp" }))
          .then(({ data, error }) => (error ? setError(error.message) : setEnroll(data)));
      }
    });
  }, []);
  const verify = async (e) => {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    const { error } = await supabase.auth.mfa.challengeAndVerify({ factorId: factor?.id || enroll?.id, code });
    setBusy(false);
    if (error) setError(error.message);
    else onDone();
  };
  return (
    <AuthShell step="Step 2 of 2" title="Two-factor check">
      {enroll && (
        <div className="mb-6 space-y-3 rounded-2xl bg-white/5 p-4 ring-1 ring-white/10">
          <div className="flex items-center gap-4">
            <img alt="QR code for your authenticator app" src={enroll.totp.qr_code} className="w-32 rounded-xl bg-white p-2" />
            <p className="text-sm text-mute">Combox controls payouts, so every account uses an authenticator app. Scan this with Google Authenticator, 1Password or Authy, then enter the 6-digit code.</p>
          </div>
          <p className="text-xs text-dim">Can't scan? Enter this key: <span className="font-mono text-mute">{enroll.totp.secret}</span></p>
        </div>
      )}
      {factor && <p className="mb-6 text-sm text-mute">Enter the 6-digit code from your authenticator app.</p>}
      <form onSubmit={verify} className="space-y-4">
        <Input
          label="Authenticator code"
          value={code}
          onChange={(v) => setCode(v.replace(/\D/g, "").slice(0, 6))}
          inputMode="numeric"
          autoComplete="one-time-code"
          placeholder="••• •••"
          className="[&_input]:text-center [&_input]:font-mono [&_input]:text-2xl [&_input]:tracking-[0.5em]"
          autoFocus
        />
        <ErrorNote error={error} />
        <Button type="submit" size="lg" className="w-full" disabled={code.length !== 6 || busy || !(factor || enroll)}>{busy ? "Verifying…" : "Verify"}</Button>
        <button type="button" onClick={signOut} className="block w-full text-center text-sm text-mute transition hover:text-fog">Use a different account</button>
      </form>
    </AuthShell>
  );
}
