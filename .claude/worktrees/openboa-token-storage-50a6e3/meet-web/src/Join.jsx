// Join by link: /join?t=<invite token>. The token is exchanged at the Combox
// API (POST /meetings/join) for a short-lived LiveKit participant token.
// A lobby states up front that an AI interviewer is present and will ask for
// consent before anything is recorded. In the room, live captions sit under
// the participant tiles, Google Meet style.
import { useEffect, useRef, useState } from "react";
import { LiveKitRoom } from "@livekit/components-react";
import { DisconnectReason } from "livekit-client";
import { ArrowRight, Clock, Cpu, Microphone2, MicrophoneSlash1, MoneyRemove, Record, ShieldTick, Video, VideoSlash } from "iconsax-react";
import LiveRoom from "./LiveRoom.jsx";
import { API, Breakthrough, Kinetic, Notice, Wordmark } from "./ui.jsx";

export default function Join() {
  const invite = new URLSearchParams(window.location.search).get("t");
  const [join, setJoin] = useState(null);
  const [error, setError] = useState(invite ? null : "This link is missing its invite token. Open the exact link from your invitation email.");
  const [entered, setEntered] = useState(false);
  const [left, setLeft] = useState(false);
  const [video, setVideo] = useState(true);
  const [audio, setAudio] = useState(true);
  const [connError, setConnError] = useState(null);
  const [deviceIssue, setDeviceIssue] = useState(null);
  const connected = useRef(false);
  const [attempt, setAttempt] = useState(0);
  const fetchedAt = useRef(0);
  // Room tokens last 15 minutes; refresh before entering if this one is old.
  const STALE_MS = 10 * 60 * 1000;

  // Exchange the invite for a short-lived room token. Runs again on
  // "Rejoin" / "Try again", since the previous room token may have expired.
  useEffect(() => {
    if (!invite) return;
    let live = true;
    setJoin(null);
    fetch(`${API}/meetings/join`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token: invite }),
    })
      .then(async (r) => {
        const body = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(body.error || `Could not join (HTTP ${r.status})`);
        if (!live) return;
        fetchedAt.current = Date.now();
        setJoin(body);
      })
      .catch((e) => live && setError(e instanceof TypeError ? "We couldn't reach the Combox server. Check your connection and try the link again." : e.message));
    return () => {
      live = false;
    };
  }, [invite, attempt]);

  // Enter the room, fetching a fresh token first if the lobby sat open too long.
  const enter = () => {
    if (Date.now() - fetchedAt.current > STALE_MS) {
      setAutoEnter(true);
      setAttempt((n) => n + 1);
    } else setEntered(true);
  };
  const [autoEnter, setAutoEnter] = useState(false);
  useEffect(() => {
    if (autoEnter && join) {
      setAutoEnter(false);
      setEntered(true);
    }
  }, [autoEnter, join]);

  const restart = () => {
    connected.current = false;
    setLeft(false);
    setConnError(null);
    setDeviceIssue(null);
    setEntered(false);
    setAttempt((n) => n + 1);
  };

  if (error) return <Notice tone="error" title="Can't join this meeting" text={error} />;
  if (connError)
    return (
      <Notice tone="error" title="Couldn't connect to the room" text={connError}>
        <button className="btn btn-lime" style={{ marginTop: 28 }} onClick={restart}>Try again</button>
      </Notice>
    );
  if (left)
    return (
      <Notice title="You left the meeting" text="Thanks for your time. You can close this tab, or rejoin with the same link while the meeting is open.">
        <button className="btn btn-ghost" style={{ marginTop: 28 }} onClick={restart}>Rejoin</button>
      </Notice>
    );
  if (!join) return <Notice title="Getting your room ready…" text="Checking your invitation." />;
  if (!entered) return <Lobby title={join.title} minutes={join.duration_minutes || 10} expiresAt={join.expires_at} video={video} audio={audio} setVideo={setVideo} setAudio={setAudio} onEnter={enter} />;

  return (
    <div className="room" data-lk-theme="default">
      <header className="room-bar">
        <Wordmark />
        <strong>{join.title}</strong>
        <span className="live"><Record size={14} variant="Bold" color="currentColor" /> LIVE</span>
        <RoomTimer minutes={join.duration_minutes || 10} />
        <span className="ai-note"><Cpu size={15} variant="Bulk" color="currentColor" /> AI interviewer present · asks consent before recording</span>
      </header>
      {deviceIssue && (
        <p className="device-issue" role="alert">
          {deviceIssue}
          <button onClick={() => setDeviceIssue(null)} aria-label="Dismiss">✕</button>
        </p>
      )}
      <div className="room-stage">
          <LiveKitRoom
            serverUrl={join.ws_url}
            token={join.token}
            connect
            video={video}
            audio={audio}
            onConnected={() => (connected.current = true)}
            onDisconnected={(reason) => {
              if (reason === DisconnectReason.CLIENT_INITIATED) return setLeft(true);
              if (!connected.current) return setConnError("The interview room didn't answer. It may not have started yet, or your network is blocking it.");
              if (reason === DisconnectReason.DUPLICATE_IDENTITY) return setConnError("You joined this interview from another tab or device, so this one was disconnected.");
              if (reason === DisconnectReason.ROOM_DELETED) return setLeft(true);
              setConnError("You were disconnected from the interview. Check your connection, then rejoin.");
            }}
            onError={(e) => setConnError(e?.message ? `The room connection failed: ${e.message}` : "The room connection failed.")}
            onMediaDeviceFailure={() =>
              setDeviceIssue("Your camera or microphone is blocked. Allow access in your browser's site settings, then turn it back on below.")
            }
          >
            <LiveRoom onDeviceIssue={setDeviceIssue} />
          </LiveKitRoom>
      </div>
    </div>
  );
}

