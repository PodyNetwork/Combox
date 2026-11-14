import { useCallback, useEffect, useRef, useState } from "react";
import { Ban, Bot, CircleCheck, FileSignature, Hourglass, OctagonAlert, Send, UserMinus, Video, Wrench } from "lucide-react";
import { useInterval, useOnScreen, usd } from "../lib/hooks.js";
import { Orb } from "./ui.jsx";

// A scripted day in an autonomous company. Every line is something ForeMan really
// does; the amounts match the judge-mode demo run.
const SCRIPT = [
  { tone: "violet", icon: Bot, text: <><b>Task routed</b> · Build the MVP web app → Software Engineer Agent 1</> },
  { tone: "lime", icon: Send, text: <><b>Payout sent</b> · $5.00 → design-ana.near · under $50, automatic</> },
  { tone: "sky", icon: Video, text: <><b>Interview booked</b> · Brand Designer · AI disclosed, consent asked</> },
  { tone: "ember", icon: Ban, text: <><b>Payout blocked</b> · $400.00 · would cross the $500 monthly cap</> },
  { tone: "violet", icon: Wrench, text: <><b>Tool call allowed</b> · github.create_issue · on the worker's list</> },
  { tone: "lime", icon: FileSignature, text: <><b>AI hired</b> · Brand Designer · freelancer-services-v1 sent · board notified</> },
  { tone: "sky", icon: Bot, text: <><b>Post scheduled</b> · launch campaign · Content Marketer Agent</> },
  { tone: "ember", icon: UserMinus, text: <><b>AI fired</b> · Ops Agent · 2 bad review periods in a row</> },
  { tone: "violet", icon: Bot, text: <><b>Lead added to CRM</b> · outbound sequence started · Sales Dev Agent</> },
  { tone: "violet", icon: Bot, text: <><b>Task routed</b> · Run the launch campaign → Content Marketer Agent</> },
  { tone: "lime", icon: Send, text: <><b>Payout sent</b> · $12.00 → sdr-agent.near · under $50, automatic</> },
];

const CAP = 500;
const OTP = "481207";

export default function ControlRoom() {
  const [wrapRef, onScreen] = useOnScreen();
  const [killed, setKilled] = useState(false);
  // Clock times only in the browser: the prerendered feed has none, so it hydrates cleanly.
  const [feed, setFeed] = useState(() => SCRIPT.slice(0, 4).map((s, i) => ({ ...s, id: i, t: "" })).reverse());
  const idx = useRef(4);
  const nextId = useRef(100);
  const [used, setUsed] = useState(186);
  const [reserved, setReserved] = useState(61);
  const [approval, setApproval] = useState("pending"); // pending | mfa | sent
  const [otp, setOtp] = useState(0);

  useEffect(() => setFeed((f) => f.map((item, i) => ({ ...item, t: stamp(-i * 3) }))), []);

  const push = useCallback((item) => {
    setFeed((f) => [{ ...item, id: nextId.current++, t: stamp(0) }, ...f].slice(0, 7));
  }, []);

  useInterval(
    () => {
      const s = SCRIPT[idx.current % SCRIPT.length];
      idx.current += 1;
      push(s);
      if (s.tone === "lime" && s.icon === Send) setUsed((u) => Math.min(CAP - reserved, u + (idx.current % 2 ? 5 : 12)));
    },
    2600,
    onScreen && !killed,
  );

  // The MFA code types itself in, then the held payout goes out.
  useEffect(() => {
    if (approval !== "mfa" || killed) return;
    if (otp < OTP.length) {
      const t = setTimeout(() => setOtp((n) => n + 1), 140);
      return () => clearTimeout(t);
    }
    const t = setTimeout(() => {
      setApproval("sent");
      setReserved(0);
      setUsed((u) => u + 61);
      push({ tone: "lime", icon: CircleCheck, text: <><b>Approved and sent</b> · $60.00 → mara.near · signed in OpenBao</> });
    }, 450);
    return () => clearTimeout(t);
  }, [approval, otp, push, killed]);

  const toggleKill = () => {
    setKilled((k) => {
      push(
        k
          ? { tone: "lime", icon: CircleCheck, text: <><b>Kill switch off</b> · company resumed by MFA admin</> }
          : { tone: "ember", icon: OctagonAlert, text: <><b>Kill switch on</b> · the whole company halted</> },
      );
      return !k;
    });
  };

  const reset = () => {
    setApproval("pending");
    setOtp(0);
    setReserved(61);
    setUsed(186);
  };

  const left = CAP - used - reserved;

  return (
    <div ref={wrapRef} className={`cr fade-up ${killed ? "halted" : ""}`} style={{ "--d": "0.5s" }}>
      <Orb className={`cr-orb ${killed ? "halted" : ""}`} />
      <div className="panel cr-window">
        <div className="cr-bar">
          <div className="lights" aria-hidden="true"><i /><i /><i /></div>
          <div className="url">foreman · control room</div>
          <span className={`badge ${killed ? "badge-ember" : "badge-lime"}`}>{killed ? "Halted" : "Autonomous"}</span>
        </div>
        {killed && (
          <div className="halt-banner" role="status">
            <OctagonAlert aria-hidden="true" /> Kill switch on. The whole company is halted.
          </div>
        )}
        <div className="cr-body">
          <div className="cr-tile">
            <h4>Monthly cap</h4>
            <div className="cr-cap-num">{usd(used, 0)} <small>of {usd(CAP, 0)}</small></div>
            <div className="meter" role="img" aria-label={`${usd(used)} sent, ${usd(reserved)} reserved, ${usd(left)} left of the ${usd(CAP, 0)} monthly cap`}>
              <i className="m-used" style={{ width: `${(used / CAP) * 100}%` }} />
              <i className="m-res" style={{ width: `${(reserved / CAP) * 100}%` }} />
            </div>
            <div className="legend">
              <span><i style={{ background: "var(--lime)" }} />Sent</span>
              <span><i style={{ background: "var(--amber)" }} />Reserved</span>
              <span>{usd(left, 0)} left</span>
            </div>
          </div>

          <div className="cr-tile cr-approve">
            <h4>Escalated to the board</h4>
            {approval === "sent" ? (
              <>
                <div className="who"><span className="amt lime">Sent</span><span className="badge badge-lime">On NEAR</span></div>
                <span className="small muted">$60.00 → mara.near</span>
                <button type="button" className="btn btn-ghost btn-sm" onClick={reset}>Replay</button>
              </>
            ) : (
              <>
                <div className="who">
                  <span className="amt">$60.00</span>
                  <span className="badge badge-amber">≥ $50</span>
                </div>
                <span className="addr">mara.near · Brand Designer</span>
                {approval === "mfa" ? (
                  <div className="otp" aria-label="Authenticator code">
                    {OTP.split("").map((d, i) => (
                      <span key={i} className={i < otp ? "on" : ""}>{i < otp ? d : ""}</span>
                    ))}
                  </div>
                ) : (
                  <button type="button" className="btn btn-lime btn-sm" disabled={killed} onClick={() => setApproval("mfa")}>
                    {killed ? "Halted" : "Sign off with MFA"}
                  </button>
                )}
              </>
            )}
          </div>

          <div className="cr-tile cr-span">
            <div className="kill">
              <div>
                <h4>Decisions made by AI</h4>
              </div>
              <div className="row" style={{ gap: 10 }}>
                <span className="tiny muted" id="kill-label">Kill switch</span>
                <button type="button" role="switch" aria-checked={killed} aria-labelledby="kill-label" className="switch" onClick={toggleKill} />
              </div>
            </div>
            <ul className="cr-feed" aria-live="off">
              {feed.map((f) => {
                const Icon = f.icon;
                return (
                  <li key={f.id} className={`tone-${f.tone}`}>
                    <span className="ic"><Icon aria-hidden="true" /></span>
                    <span className="txt">{f.text}</span>
                    <span className="t">{f.t}</span>
                  </li>
                );
              })}
            </ul>
          </div>
        </div>
      </div>
      <p className="tiny dim center mt-s">
        <Hourglass size={12} style={{ display: "inline", verticalAlign: "-2px" }} aria-hidden="true" /> Interactive: the company runs itself. Sign off the escalated payout, or flip the kill switch.
      </p>
    </div>
  );
}

function stamp(offsetSecs) {
  const d = new Date(Date.now() + offsetSecs * 1000);
  return d.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}
