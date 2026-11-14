// Org chart as a live network: the policy engine at the core, departments,
// teams and workers in rings, with work flowing out along the edges.
// Drag to pan, pinch or ⌘-scroll to zoom, click a department to fly into it,
// search to find a worker. All camera moves ease toward a target.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Icon, MOTION, human, usd } from "../ui.jsx";

const R = { dept: 150, team: 265, worker: 385 };
const VB = { x: -560, y: -500, w: 1120, h: 1000 };
const K_MIN = 0.7;
const K_MAX = 3;
const EDGE = { active: "#d4ff3a", on_probation: "#ffb547", candidate: "#5fd4ff", suspended: "#ff5a1f", fired: "#ff5a1f" };
const STATUSES = [
  ["active", "Active"],
  ["on_probation", "Probation"],
  ["candidate", "Candidate"],
  ["fired", "Fired"],
];
const group = (s) => (s === "suspended" ? "fired" : s);
const clamp = (k) => Math.min(K_MAX, Math.max(K_MIN, k));
const polar = (r, a) => [Math.cos(a) * r, Math.sin(a) * r];
// Node labels stay inside the canvas; the hover card shows the full name.
const short = (name) => (name.length > 20 ? `${name.slice(0, 19).trimEnd()}…` : name);

// Curve from parent to child that bends radially (like routed cables).
function cable(r1, a1, r2, a2) {
  const [x1, y1] = polar(r1, a1);
  const [x2, y2] = polar(r2, a2);
  const mid = (r1 + r2) / 2;
  const [c1x, c1y] = polar(mid, a1);
  const [c2x, c2y] = polar(mid, a2);
  return `M${x1.toFixed(1)},${y1.toFixed(1)} C${c1x.toFixed(1)},${c1y.toFixed(1)} ${c2x.toFixed(1)},${c2y.toFixed(1)} ${x2.toFixed(1)},${y2.toFixed(1)}`;
}

function layout(departments) {
  const weight = (d) => Math.max(1, d.teams.reduce((n, t) => n + Math.max(1, t.workers.length), 0));
  const total = departments.reduce((n, d) => n + weight(d), 0) || 1;
  const nodes = [];
  const edges = [];
  let start = -Math.PI / 2;
  let wi = 0;
  const gap = 0.06;
  departments.forEach((d) => {
    const span = (weight(d) / total) * Math.PI * 2;
    const da = start + span / 2;
    const count = d.teams.reduce((n, t) => n + t.workers.length, 0);
    nodes.push({ kind: "dept", id: d.id, name: d.name, a: da, r: R.dept, count });
    edges.push({ id: `c-${d.id}`, d: cable(46, da, R.dept - 30, da), tone: "core", dept: d.id, delay: 0.15 });
    let ts = start + gap / 2;
    const inner = span - gap;
    const tw = weight(d);
    d.teams.forEach((t) => {
      const tspan = (Math.max(1, t.workers.length) / tw) * inner;
      const ta = ts + tspan / 2;
      nodes.push({ kind: "team", id: t.id, name: t.name, a: ta, r: R.team, dept: d.id });
      edges.push({ id: `d-${t.id}`, d: cable(R.dept + 30, da, R.team - 10, ta), tone: "dept", dept: d.id, team: t.id, delay: 0.5 });
      t.workers.forEach((w, i) => {
        const wa = t.workers.length === 1 ? ta : ts + (tspan * (i + 0.5)) / t.workers.length;
        nodes.push({ kind: "worker", id: w.id, a: wa, r: R.worker, w, team: t.name, dept: d.id, order: wi });
        edges.push({ id: `t-${w.id}`, d: cable(R.team + 10, ta, R.worker - 24, wa), tone: w.status, worker: w.id, dept: d.id, team: t.id, delay: 0.85 + wi * 0.05 });
        wi++;
      });
      ts += tspan;
    });
    start += span;
  });
  return { nodes, edges };
}