// Time left in the interview. The interviewer ends it at the limit; this
// counts down from when the candidate entered the room.
function RoomTimer({ minutes }) {
  const [start] = useState(() => Date.now());
  const [now, setNow] = useState(start);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  const left = Math.max(0, minutes * 60 - Math.floor((now - start) / 1000));
  const mm = String(Math.floor(left / 60)).padStart(2, "0");
  const ss = String(left % 60).padStart(2, "0");
  return (
    <span className={`timer${left <= 60 ? " timer-low" : ""}`} role="timer" aria-label={`${Math.ceil(left / 60)} minutes left`}>
      <Clock size={14} variant="Bold" color="currentColor" /> {mm}:{ss} left
    </span>
  );
}

function Lobby({ title, minutes, expiresAt, video, audio, setVideo, setAudio, onEnter }) {
  const expires = expiresAt ? new Date(expiresAt).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }) : null;
  return (
    <div className="lobby">
      <section className="lobby-art">
        <Wordmark />
        <Breakthrough word="HELLO" />
        <h1>
          <Kinetic text="A real conversation," />
          <br />
          <em><Kinetic text="with an AI interviewer." delay={19} /></em>
        </h1>
      </section>
      <section className="lobby-form">
        <div className="inner">
          <p className="eyebrow">You're invited</p>
          <h2><Kinetic text={title} delay={10} /></h2>
          <div className="disclosure">
            <ul>
              <li>
                <span className="dot" style={{ background: "rgb(139 108 255 / .18)", color: "var(--violet)" }}><Cpu size={16} variant="Bulk" color="currentColor" /></span>
                <span><b>The interviewer is an AI agent</b>It will say so at the start. A person on the team reviews the summary.</span>
              </li>
              <li>
                <span className="dot" style={{ background: "rgb(212 255 58 / .14)", color: "var(--lime)" }}><ShieldTick size={16} variant="Bulk" color="currentColor" /></span>
                <span><b>Nothing is recorded without your yes</b>It asks for consent first. Say no and the interview ends, with no recording.</span>
              </li>
              <li>
                <span className="dot" style={{ background: "rgb(255 181 71 / .14)", color: "var(--amber)" }}><MoneyRemove size={16} variant="Bulk" color="currentColor" /></span>
                <span><b>It won't discuss pay</b>Rates are on the posting; offers come from a person.</span>
              </li>
              <li>
                <span className="dot" style={{ background: "rgb(95 212 255 / .14)", color: "var(--sky)" }}><Clock size={16} variant="Bulk" color="currentColor" /></span>
                <span><b>It takes {minutes} minutes</b>The interviewer wraps up at the time limit.{expires && ` This link expires at ${expires}.`}</span>
              </li>
            </ul>
          </div>
          <div className="toggles" role="group" aria-label="Devices">
            <button type="button" className="toggle" aria-pressed={video} onClick={() => setVideo(!video)}>
              {video ? <Video size={18} variant="Bold" color="currentColor" /> : <VideoSlash size={18} variant="Linear" color="currentColor" />}
              {video ? "Camera on" : "Camera off"}
            </button>
            <button type="button" className="toggle" aria-pressed={audio} onClick={() => setAudio(!audio)}>
              {audio ? <Microphone2 size={18} variant="Bold" color="currentColor" /> : <MicrophoneSlash1 size={18} variant="Linear" color="currentColor" />}
              {audio ? "Mic on" : "Mic off"}
            </button>
          </div>
          <button className="btn btn-lime btn-lg btn-block" onClick={onEnter}>Join the interview <ArrowRight size={18} color="currentColor" /></button>
          <p className="muted" style={{ marginTop: 14, fontSize: 12, textAlign: "center" }}>Your browser will ask for camera and microphone access.</p>
        </div>
      </section>
    </div>
  );
}
