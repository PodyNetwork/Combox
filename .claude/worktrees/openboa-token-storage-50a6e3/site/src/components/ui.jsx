import { Fragment, useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";
import { reducedMotion, useCountUp, useInView, useScrollSpy } from "../lib/hooks.js";

export function Orb({ className = "", style }) {
  return (
    <div className={`orb ${className}`} style={style} aria-hidden="true">
      <div className="orb-halo" />
      <div className="orb-ring" />
      <div className="orb-core" />
    </div>
  );
}

export function SectionHead({ eyebrow, title, lead, center = false, children }) {
  return (
    <div className={`sec-head ${center ? "center" : ""}`}>
      {eyebrow && <span className="eyebrow" data-reveal>{eyebrow}</span>}
      <h2 className="h-l" data-reveal style={{ "--d": "0.05s" }}>{title}</h2>
      {lead && <p className="lead" data-reveal style={{ "--d": "0.1s" }}>{lead}</p>}
      {children}
    </div>
  );
}

export function CodeBlock({ code, label = "Copy command" }) {
  const [done, setDone] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code.replace(/^#.*\n?/gm, "").trim());
      setDone(true);
      setTimeout(() => setDone(false), 1600);
    } catch {
      /* clipboard blocked: the text is still selectable */
    }
  };
  return (
    <div className="code">
      <pre>
        <code>
          {code.split("\n").map((l, i) => (
            <span key={i} className={l.startsWith("#") ? "c" : undefined}>{l}{"\n"}</span>
          ))}
        </code>
      </pre>
      <button type="button" className={`copy ${done ? "done" : ""}`} onClick={copy} aria-label={done ? "Copied" : label}>
        {done ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
      </button>
      <span className="sr-only" aria-live="polite">{done ? "Copied to clipboard" : ""}</span>
    </div>
  );
}

export function Range({ label, value, min, max, step = 1, onChange, format = (v) => v, id }) {
  const pct = ((value - min) / (max - min)) * 100;
  return (
    <div className="ctl">
      <label htmlFor={id}>
        {label}
        <output htmlFor={id}>{format(value)}</output>
      </label>
      <input id={id} className="range" type="range" min={min} max={max} step={step} value={value} style={{ "--pct": `${pct}%` }} onChange={(e) => onChange(Number(e.target.value))} />
    </div>
  );
}

export function Toggle({ label, checked, onChange, hint }) {
  return (
    <div className="toggle-row" onClick={() => onChange(!checked)}>
      <span>
        {label}
        {hint && <span className="dim tiny" style={{ display: "block" }}>{hint}</span>}
      </span>
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={label}
        className="switch lime"
        onClick={(e) => {
          e.stopPropagation();
          onChange(!checked);
        }}
      />
    </div>
  );
}

export function BrandIcon({ icon, size = 26 }) {
  return (
    <svg viewBox="0 0 24 24" width={size} height={size} aria-hidden="true">
      <path d={icon.path} />
    </svg>
  );
}

export function Marquee({ items }) {
  const row = (dup) =>
    items.map((it) => (
      <span className="logo" key={`${dup}-${it.name}`} aria-hidden={dup ? "true" : undefined}>
        {it.icon ? <BrandIcon icon={it.icon} /> : it.glyph}
        {it.name}
      </span>
    ));
  return (
    <div className="marquee">
      <div className="marquee-track">
        {row(false)}
        <span className="dup-wrap">{row(true)}</span>
      </div>
    </div>
  );
}

export function Faq({ items }) {
  return (
    <div className="faq">
      {items.map((f, i) => (
        <details key={f.q} data-reveal style={{ "--d": `${Math.min(i, 6) * 0.04}s` }}>
          <summary>
            {f.q}
            <span className="pm" aria-hidden="true" />
          </summary>
          <div className="ans">{typeof f.a === "string" ? <p>{f.a}</p> : f.a}</div>
        </details>
      ))}
    </div>
  );
}

export function Stat({ value, prefix = "", suffix = "", label, color, decimals = 0 }) {
  const [ref, inView] = useInView({ threshold: 0.4 });
  const v = useCountUp(value, inView);
  return (
    <div className="stat" ref={ref}>
      <div className="n" style={{ color }}>
        {prefix}
        {v.toFixed(decimals)}
        {suffix}
      </div>
      <p>{label}</p>
    </div>
  );
}

/** A sentence whose words light up as it scrolls through the viewport. */
export function ScrollLit({ text, highlight = [] }) {
  const ref = useRef(null);
  const words = text.split(" ");
  const [lit, setLit] = useState(0);
  useEffect(() => {
    if (reducedMotion()) {
      setLit(words.length);
      return;
    }
    let raf = 0;
    const on = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        const r = ref.current.getBoundingClientRect();
        const start = innerHeight * 0.85;
        const end = innerHeight * 0.35;
        const p = Math.min(1, Math.max(0, (start - r.top) / (start - end + r.height * 0.6)));
        setLit(Math.round(p * words.length));
      });
    };
    on();
    addEventListener("scroll", on, { passive: true });
    addEventListener("resize", on);
    return () => {
      removeEventListener("scroll", on);
      removeEventListener("resize", on);
    };
  }, [words.length]);
  return (
    <p className="statement" ref={ref} aria-label={text}>
      {words.map((w, i) => (
        <span key={i} aria-hidden="true" className={`${i < lit ? "lit" : ""} ${highlight.includes(w.replace(/[^\w$]/g, "").toLowerCase()) ? "hl" : ""}`}>
          {w}{" "}
        </span>
      ))}
    </p>
  );
}

