import { useEffect, useState } from "react";
import { RotateCcw } from "lucide-react";
import { reducedMotion, useInView } from "../lib/hooks.js";

// What a first ./scripts/start demo run walks through (judge mode). Shortened.
const LINES = [
  { cls: "p", text: "$ ./scripts/start demo", typed: true },
  { text: "Starting ForeMan in judge mode. The first run builds everything and takes a few minutes." },
  { text: "postgres   local container on :55432" },
  { text: "openbao    dev mode · treasury key created, non-exportable" },
  { text: "treasury   NEAR testnet account funded from the faucet" },
  { cls: "v", text: "planner    AI built 5 departments · 5 agents · 1 hiring request (Brand Designer)" },
  { cls: "s", text: "interview  AI disclosed · consent recorded · declined to discuss pay" },
  { cls: "err", text: "review     AI fired Ops Agent after 2 below-threshold periods · board notified" },
  { cls: "ok", text: "payout     $5.00   sent on NEAR testnet (signed in OpenBao)" },
  { cls: "warn", text: "payout     $60.00  escalated to the board (≥ $50 in 24h)" },
  { cls: "err", text: "payout     $400.00 blocked: cap_exceeded (board notified)" },
  { text: "report     judge/out/report.json" },
  { cls: "ok", text: "The company is running itself. Website: http://localhost:5175 (Sign in → dashboard)" },
];

export default function Terminal() {
  const [ref, inView] = useInView({ threshold: 0.35 });
  const [shown, setShown] = useState(0);
  const [typed, setTyped] = useState(0);
  const [run, setRun] = useState(0);

  useEffect(() => {
    if (!inView) return;
    if (reducedMotion()) {
      setTyped(LINES[0].text.length);
      setShown(LINES.length);
      return;
    }
    let t;
    if (typed < LINES[0].text.length) t = setTimeout(() => setTyped((n) => n + 1), 70);
    else if (shown < LINES.length) t = setTimeout(() => setShown((n) => Math.max(1, n + 1)), shown === 0 ? 300 : 380);
    return () => clearTimeout(t);
  }, [inView, typed, shown, run]);

  const replay = () => {
    setShown(0);
    setTyped(0);
    setRun((r) => r + 1);
  };

  return (
    <div className="term" ref={ref}>
      <div className="term-bar">
        <i /><i /><i />
        <span>~/axom — zsh</span>
      </div>
      <div className="term-body" aria-label="Example output of ./scripts/start demo">
        <span className="p">{LINES[0].text.slice(0, typed)}</span>
        {shown === 0 && <span className="caret" />}
        {"\n"}
        {LINES.slice(1, shown).map((l, i) => (
          <span key={i} className={l.cls}>{l.text}{"\n"}</span>
        ))}
        {shown >= LINES.length && (
          <>
            <span className="p">$ </span>
            <span className="caret" />
          </>
        )}
      </div>
      {shown >= LINES.length && (
        <div style={{ padding: "0 20px 16px", textAlign: "right" }}>
          <button type="button" className="btn btn-ghost btn-sm" onClick={replay}><RotateCcw aria-hidden="true" /> Replay</button>
        </div>
      )}
    </div>
  );
}