export default function OrgNetwork({ departments, onSelect, selected }) {
  const { nodes, edges } = useMemo(() => layout(departments), [departments]);
  const svgRef = useRef(null);
  const sceneRef = useRef(null);
  const view = useRef({ x: 0, y: 0, k: 1 });
  const target = useRef({ x: 0, y: 0, k: 1 });
  const raf = useRef(0);
  const [, rerender] = useState(0);
  const [zoomed, setZoomed] = useState(false);
  const [hover, setHover] = useState(null);
  const [deptFocus, setDeptFocus] = useState(null);
  const [query, setQuery] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  const [statuses, setStatuses] = useState(() => new Set(STATUSES.map(([k]) => k)));
  const [kind, setKind] = useState("all");

  // ------------------------------------------------------------ camera
  const apply = () => {
    const v = view.current;
    sceneRef.current?.setAttribute("transform", `translate(${v.x.toFixed(2)} ${v.y.toFixed(2)}) scale(${v.k.toFixed(4)})`);
    setZoomed(v.k > 1.04);
  };
  const animate = useCallback(() => {
    cancelAnimationFrame(raf.current);
    const step = () => {
      const v = view.current;
      const t = target.current;
      const e = MOTION ? 0.14 : 1;
      v.x += (t.x - v.x) * e;
      v.y += (t.y - v.y) * e;
      v.k += (t.k - v.k) * e;
      const done = Math.abs(t.x - v.x) < 0.2 && Math.abs(t.y - v.y) < 0.2 && Math.abs(t.k - v.k) < 0.001;
      if (done) Object.assign(v, t);
      apply();
      if (done) rerender((n) => n + 1);
      else raf.current = requestAnimationFrame(step);
    };
    raf.current = requestAnimationFrame(step);
  }, []);
  useEffect(() => () => cancelAnimationFrame(raf.current), []);

  const jump = (t) => {
    target.current = t;
    Object.assign(view.current, t);
    cancelAnimationFrame(raf.current);
    apply();
  };
  const toSvg = (cx, cy) => {
    const svg = svgRef.current;
    const p = svg.createSVGPoint();
    p.x = cx;
    p.y = cy;
    return p.matrixTransform(svg.getScreenCTM().inverse());
  };
  const zoomAt = (px, py, factor, instant = false) => {
    const t = target.current;
    const k = clamp(t.k * factor);
    const r = k / t.k;
    const next = { k, x: px - (px - t.x) * r, y: py - (py - t.y) * r };
    if (instant) jump(next);
    else {
      target.current = next;
      animate();
    }
  };
  const focusOn = (px, py, k) => {
    const kk = clamp(k);
    target.current = { k: kk, x: -px * kk, y: -py * kk };
    animate();
  };
  const fit = () => {
    setDeptFocus(null);
    target.current = { x: 0, y: 0, k: 1 };
    animate();
  };

  // Pinch on a trackpad or ⌘/Ctrl-scroll zooms; plain scroll pans once zoomed in,
  // otherwise the page scrolls as usual.
  useEffect(() => {
    const svg = svgRef.current;
    const onWheel = (e) => {
      if (e.ctrlKey || e.metaKey) {
        e.preventDefault();
        const p = toSvg(e.clientX, e.clientY);
        zoomAt(p.x, p.y, Math.exp(-e.deltaY * 0.01), true);
      } else if (view.current.k > 1.04) {
        e.preventDefault();
        const s = VB.w / svg.clientWidth;
        const t = target.current;
        jump({ k: t.k, x: t.x - e.deltaX * s, y: t.y - e.deltaY * s });
      }
    };
    svg.addEventListener("wheel", onWheel, { passive: false });
    return () => svg.removeEventListener("wheel", onWheel);
  }, []);

  // Drag to pan; two fingers to pinch.
  const pointers = useRef(new Map());
  const drag = useRef(null);
  const pinch = useRef(null);
  const moved = useRef(false);
  useEffect(() => {
    const move = (e) => {
      if (!pointers.current.has(e.pointerId)) return;
      pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
      const pts = [...pointers.current.values()];
      if (pinch.current && pts.length >= 2) {
        const d = Math.hypot(pts[0].x - pts[1].x, pts[0].y - pts[1].y);
        const { d0, k0, mid } = pinch.current;
        const k = clamp((k0 * d) / d0);
        const t = target.current;
        const r = k / t.k;
        jump({ k, x: mid.x - (mid.x - t.x) * r, y: mid.y - (mid.y - t.y) * r });
        moved.current = true;
        return;
      }
      const g = drag.current;
      if (!g) return;
      const dx = e.clientX - g.sx;
      const dy = e.clientY - g.sy;
      if (!moved.current && Math.hypot(dx, dy) < 4) return;
      if (!moved.current) setHover(null);
      moved.current = true;
      const s = VB.w / svgRef.current.clientWidth;
      jump({ k: target.current.k, x: g.tx + dx * s, y: g.ty + dy * s });
    };
    const up = (e) => {
      pointers.current.delete(e.pointerId);
      if (pointers.current.size < 2) pinch.current = null;
      if (pointers.current.size === 0) {
        drag.current = null;
        svgRef.current?.classList.remove("cursor-grabbing");
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
    };
  }, []);
  const down = (e) => {
    if (e.pointerType === "mouse" && e.button !== 0) return;
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
    moved.current = false;
    if (pointers.current.size === 1) {
      drag.current = { sx: e.clientX, sy: e.clientY, tx: target.current.x, ty: target.current.y };
      svgRef.current.classList.add("cursor-grabbing");
    } else if (pointers.current.size === 2) {
      const [a, b] = [...pointers.current.values()];
      pinch.current = { d0: Math.hypot(a.x - b.x, a.y - b.y) || 1, k0: target.current.k, mid: toSvg((a.x + b.x) / 2, (a.y + b.y) / 2) };
      drag.current = null;
    }
  };
  // A drag that ends over a node must not count as a click.
  const click = (fn) => (e) => {
    if (moved.current) return;
    e.stopPropagation();
    fn();
  };

  // ------------------------------------------------------------ filters
  const q = query.trim().toLowerCase();
  const workers = nodes.filter((n) => n.kind === "worker");
  const matches = (w) => `${w.name} ${w.role} ${(w.skills || []).join(" ")}`.toLowerCase().includes(q);
  const visible = (n) =>
    statuses.has(group(n.w.status)) &&
    (kind === "all" || (kind === "ai") === (n.w.kind === "ai_agent")) &&
    (!deptFocus || deptFocus === n.dept) &&
    (!q || matches(n.w));
  const visibleIds = new Set(workers.filter(visible).map((n) => n.id));
  const results = q ? workers.filter((n) => matches(n.w)) : [];

  const focus = hover || selected;
  const chain = focus ? edges.find((e) => e.worker === focus) : null;
  const onRoute = (e) => !chain || e.worker === focus || (!e.worker && e.team === chain.team) || (e.tone === "core" && e.dept === chain.dept);
  const edgeVisible = (e) => (e.worker ? visibleIds.has(e.worker) : !deptFocus || e.dept === deptFocus);

  const flyTo = (n, k = 1.8) => {
    const [x, y] = polar(n.r, n.a);
    focusOn(x * 0.85, y * 0.85, k);
  };
  // Picking a worker while another department is in focus moves the focus
  // (and the camera) to that worker's department instead of leaving it dimmed.
  const selectWorker = (n) => {
    if (deptFocus && deptFocus !== n.dept) {
      const d = nodes.find((x) => x.kind === "dept" && x.id === n.dept);
      setDeptFocus(n.dept);
      if (d) {
        const [x, y] = polar((R.dept + R.worker) / 2, d.a);
        focusOn(x, y, 1.65);
      }
    }
    onSelect(n.w);
  };
  const openDept = (n) => {
    if (deptFocus === n.id) return fit();
    setDeptFocus(n.id);
    const [x, y] = polar((R.dept + R.worker) / 2, n.a);
    focusOn(x, y, 1.65);
  };
  const pick = (n) => {
    if (!n) return;
    setDeptFocus(null);
    onSelect(n.w);
    flyTo(n);
    setSearchOpen(false);
  };
  const toggleStatus = (k) =>
    setStatuses((s) => {
      const next = new Set(s);
      next.has(k) ? next.delete(k) : next.add(k);
      return next.size ? next : new Set(STATUSES.map(([x]) => x));
    });

  const onKey = (e) => {
    if (e.target.tagName === "INPUT") return;
    if (e.key === "Escape") {
      onSelect(null);
      setHover(null);
      fit();
    } else if (e.key === "+" || e.key === "=") zoomAt(0 - target.current.x / target.current.k, 0 - target.current.y / target.current.k, 1.3);
    else if (e.key === "-") zoomAt(-target.current.x / target.current.k, -target.current.y / target.current.k, 1 / 1.3);
    else if (e.key === "0") fit();
  };

  // Screen position (in % of the canvas) of a scene point, for the hover card.
  const toScreen = (x, y) => {
    const v = view.current;
    return [((x * v.k + v.x - VB.x) / VB.w) * 100, ((y * v.k + v.y - VB.y) / VB.h) * 100];
  };
  const hoverNode = hover && !drag.current ? nodes.find((n) => n.id === hover) : null;
  const focusedDept = deptFocus && nodes.find((n) => n.id === deptFocus);

  return (
    <div className="relative select-none outline-none" tabIndex={-1} onKeyDown={onKey}>
      <svg
        ref={svgRef}
        viewBox={`${VB.x} ${VB.y} ${VB.w} ${VB.h}`}
        className="block w-full cursor-grab"
        style={{ touchAction: zoomed ? "none" : "pan-y" }}
        role="img"
        aria-label="Org chart network: the policy engine, departments, teams and workers"
        onPointerDown={down}
        onDoubleClick={(e) => {
          if (e.target.closest?.("[data-node]")) return;
          const p = toSvg(e.clientX, e.clientY);
          zoomAt(p.x, p.y, 1.6);
        }}
      >
        <defs>
          <radialGradient id="g-ai" cx="30%" cy="25%" r="80%">
            <stop offset="0" stopColor="#fff" />
            <stop offset=".2" stopColor="#c9bbff" />
            <stop offset=".55" stopColor="#8b6cff" />
            <stop offset="1" stopColor="#1c1440" />
          </radialGradient>
          <radialGradient id="g-human" cx="30%" cy="25%" r="80%">
            <stop offset="0" stopColor="#fff" />
            <stop offset=".2" stopColor="#c5f1ff" />
            <stop offset=".55" stopColor="#5fd4ff" />
            <stop offset="1" stopColor="#0a2e3d" />
          </radialGradient>
          <radialGradient id="g-core" cx="35%" cy="30%" r="75%">
            <stop offset="0" stopColor="rgba(255,255,255,.7)" />
            <stop offset=".25" stopColor="#2a2340" />
            <stop offset="1" stopColor="#050507" />
          </radialGradient>
          <filter id="glow" x="-50%" y="-50%" width="200%" height="200%">
            <feGaussianBlur stdDeviation="4" result="b" />
            <feMerge><feMergeNode in="b" /><feMergeNode in="SourceGraphic" /></feMerge>
          </filter>
          <filter id="halo" x="-100%" y="-100%" width="300%" height="300%"><feGaussianBlur stdDeviation="22" /></filter>
        </defs>

        <g ref={sceneRef}>
          {/* rings */}
          {[R.dept, R.team, R.worker].map((r, i) => (
            <circle key={r} r={r} fill="none" stroke="rgba(255,255,255,.05)" strokeDasharray={i === 2 ? "2 6" : undefined} className="net-fade" style={{ "--d": `${i * 0.15}s` }} />
          ))}
          <circle r={R.worker + 60} fill="none" stroke="rgba(255,255,255,.03)" />

          {/* cables */}
          {edges.map((e) => {
            const color = e.tone === "core" ? "#8b6cff" : e.tone === "dept" ? "rgba(255,255,255,.35)" : EDGE[e.tone] || "#8d8c96";
            const dead = e.tone === "fired";
            const shown = edgeVisible(e);
            const lit = shown && onRoute(e);
            const strong = chain && lit;
            return (
              <g key={e.id} opacity={!shown ? 0.06 : lit ? 1 : 0.14} style={{ transition: "opacity .45s ease" }}>
                <path d={e.d} fill="none" stroke={color} strokeOpacity={dead ? 0.2 : 0.1} strokeWidth="6" className="net-fade" style={{ "--d": `${e.delay}s` }} />
                <path
                  id={e.id}
                  d={e.d}
                  pathLength={dead ? undefined : 1}
                  fill="none"
                  stroke={color}
                  strokeWidth={strong ? 2.6 : 1.4}
                  strokeDasharray={dead ? "3 7" : undefined}
                  filter={strong ? "url(#glow)" : undefined}
                  className={dead ? "net-fade" : "net-draw"}
                  style={{ "--d": `${e.delay}s`, transition: "stroke-width .3s ease" }}
                />
                {MOTION && !dead && shown && (
                  <circle r={strong ? 3.2 : 2.2} fill={color} opacity=".85">
                    <animateMotion dur={`${strong ? 2.4 : 7 + (e.id.charCodeAt(e.id.length - 1) % 7) * 0.6}s`} begin="1.8s" repeatCount="indefinite" rotate="auto">
                      <mpath href={`#${e.id}`} />
                    </animateMotion>
                  </circle>
                )}
              </g>
            );
          })}

          {/* core */}
          <g className="cursor-pointer" data-node onClick={click(fit)} aria-label="Reset view">
            <g className="net-pop" style={{ "--d": "0s" }}>
              <circle r="80" fill="#8b6cff" opacity=".2" filter="url(#halo)" />
              <circle r="46" fill="url(#g-core)" stroke="rgba(212,255,58,.6)" strokeWidth="1.2" />
              <circle r="54" fill="none" stroke="rgba(255,255,255,.12)" />
              <text y="-4" textAnchor="middle" className="fill-fog font-display text-[13px] font-semibold">POLICY</text>
              <text y="12" textAnchor="middle" className="fill-lime font-mono text-[9px] tracking-[0.2em]">ENGINE</text>
            </g>
          </g>

          {/* nodes */}
          {nodes.map((n) => {
            const [x, y] = polar(n.r, n.a);
            const right = Math.cos(n.a) >= -0.01;
            if (n.kind === "dept") {
              const dim = deptFocus && deptFocus !== n.id;
              const on = deptFocus === n.id;
              return (
                <g
                  key={n.id}
                  transform={`translate(${x},${y})`}
                  data-node
                  className="cursor-pointer"
                  role="button"
                  tabIndex={0}
                  aria-label={`${n.name}: ${n.count} workers. ${on ? "Show all departments" : "Zoom into department"}`}
                  onClick={click(() => openDept(n))}
                  onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && openDept(n)}
                  opacity={dim ? 0.3 : 1}
                  style={{ transition: "opacity .45s ease" }}
                >
                  <g className="net-pop" style={{ "--d": "0.3s" }}>
                    <circle r="30" fill="#0f0f14" stroke={on ? "#d4ff3a" : "rgba(255,255,255,.25)"} strokeWidth={on ? 1.6 : 1} style={{ transition: "stroke .3s" }} />
                    <text textAnchor="middle" y="4" className="fill-fog font-display text-[11px] font-semibold">{n.name.slice(0, 3).toUpperCase()}</text>
                    <g transform="translate(21,-21)">
                      <circle r="9" fill="#17171f" stroke="rgba(255,255,255,.2)" />
                      <text textAnchor="middle" y="3.5" className="fill-mute font-mono text-[9px]">{n.count}</text>
                    </g>
                    <text textAnchor="middle" y="48" className="net-label fill-mute font-mono text-[10px] uppercase tracking-[0.15em]">{n.name}</text>
                  </g>
                </g>
              );
            }
            if (n.kind === "team")
              return (
                <g key={n.id} transform={`translate(${x},${y})`} opacity={deptFocus && deptFocus !== n.dept ? 0.25 : 1} style={{ transition: "opacity .45s ease" }}>
                  <g className="net-pop" style={{ "--d": "0.6s" }}>
                    <rect x="-8" y="-8" width="16" height="16" rx="3" transform="rotate(45)" fill="#17171f" stroke="rgba(255,255,255,.4)" />
                    <text x={right ? 14 : -14} y="-12" textAnchor={right ? "start" : "end"} className="net-label fill-mute font-mono text-[9px] uppercase tracking-widest">{n.name}</text>
                  </g>
                </g>
              );
            const w = n.w;
            const ai = w.kind === "ai_agent";
            const fired = w.status === "fired";
            const isFocus = focus === w.id;
            const isSelected = selected === w.id;
            const shown = visibleIds.has(n.id);
            return (
              <g
                key={n.id}
                transform={`translate(${x},${y})`}
                data-node
                className="cursor-pointer outline-none"
                tabIndex={0}
                role="button"
                aria-label={`${w.name}, ${w.role}, ${human(w.status)}`}
                onPointerEnter={() => !drag.current && (!deptFocus || deptFocus === n.dept) && setHover(n.id)}
                onPointerLeave={() => setHover(null)}
                onFocus={() => setHover(n.id)}
                onBlur={() => setHover(null)}
                onClick={click(() => selectWorker(n))}
                onDoubleClick={() => flyTo(n, 2.2)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    if (deptFocus && deptFocus !== n.dept) setDeptFocus(null);
                    onSelect(w);
                    flyTo(n);
                  }
                }}
                opacity={!shown ? 0.12 : focus && !isFocus ? 0.45 : fired ? 0.55 : 1}
                style={{ transition: "opacity .45s ease" }}
              >
                <g className="net-pop" style={{ "--d": `${1 + n.order * 0.05}s` }}>
                  {isSelected && <circle r="24" fill="none" stroke={EDGE[w.status] || "#8d8c96"} strokeWidth="2" className="net-ping" />}
                  {isFocus && <circle r="34" fill={ai ? "#8b6cff" : "#5fd4ff"} opacity=".45" filter="url(#halo)" />}
                  <circle r="24" fill="none" stroke={EDGE[w.status] || "#8d8c96"} strokeWidth={isFocus ? 2.5 : 1.5} strokeDasharray={fired ? "3 4" : undefined} />
                  <circle r={isFocus ? 21 : 19} fill={`url(#g-${ai ? "ai" : "human"})`} style={{ filter: fired ? "grayscale(1)" : undefined, transition: "r .25s ease" }} />
                  <text textAnchor="middle" y="4" className="pointer-events-none fill-ink font-display text-[10px] font-bold">
                    {w.name.split(/\s+/).map((p) => p[0]).slice(0, 2).join("").toUpperCase()}
                  </text>
                  <text x={right ? 32 : -32} y="-2" textAnchor={right ? "start" : "end"} className={`net-label font-sans text-[12px] font-medium ${fired ? "fill-dim" : "fill-fog"}`}>{short(w.name)}</text>
                  <text x={right ? 32 : -32} y="12" textAnchor={right ? "start" : "end"} className="net-label fill-mute font-mono text-[9px]">
                    {ai ? "AI" : "HUMAN"} · {human(w.status).toUpperCase()}
                  </text>
                </g>
              </g>
            );
          })}
        </g>
      </svg>

      {/* Hover card */}
      {hoverNode && hoverNode.kind === "worker" && (() => {
        const [x, y] = polar(hoverNode.r, hoverNode.a);
        const [left, top] = toScreen(x, y);
        const w = hoverNode.w;
        return (
          <div
            className="net-tip pointer-events-none absolute z-10 w-60 -translate-x-1/2 rounded-2xl bg-ink/90 p-3.5 shadow-2xl ring-1 ring-white/10 backdrop-blur-md"
            style={{ left: `${left}%`, top: `${top}%`, transform: `translate(-50%, ${top > 50 ? "calc(-100% - 34px)" : "34px"})` }}
            role="tooltip"
          >
            <div className="flex items-center justify-between gap-2">
              <p className="truncate text-sm font-semibold">{w.name}</p>
              <span className="shrink-0 font-mono text-[10px] uppercase tracking-wider" style={{ color: EDGE[w.status] || "#a6a5ae" }}>{human(w.status)}</span>
            </div>
            <p className="text-xs text-mute">{w.role} · {hoverNode.team}</p>
            <p className="mt-2 text-xs"><span className="num text-fog">{usd(w.pay_rate_usd, Number(w.pay_rate_usd) % 1 ? 2 : 0)}</span><span className="text-mute"> / {human(w.pay_unit)}</span></p>
            {w.skills?.length > 0 && (
              <div className="mt-2 flex flex-wrap gap-1">
                {w.skills.slice(0, 4).map((s) => <span key={s} className="rounded-md bg-white/5 px-1.5 py-0.5 font-mono text-[10px] text-mute">{s}</span>)}
              </div>
            )}
            <p className="mt-2.5 text-[10px] text-dim">Click to open · double-click to zoom</p>
          </div>
        );
      })()}

      {/* Search and breadcrumb */}
      <div className="absolute left-3 top-3 z-10 flex flex-col items-start gap-2">
        <label className="flex h-9 w-56 items-center gap-2 rounded-full bg-ink/80 px-3 ring-1 ring-white/10 backdrop-blur-md transition focus-within:ring-lime/50 sm:w-64">
          <Icon name="search" size={14} className="shrink-0 text-dim" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onFocus={() => setSearchOpen(true)}
            onBlur={() => setTimeout(() => setSearchOpen(false), 150)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                pick(results[0]);
                e.currentTarget.blur();
              }
              if (e.key === "Escape") setQuery("");
            }}
            placeholder="Find a worker or skill"
            aria-label="Find a worker or skill"
            className="min-w-0 flex-1 bg-transparent text-sm text-fog placeholder:text-dim focus:outline-none"
          />
          {q && <span className="num shrink-0 text-[11px] text-mute">{results.length}</span>}
          {q && (
            <button onClick={() => setQuery("")} className="shrink-0 rounded-full p-0.5 text-dim hover:text-fog" aria-label="Clear search">
              <Icon name="x" size={12} />
            </button>
          )}
        </label>
        {q && searchOpen && results.length > 0 && (
          <ul className="w-56 overflow-hidden rounded-2xl bg-ink/90 py-1 ring-1 ring-white/10 backdrop-blur-md sm:w-64">
            {results.slice(0, 5).map((n) => (
              <li key={n.id}>
                <button
                  className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm hover:bg-white/5"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => pick(n)}
                >
                  <span className="shrink-0 rounded-full size-2" style={{ background: EDGE[n.w.status] }} />
                  <span className="truncate">{n.w.name}</span>
                  <span className="ml-auto truncate text-xs text-mute">{n.w.role}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
        {focusedDept && (
          <div className="flex items-center gap-1 rounded-full bg-ink/80 py-1 pl-3 pr-1 text-xs ring-1 ring-white/10 backdrop-blur-md">
            <button className="text-mute hover:text-fog" onClick={fit}>All departments</button>
            <span className="text-dim">/</span>
            <span className="font-medium text-lime">{focusedDept.name}</span>
            <button className="ml-1 rounded-full p-1 text-mute hover:bg-white/10 hover:text-fog" onClick={fit} aria-label="Show all departments">
              <Icon name="x" size={12} />
            </button>
          </div>
        )}
      </div>

      {/* Camera controls */}
      <div className="absolute right-3 top-3 z-10 flex flex-col overflow-hidden rounded-2xl bg-ink/80 ring-1 ring-white/10 backdrop-blur-md">
        {[
          ["plus", "Zoom in", () => zoomAt(-target.current.x / target.current.k, -target.current.y / target.current.k, 1.35)],
          ["minus", "Zoom out", () => zoomAt(-target.current.x / target.current.k, -target.current.y / target.current.k, 1 / 1.35)],
          ["fit", "Fit to view", fit],
        ].map(([icon, label, fn]) => (
          <button key={icon} onClick={fn} aria-label={label} title={label} className="flex items-center justify-center text-mute transition size-9 hover:bg-white/10 hover:text-fog">
            <Icon name={icon} size={16} />
          </button>
        ))}
      </div>

      {/* Legend: click to filter */}
      <div className="absolute bottom-3 left-3 right-3 z-10 flex flex-wrap items-center gap-1.5">
        {STATUSES.map(([k, l]) => {
          const on = statuses.has(k);
          return (
            <button
              key={k}
              onClick={() => toggleStatus(k)}
              aria-pressed={on}
              className={`flex items-center gap-1.5 rounded-full px-2.5 py-1 font-mono text-[10px] uppercase tracking-wider ring-1 backdrop-blur-md transition ${on ? "bg-ink/80 text-fog ring-white/15" : "bg-ink/40 text-dim ring-white/5 line-through"}`}
            >
              <i className="h-0.75 w-3.5 rounded-full transition-opacity" style={{ background: EDGE[k], opacity: on ? 1 : 0.3 }} />
              {l}
            </button>
          );
        })}
        <span className="mx-1 h-4 w-px bg-white/10" />
        {[["all", "All"], ["ai", "AI"], ["human", "Human"]].map(([k, l]) => (
          <button
            key={k}
            onClick={() => setKind(k)}
            aria-pressed={kind === k}
            className={`rounded-full px-2.5 py-1 font-mono text-[10px] uppercase tracking-wider ring-1 backdrop-blur-md transition ${kind === k ? "bg-lime text-ink ring-lime" : "bg-ink/80 text-mute ring-white/10 hover:text-fog"}`}
          >
            {l}
          </button>
        ))}
        <span className="ml-auto hidden font-mono text-[10px] text-dim lg:block">Drag to pan · pinch or ⌘ scroll to zoom · Esc resets</span>
      </div>
    </div>
  );
}