/** A slow constellation of nodes behind the hero; links bend toward the pointer. */
export function HeroCanvas() {
  const ref = useRef(null);
  useEffect(() => {
    const canvas = ref.current;
    const ctx = canvas.getContext("2d");
    const still = reducedMotion();
    let w = 0, h = 0, dpr = 1, raf = 0, visible = true;
    const pointer = { x: -9999, y: -9999 };
    let nodes = [];
    const colors = ["212,255,58", "139,108,255", "95,212,255"];

    const resize = () => {
      dpr = Math.min(2, devicePixelRatio || 1);
      w = canvas.clientWidth;
      h = canvas.clientHeight;
      canvas.width = w * dpr;
      canvas.height = h * dpr;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      const count = Math.round(Math.min(90, (w * h) / 16000));
      nodes = Array.from({ length: count }, () => ({
        x: Math.random() * w,
        y: Math.random() * h,
        vx: (Math.random() - 0.5) * 0.25,
        vy: (Math.random() - 0.5) * 0.25,
        r: Math.random() * 1.6 + 0.6,
        c: colors[Math.random() < 0.6 ? 1 : Math.random() < 0.5 ? 0 : 2],
      }));
    };

    const draw = () => {
      ctx.clearRect(0, 0, w, h);
      for (const n of nodes) {
        if (!still) {
          n.x += n.vx;
          n.y += n.vy;
          if (n.x < 0 || n.x > w) n.vx *= -1;
          if (n.y < 0 || n.y > h) n.vy *= -1;
          const dx = pointer.x - n.x, dy = pointer.y - n.y;
          const d = Math.hypot(dx, dy);
          if (d < 180) {
            n.x += dx * 0.004;
            n.y += dy * 0.004;
          }
        }
      }
      for (let i = 0; i < nodes.length; i++) {
        const a = nodes[i];
        for (let j = i + 1; j < nodes.length; j++) {
          const b = nodes[j];
          const d = Math.hypot(a.x - b.x, a.y - b.y);
          if (d < 130) {
            ctx.strokeStyle = `rgba(${a.c},${(1 - d / 130) * 0.22})`;
            ctx.lineWidth = 1;
            ctx.beginPath();
            ctx.moveTo(a.x, a.y);
            ctx.lineTo(b.x, b.y);
            ctx.stroke();
          }
        }
        const pd = Math.hypot(pointer.x - a.x, pointer.y - a.y);
        if (pd < 200) {
          ctx.strokeStyle = `rgba(212,255,58,${(1 - pd / 200) * 0.35})`;
          ctx.beginPath();
          ctx.moveTo(a.x, a.y);
          ctx.lineTo(pointer.x, pointer.y);
          ctx.stroke();
        }
        ctx.fillStyle = `rgba(${a.c},0.85)`;
        ctx.beginPath();
        ctx.arc(a.x, a.y, a.r, 0, Math.PI * 2);
        ctx.fill();
      }
      if (!still && visible) raf = requestAnimationFrame(draw);
    };

    const onMove = (e) => {
      const r = canvas.getBoundingClientRect();
      pointer.x = e.clientX - r.left;
      pointer.y = e.clientY - r.top;
      if (still) draw();
    };
    const onLeave = () => {
      pointer.x = pointer.y = -9999;
    };
    const io = new IntersectionObserver(([e]) => {
      visible = e.isIntersecting;
      cancelAnimationFrame(raf);
      if (visible) raf = requestAnimationFrame(draw);
    });

    resize();
    draw();
    io.observe(canvas);
    addEventListener("resize", resize);
    addEventListener("pointermove", onMove, { passive: true });
    document.addEventListener("pointerleave", onLeave);
    return () => {
      cancelAnimationFrame(raf);
      io.disconnect();
      removeEventListener("resize", resize);
      removeEventListener("pointermove", onMove);
      document.removeEventListener("pointerleave", onLeave);
    };
  }, []);
  return <canvas ref={ref} className="hero-canvas" aria-hidden="true" />;
}

/** Hero/page title revealed line by line. */
export function RiseTitle({ lines, className = "h-xl", as: Tag = "h1" }) {
  return (
    <Tag className={className}>
      {lines.map((l, i) => (
        <Fragment key={i}>
          <span className="line">
            <span style={{ "--i": i }}>{l}</span>
          </span>
          {/* A space between block lines: invisible, but keeps the words apart for screen readers and search. */}
          {i < lines.length - 1 ? " " : null}
        </Fragment>
      ))}
    </Tag>
  );
}

export function PageHero({ eyebrow, lines, lead, children }) {
  return (
    <section className="page-hero">
      <Orb className="page-hero-orb" />
      <div className="wrap">
        <span className="eyebrow fade-up" style={{ "--d": "0.05s" }}>{eyebrow}</span>
        <RiseTitle lines={lines} />
        {lead && <p className="lead fade-up" style={{ "--d": "0.25s" }}>{lead}</p>}
        {children && <div className="fade-up mt-l" style={{ "--d": "0.35s" }}>{children}</div>}
      </div>
    </section>
  );
}

/** Sticky in-page navigation that highlights the section in view. */
export function AnchorNav({ items }) {
  const on = useScrollSpy(items);
  return (
    <nav className="anchor-nav" aria-label="On this page">
      {items.map((i) => (
        <a key={i.id} href={`#${i.id}`} className={on === i.id ? "on" : ""} aria-current={on === i.id ? "true" : undefined}>
          {i.label}
        </a>
      ))}
    </nav>
  );
}
