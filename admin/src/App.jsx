// Combox control room. On the multi-tenant platform: sign up, create a company,
// switch between companies, connect tools, pay in NEAR, and manage members.
// In a single-company deployment the platform pages are hidden and everything
// talks to that deployment's API.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { API, UNAUTHORIZED_EVENT, detectMode, platform } from "./api.js";
import { AuthShell, JUDGE_MODE, JudgeSignIn, Mfa, SignIn, signOut, useAal2Session, Wordmark } from "./Auth.jsx";
import { AppCtx, Button, ErrorNote, Icon, Modal, ToastProvider, useApp, useLoad, useToast } from "./ui.jsx";
import { Bell, useUnread } from "./Notifications.jsx";
import { markSignedIn } from "./signedIn.js";
import Onboarding from "./pages/Onboarding.jsx";
import Overview from "./pages/Overview.jsx";
import Team from "./pages/Team.jsx";
import Tasks from "./pages/Tasks.jsx";
import Meetings from "./pages/Meetings.jsx";
import Hiring from "./pages/Hiring.jsx";
import Approvals from "./pages/Approvals.jsx";
import Spending from "./pages/Spending.jsx";
import Activity from "./pages/Activity.jsx";
import AgentLog from "./pages/AgentLog.jsx";
import Tools from "./pages/Tools.jsx";
import KillSwitch from "./pages/KillSwitch.jsx";
import Integrations from "./pages/Integrations.jsx";
import Billing from "./pages/Billing.jsx";
import Members from "./pages/Members.jsx";
import Settings from "./pages/Settings.jsx";
import Operator from "./pages/Operator.jsx";

const PAGES = [
  { id: "overview", label: "Overview", icon: "overview", el: Overview, group: "Company" },
  { id: "team", label: "Org chart", icon: "org", el: Team, group: "Company" },
  { id: "tasks", label: "Tasks", icon: "tasks", el: Tasks, group: "Company" },
  { id: "meetings", label: "Interviews", icon: "meetings", el: Meetings, group: "Company" },
  { id: "hiring", label: "Hiring", icon: "hiring", el: Hiring, group: "Company" },
  { id: "approvals", label: "Payroll", icon: "payroll", el: Approvals, group: "Company" },
  { id: "spending", label: "Monthly cap", icon: "cap", el: Spending, group: "Company" },
  { id: "agent-log", label: "Agent log", icon: "bolt", el: AgentLog, group: "Company" },
  { id: "activity", label: "Feed", icon: "feed", el: Activity, group: "Company" },
  { id: "tools", label: "Tools", icon: "tools", el: Tools, group: "Company" },
  { id: "kill", label: "Kill switch", icon: "kill", el: KillSwitch, group: "Company" },
  { id: "integrations", label: "Integrations", icon: "integrations", el: Integrations, group: "Account" },
  { id: "billing", label: "Billing", icon: "billing", el: Billing, group: "Account", platformOnly: true },
  { id: "members", label: "Members", icon: "members", el: Members, group: "Account", platformOnly: true },
  { id: "settings", label: "Settings", icon: "settings", el: Settings, group: "Account" },
  { id: "platform", label: "All companies", icon: "platform", el: Operator, group: "Platform", operatorOnly: true },
];
const GROUPS = ["Company", "Account", "Platform"];

