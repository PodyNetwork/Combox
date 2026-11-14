import { useEffect, useState } from "react";
import { ArrowRight, ArrowUpRight } from "lucide-react";
import { siGithub } from "simple-icons";
import { DASHBOARD_HOME, DASHBOARD_URL, JOBS_URL, JUDGE_MODE, NAV, REPO_URL, SIGNED_IN, dashboardHref } from "./lib/config.js";
import { reducedMotion } from "./lib/hooks.js";

export default function Layout({ page, children }) {
  useGlobalEffects();
  return (
    <>
      <a className="skip" href="#main">Skip to content</a>
      <div className="atmos" aria-hidden="true"><i /><i /><i /></div>
      <ScrollProgress />
      <Nav page={page} />
      <main id="main">{children}</main>
      <Footer />
    </>
  );
}

/** Reveal-on-scroll, pointer spotlight on cards, and magnetic buttons. */
function useGlobalEffects() {
  useEffect(() => {
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) {
            e.target.classList.add("is-in");
            io.unobserve(e.target);
          }
        }
      },
      { threshold: 0.12, rootMargin: "0px 0px -6% 0px" },
    );
    const watch = (root) => root.querySelectorAll?.("[data-reveal]:not(.is-in)").forEach((el) => io.observe(el));
    watch(document);
    // Demos add content later (tabs, steps): watch only what was added.
    const mo = new MutationObserver((records) => records.forEach((r) => r.addedNodes.forEach(watch)));
    mo.observe(document.body, { childList: true, subtree: true });

    // Judge mode: carry the local sign-in token into the dashboard (see takeJudgeToken).
    const onClick = (e) => {
      const a = JUDGE_MODE && e.target.closest?.("a[href]");
      if (a && a.href.startsWith(DASHBOARD_URL)) a.href = dashboardHref();
    };
    document.addEventListener("click", onClick);

    const onMove = (e) => {
      const el = e.target.closest?.(".spot");
      if (el) {
        const r = el.getBoundingClientRect();
        el.style.setProperty("--mx", `${e.clientX - r.left}px`);
        el.style.setProperty("--my", `${e.clientY - r.top}px`);
      }
      if (reducedMotion() || e.pointerType !== "mouse") return;
      const m = e.target.closest?.(".magnetic");
      document.querySelectorAll(".magnetic.pulled").forEach((b) => {
        if (b !== m) {
          b.style.transform = "";
          b.classList.remove("pulled");
        }
      });
      if (m) {
        const r = m.getBoundingClientRect();
        const x = (e.clientX - (r.left + r.width / 2)) * 0.18;
        const y = (e.clientY - (r.top + r.height / 2)) * 0.28;
        m.style.transform = `translate(${x}px, ${y}px)`;
        m.classList.add("pulled");
      }
    };
    document.addEventListener("pointermove", onMove, { passive: true });
    return () => {
      io.disconnect();
      mo.disconnect();
      document.removeEventListener("pointermove", onMove);
      document.removeEventListener("click", onClick);
    };
  }, []);
}

/** Read after hydration: the prerendered HTML is the signed-out page. */
function useSignedIn() {
  const [signedIn, setSignedIn] = useState(false);
  useEffect(() => setSignedIn(SIGNED_IN), []);
  return signedIn;
}

function ScrollProgress() {
  useEffect(() => {
    const el = document.querySelector(".progress");
    let raf = 0;
    const on = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        const h = document.documentElement.scrollHeight - innerHeight;
        el.style.setProperty("--p", h > 0 ? Math.min(1, scrollY / h) : 0);
      });
    };
    on();
    addEventListener("scroll", on, { passive: true });
    addEventListener("resize", on);
    return () => {
      removeEventListener("scroll", on);
      removeEventListener("resize", on);
    };
  }, []);
  return <div className="progress" aria-hidden="true" />;
}