/** /c/<company>/<page> on the platform, /<page> otherwise. */
function parsePath(pathname) {
  const m = pathname.match(/^\/c\/([a-z0-9-]{3,40})(?:\/([a-z-]+))?/);
  if (m) return { slug: m[1], page: m[2] || "overview" };
  const p = pathname.replace(/^\//, "").split("/")[0];
  return { slug: null, page: p || "overview" };
}

export default function App() {
  return (
    <ToastProvider>
      <Gate />
    </ToastProvider>
  );
}

// While the saved session is read and the API answers: a plain screen, not
// the sign-in layout, so a signed-in reload never flashes the sign-in page.
// The mark fades in only if the check takes a moment.
function Checking() {
  return (
    <div className="grid min-h-screen place-items-center" aria-busy="true" aria-label="Loading">
      <div className="reveal" style={{ "--i": 10 }}><Wordmark /></div>
    </div>
  );
}

function Gate() {
  const { session, aal, refresh } = useAal2Session();
  const [mode, setMode] = useState(null);
  useEffect(() => {
    detectMode().then(setMode);
  }, []);
  // Tell the public website whether to send visitors straight here.
  useEffect(() => {
    if (session === null) markSignedIn(false);
    else if (session && aal?.currentLevel === "aal2") markSignedIn(true);
  }, [session, aal]);
  if (session === undefined || mode === null) return <Checking />;
  if (mode === "unreachable" || mode === "not-api") {
    const error = mode === "not-api"
      ? `${API} answered, but it isn't the Combox API (it's usually on port 8080). Set VITE_AXOM_API_URL to the API's address, e.g. http://localhost:8080, and rebuild the dashboard.`
      : `Can't reach the Combox API at ${API}. Make sure it's running and that VITE_AXOM_API_URL points to it, then reload.`;
    return (
      <AuthShell step="Can't connect" title="The Combox API isn't answering">
        <ErrorNote error={error} />
        <Button size="lg" className="mt-6 w-full" onClick={() => window.location.reload()}>Try again</Button>
      </AuthShell>
    );
  }
  if (!session) return JUDGE_MODE ? <JudgeSignIn onDone={refresh} /> : <SignIn onDone={refresh} />;
  if (!aal) return <Checking />;
  if (aal.currentLevel !== "aal2") return <Mfa onDone={refresh} />;
  return <Shell session={session} mode={mode} />;
}

function Shell({ session, mode }) {
  const isPlatform = mode === "multi-tenant";
  const toast = useToast();
  const [route, setRoute] = useState(() => parsePath(window.location.pathname));
  const [me, setMe] = useState(null);
  const [meError, setMeError] = useState(null);
  const [creating, setCreating] = useState(false);

  const navigate = useCallback((slug, page) => {
    const path = slug ? `/c/${slug}/${page}` : `/${page}`;
    if (window.location.pathname !== path) window.history.pushState(null, "", path);
    setRoute({ slug, page });
    window.scrollTo({ top: 0 });
  }, []);
  useEffect(() => {
    const onPop = () => setRoute(parsePath(window.location.pathname));
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  const reloadMe = useCallback(() => {
    if (!isPlatform) return Promise.resolve();
    return platform(session, "/me").then(setMe, (e) => setMeError(e.message));
  }, [session, isPlatform]);
  useEffect(() => {
    reloadMe();
  }, [reloadMe]);

  if (isPlatform && meError) {
    return (
      <AuthShell step="Something went wrong" title="Couldn't load your account">
        <ErrorNote error={meError} />
        <Button variant="ghost" size="lg" className="mt-6 w-full" onClick={signOut}>Sign out</Button>
      </AuthShell>
    );
  }
  if (isPlatform && !me) return <Checking />;

  const companies = me?.companies || [];
  // An operator without a company of their own still gets the platform page.
  const operatorOnly = isPlatform && companies.length === 0 && me?.operator && !creating;
  if (isPlatform && !operatorOnly && (companies.length === 0 || creating)) {
    return (
      <Onboarding
        session={session}
        canCancel={companies.length > 0}
        onCancel={() => setCreating(false)}
        onDone={async (slug) => {
          await reloadMe();
          setCreating(false);
          navigate(slug, "overview");
        }}
      />
    );
  }

  const current = isPlatform ? companies.find((c) => c.company.slug === route.slug) || companies[0] : null;
  const slug = current?.company.slug || null;
  const pages = operatorOnly ? PAGES.filter((p) => p.operatorOnly) : PAGES.filter((p) => (isPlatform || !p.platformOnly) && (!p.operatorOnly || me?.operator));
  const page = pages.find((p) => p.id === route.page) || pages[0];
  const ctx = {
    session,
    platform: isPlatform,
    me,
    company: current,
    slug,
    base: isPlatform ? `${API}/t/${slug}` : API,
    reloadMe,
    navigate: (p) => navigate(slug, p),
    notify: (text, severity = "success") => toast(text, severity === "error" || severity === "warning" ? severity : "ok"),
    newCompany: () => setCreating(true),
  };

  return (
    <AppCtx.Provider value={ctx}>
      <Frame pages={pages} page={page} current={current} companies={companies} onPickCompany={(s) => navigate(s, page.id)} email={me?.user?.email} />
    </AppCtx.Provider>
  );
}

// The signed-in layout. It sits inside the app context so it can keep the
// company's status fresh in the background: the kill switch, payouts waiting
// for approval, and notifications.
function Frame({ pages, page, current, companies, onPickCompany, email }) {
  const app = useApp();
  const { platform: isPlatform, slug, navigate } = app;
  // An operator without a company of their own has no company endpoints.
  const hasCompany = !isPlatform || !!slug;
  const kill = useLoad(hasCompany ? "/admin/kill-switch" : null, { poll: 15000 });
  const pending = useLoad(hasCompany ? "/payouts/pending" : null, { poll: 30000 });
  const feed = useLoad(hasCompany ? "/admin/notifications" : null, { poll: 30000 });
  const notes = useUnread(feed);
  const halted = !!kill.data?.on;
  const pendingCount = pending.data?.payouts?.length ?? 0;
  const badges = { approvals: pendingCount, activity: notes.unread };

  // Opening the Feed counts as having seen the notifications.
  useEffect(() => {
    if (page.id === "activity" && notes.unread) notes.markSeen();
  }, [page.id, notes.unread]);

  // The tab title follows the page and flags payouts waiting for approval.
  useEffect(() => {
    document.title = `${pendingCount ? `(${pendingCount}) ` : ""}${page.label} · Combox`;
  }, [page.label, pendingCount]);

  // An expired or revoked session gets one clear prompt, not a page of errors.
  const [signedOut, setSignedOut] = useState(false);
  useEffect(() => {
    const on = () => setSignedOut(true);
    window.addEventListener(UNAUTHORIZED_EVENT, on);
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, on);
  }, []);

  // Pages read the shared status from here, and call refreshStatus after
  // changing it, so the banner and badges update at once.
  const ctx = useMemo(
    () => ({
      ...app,
      status: { kill, pending, feed },
      refreshStatus: () => {
        kill.reload();
        pending.reload();
        feed.reload();
      },
    }),
    [app, kill, pending, feed],
  );
  const expired = current?.subscription?.state === "expired";
  const suspended = current?.company?.status === "suspended";
  const PageEl = page.el;
  const nav = { pages, page, badges, onGo: navigate };

  return (
    <AppCtx.Provider value={ctx}>
      <div className="min-h-screen lg:pl-64">
        {halted && (
          <button onClick={() => navigate("kill")} className="hazard hazard-move sticky top-0 z-30 block w-full py-1.5 text-center font-display text-xs font-semibold tracking-widest">
            <span className="rounded-full bg-ink px-3 py-1 text-ember">KILL SWITCH ON · ALL TOOL CALLS AND PAYOUTS HALTED</span>
          </button>
        )}
        <Sidebar {...nav} halted={halted} hasCompany={hasCompany} feed={feed} notes={notes} current={current} companies={companies} onPickCompany={onPickCompany} email={email} />
        <MobileNav {...nav} hasCompany={hasCompany} feed={feed} notes={notes} current={current} companies={companies} onPickCompany={onPickCompany} />
        <main className="mx-auto max-w-350 px-4 pb-24 pt-6 sm:px-8 lg:pt-10">
          {suspended && (
            <p className="mb-6 rounded-2xl bg-ember/10 px-4 py-3 text-sm text-ember ring-1 ring-ember/30">
              This company is suspended by the platform operator. Its automation and payouts are stopped; contact the operator.
            </p>
          )}
          {expired && !suspended && (
            <div className="mb-6 flex flex-wrap items-center gap-3 rounded-2xl bg-amber/10 px-4 py-3 text-sm text-amber ring-1 ring-amber/30">
              <span className="mr-auto">Your subscription has ended. Automation is paused until you renew; your data is safe.</span>
              <Button size="sm" onClick={() => navigate("billing")}>Renew</Button>
            </div>
          )}
          <div key={`${slug}-${page.id}`} className="page-in">
            <PageEl />
          </div>
        </main>
        <Modal open={signedOut} onClose={() => setSignedOut(false)} kicker="Signed out" title="Your session has ended" backdropClose={false}>
          <p className="text-sm text-mute">
            The API no longer accepts this session{JUDGE_MODE ? " (judge tokens last 12 hours; print a new one with ./judge/token.sh)" : ""}. Sign in again to keep working; nothing you were viewing has changed.
          </p>
          <div className="mt-6 grid grid-cols-2 gap-3">
            <Button variant="ghost" onClick={() => setSignedOut(false)}>Dismiss</Button>
            <Button onClick={signOut}>Sign in again</Button>
          </div>
        </Modal>
      </div>
    </AppCtx.Provider>
  );
}

function Badge({ id, badges }) {
  const n = badges[id];
  if (!n) return null;
  return (
    <span className={`rounded-full px-2 py-0.5 font-mono text-[10px] font-semibold text-ink ${id === "approvals" ? "bg-ember" : "bg-lime"}`}>
      {n > 99 ? "99+" : n}
    </span>
  );
}

function Sidebar({ pages, page, badges, onGo, halted, hasCompany, feed, notes, current, companies, onPickCompany, email }) {
  const { platform: isPlatform, newCompany } = useApp();
  return (
    <aside className="fixed inset-y-0 left-0 z-20 hidden w-64 flex-col border-r border-white/5 bg-ink/80 p-5 backdrop-blur-xl lg:flex">
      <div className="mb-6 flex items-center justify-between pl-2">
        <Wordmark />
        {hasCompany && <Bell feed={feed} {...notes} onOpenAll={() => onGo("activity")} align="side" />}
      </div>
      {isPlatform && current && <CompanySwitcher companies={companies} current={current} onPick={onPickCompany} onNew={newCompany} />}
      <nav className="-mr-2 flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto pr-2" aria-label="Main">
        {GROUPS.filter((g) => pages.some((p) => p.group === g)).map((g, gi) => (
          <div key={g} className="flex flex-col gap-1">
            {(gi > 0 || isPlatform) && <p className={`px-3 pb-1 font-mono text-[10px] uppercase tracking-[0.18em] text-dim ${gi > 0 ? "pt-4" : ""}`}>{g}</p>}
            {pages.filter((p) => p.group === g).map((p) => {
              const on = p.id === page.id;
              return (
                <button
                  key={p.id}
                  onClick={() => onGo(p.id)}
                  aria-current={on ? "page" : undefined}
                  className={`group relative flex items-center gap-3 rounded-2xl px-3 py-2.5 text-left text-sm transition ${on ? "bg-white/6 text-fog" : "text-mute hover:bg-white/3 hover:text-fog"}`}
                >
                  {on && <span className="absolute -left-5 top-1/2 h-6 w-1 -translate-y-1/2 rounded-r-full bg-lime shadow-[0_0_20px_4px_rgba(212,255,58,0.5)]" />}
                  <Icon name={p.icon} className={on ? "text-lime" : p.id === "kill" ? "text-ember/80" : ""} />
                  <span className="mr-auto">{p.label}</span>
                  <Badge id={p.id} badges={badges} />
                </button>
              );
            })}
          </div>
        ))}
      </nav>
      <div className="mt-4 space-y-1 border-t border-white/5 pt-4">
        {hasCompany && (
          <button
            onClick={() => onGo("kill")}
            className={`group flex w-full items-center gap-3 rounded-2xl px-3 py-2.5 text-left transition ${halted ? "bg-ember/10 hover:bg-ember/15" : "hover:bg-white/4"}`}
          >
            <span className={`pulse-dot shrink-0 rounded-full size-2 ${halted ? "bg-ember text-ember" : "bg-lime text-lime"}`} />
            <span className="min-w-0 flex-1">
              <span className={`block text-sm font-medium ${halted ? "text-ember" : "text-fog"}`}>{halted ? "Operations halted" : "Operations normal"}</span>
              <span className="block text-xs text-mute">{halted ? "Kill switch is on" : "Kill switch off"}</span>
            </span>
            <Icon name="arrow" size={14} className="shrink-0 text-dim transition group-hover:translate-x-0.5 group-hover:text-fog" />
          </button>
        )}
        {email && <p className="truncate px-3 pt-1 text-xs text-dim" title={email}>{email}</p>}
        <button onClick={signOut} className="flex w-full items-center gap-3 rounded-2xl px-3 py-2.5 text-sm text-mute transition hover:bg-white/3 hover:text-fog">
          <Icon name="out" /> Sign out
        </button>
      </div>
    </aside>
  );
}

function MobileNav({ pages, page, badges, onGo, hasCompany, feed, notes, current, companies, onPickCompany }) {
  const { platform: isPlatform, newCompany } = useApp();
  useEffect(() => {
    document.querySelector(`[data-nav="${page.id}"]`)?.scrollIntoView({ inline: "center", block: "nearest", behavior: "smooth" });
  }, [page.id]);
  return (
    <div className="sticky top-0 z-20 border-b border-white/5 bg-ink/85 backdrop-blur-xl lg:hidden">
      <div className="flex items-center gap-3 px-4 py-3">
        <Wordmark className="mr-auto" />
        {hasCompany && <Bell feed={feed} {...notes} onOpenAll={() => onGo("activity")} />}
        <button onClick={signOut} className="rounded-full p-2 text-mute" aria-label="Sign out"><Icon name="out" /></button>
      </div>
      {isPlatform && current && (
        <div className="px-3 pb-2">
          <CompanySwitcher companies={companies} current={current} onPick={onPickCompany} onNew={newCompany} compact />
        </div>
      )}
      <nav className="scrollbar-none flex gap-1 overflow-x-auto px-3 pb-3" aria-label="Main">
        {pages.map((p) => (
          <button
            key={p.id}
            onClick={() => onGo(p.id)}
            aria-current={p.id === page.id ? "page" : undefined}
            data-nav={p.id}
            className={`flex shrink-0 items-center gap-1.5 rounded-full px-3 py-1.5 text-xs ${p.id === page.id ? "bg-lime text-ink" : "bg-white/5 text-mute"}`}
          >
            {p.label}
            <Badge id={p.id} badges={badges} />
          </button>
        ))}
      </nav>
    </div>
  );
}

function CompanySwitcher({ companies, current, onPick, onNew, compact = false }) {
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
  return (
    <div ref={ref} className={`relative ${compact ? "" : "mb-6"}`}>
      <button
        onClick={() => setOpen(!open)}
        aria-haspopup="menu"
        aria-expanded={open}
        className="flex w-full items-center gap-3 rounded-2xl bg-white/4 px-3 py-2.5 text-left ring-1 ring-white/10 transition hover:bg-white/6"
      >
        <Icon name="building" className="shrink-0 text-violet" />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm font-medium">{current.company.name}</span>
          <span className="block text-xs capitalize text-mute">{current.role}</span>
        </span>
        <Icon name="switch" size={14} className="shrink-0 text-dim" />
      </button>
      {open && (
        <div role="menu" className="reveal absolute left-0 right-0 top-full z-40 mt-2 overflow-hidden rounded-2xl border border-white/10 bg-panel py-1 shadow-2xl">
          {companies.map((c) => {
            const on = c.company.slug === current.company.slug;
            return (
              <button
                key={c.company.slug}
                role="menuitem"
                onClick={() => {
                  setOpen(false);
                  onPick(c.company.slug);
                }}
                className={`flex w-full items-center gap-2 px-3 py-2 text-left text-sm transition hover:bg-white/5 ${on ? "text-lime" : "text-fog"}`}
              >
                <span className="truncate">{c.company.name}</span>
                {on && <Icon name="check" size={14} className="ml-auto shrink-0" />}
              </button>
            );
          })}
          <div className="my-1 border-t border-white/5" />
          <button
            role="menuitem"
            onClick={() => {
              setOpen(false);
              onNew();
            }}
            className="flex w-full items-center gap-2 px-3 py-2 text-left text-sm text-mute transition hover:bg-white/5 hover:text-fog"
          >
            <Icon name="plus" size={14} /> New company
          </button>
        </div>
      )}
    </div>
  );
}