function Nav({ page }) {
  const [scrolled, setScrolled] = useState(false);
  const [open, setOpen] = useState(false);
  const signedIn = useSignedIn();

  useEffect(() => {
    const on = () => setScrolled(scrollY > 12);
    on();
    addEventListener("scroll", on, { passive: true });
    return () => removeEventListener("scroll", on);
  }, []);

  useEffect(() => {
    document.documentElement.style.overflow = open ? "hidden" : "";
    if (!open) return;
    const onKey = (e) => e.key === "Escape" && setOpen(false);
    const onResize = () => innerWidth > 1080 && setOpen(false);
    addEventListener("keydown", onKey);
    addEventListener("resize", onResize);
    return () => {
      removeEventListener("keydown", onKey);
      removeEventListener("resize", onResize);
    };
  }, [open]);

  return (
    <>
      <header className={`nav ${scrolled || open ? "scrolled" : ""}`}>
        <div className="wrap">
          <nav className="nav-inner" aria-label="Main">
            <a href="/" className="wordmark" aria-label="Combox home"><Mark />Combox</a>
            <ul className="nav-links">
              {NAV.map((n) => (
                <li key={n.href}><a href={n.href} aria-current={page === n.page ? "page" : undefined}>{n.label}</a></li>
              ))}
            </ul>
            <div className="nav-cta">
              {signedIn ? (
                <a className="btn btn-lime btn-sm magnetic" href={DASHBOARD_HOME}>Open dashboard<ArrowRight className="arrow" aria-hidden="true" /></a>
              ) : (
                <>
                  <a className="btn btn-ghost btn-sm" href={DASHBOARD_HOME}>Sign in</a>
                  <a className="btn btn-lime btn-sm magnetic" href="/get-started/">Start free<ArrowRight className="arrow" aria-hidden="true" /></a>
                </>
              )}
              <button className="nav-burger" aria-expanded={open} aria-controls="drawer" aria-label={open ? "Close menu" : "Open menu"} onClick={() => setOpen((o) => !o)}>
                <span />
              </button>
            </div>
          </nav>
        </div>
      </header>
      <div id="drawer" className={`drawer ${open ? "open" : ""}`} aria-hidden={!open} inert={open ? undefined : true}>
        <ul>
          {[{ href: "/", label: "Home", page: "home" }, ...NAV].map((n, i) => (
            <li key={n.href} style={{ "--i": i }}>
              <a className="big" href={n.href} aria-current={page === n.page ? "page" : undefined} onClick={() => setOpen(false)}>
                {n.label}<ArrowUpRight aria-hidden="true" />
              </a>
            </li>
          ))}
        </ul>
        <div className="drawer-cta">
          {signedIn ? (
            <a className="btn btn-lime btn-lg" href={DASHBOARD_HOME}>Open dashboard</a>
          ) : (
            <>
              <a className="btn btn-lime btn-lg" href="/get-started/">Start free</a>
              <a className="btn btn-ghost btn-lg" href={DASHBOARD_HOME}>Sign in to the dashboard</a>
            </>
          )}
        </div>
      </div>
    </>
  );
}

// The Combox mark: a hard hat on a lime disc. The hat tips up on hover.
export function Mark() {
  return (
    <svg className="mark" viewBox="0 0 32 32" aria-hidden="true">
      <circle cx="16" cy="16" r="15" fill="#d4ff3a" />
      <g className="hat">
        <path d="M8.5 18.5a7.5 7.5 0 0 1 15 0z" fill="#07070a" />
        <rect x="5.5" y="18.5" width="21" height="3" rx="1.5" fill="#07070a" />
        <rect x="14.6" y="10.2" width="2.8" height="6.4" rx="1.4" fill="#d4ff3a" />
      </g>
    </svg>
  );
}

function Footer() {
  return (
    <footer className="footer">
      <div className="wrap">
        <div className="footer-grid">
          <div>
            <a href="/" className="wordmark"><Mark />Combox</a>
            <p className="muted small mt-m" style={{ maxWidth: "22rem" }}>
              The operating system for fully autonomous companies. AI runs every department; code enforces the limits.
            </p>
            <div className="row mt-m">
              <a className="btn btn-ghost btn-sm" href={REPO_URL} target="_blank" rel="noreferrer">
                <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true"><path d={siGithub.path} /></svg>
                Source on GitHub
              </a>
            </div>
          </div>
          <div>
            <h5>Product</h5>
            <ul>
              <li><a href="/how-it-works/">How it works</a></li>
              <li><a href="/security/">Security and guardrails</a></li>
              <li><a href="/pricing/">Pricing</a></li>
              <li><a href="/get-started/">Get started</a></li>
            </ul>
          </div>
          <div>
            <h5>People</h5>
            <ul>
              <li><a href="/talent/">For freelancers</a></li>
              <li><a href={JOBS_URL}>Open roles (Combox Jobs)</a></li>
              <li><a href="/talent/#interview">The AI interview</a></li>
              <li><a href="/talent/#pay">Getting paid in NEAR</a></li>
            </ul>
          </div>
          <div>
            <h5>Company</h5>
            <ul>
              <li><a href={DASHBOARD_HOME}>Dashboard sign-in</a></li>
              <li><a href="/privacy/">Privacy and data</a></li>
              <li><a href={`${REPO_URL}/blob/HEAD/docs/SETUP.md`} target="_blank" rel="noreferrer">Setup guide</a></li>
              <li><a href={`${REPO_URL}/blob/HEAD/DECISIONS.md`} target="_blank" rel="noreferrer">Design decisions</a></li>
            </ul>
          </div>
        </div>
        <div className="footer-giant" aria-hidden="true">combox</div>
        <div className="footer-bottom">
          <span suppressHydrationWarning>© {new Date().getFullYear()} Combox. Run your company in a box.</span>
          <span className="mono">policy {"2026-11-08.1"}</span>
        </div>
      </div>
    </footer>
  );
}
