// Join by link: /join?t=<invite token>. The token is exchanged at the Combox
// API (POST /meetings/join) for a short-lived LiveKit participant token; that
// call also sends the AI interviewer, which gets ready while the person is in
// the lobby. A lobby states up front that an AI interviewer is present and
// will ask for consent before anything is recorded.
//
// Speed: the lobby shows at once (the invite is checked behind it), the
// camera and mic open in the lobby with a preview, and the room's code
// (LiveKit) loads in the background, so "Join" connects straight away.
import { Suspense, lazy, useEffect, useRef, useState } from "react";
import { ArrowRight, Clock, Cpu, Microphone2, MicrophoneSlash1, MoneyRemove, ShieldTick, Video, VideoSlash } from "iconsax-react";
import { API, Breakthrough, Kinetic, Notice, Wordmark } from "./ui.jsx";

const loadRoom = () => import("./LiveRoom.jsx");
const Room = lazy(loadRoom);
// Room tokens last 15 minutes; refresh before entering if this one is old.
const STALE_MS = 10 * 60 * 1000;
// A small camera picture: an interview needs a face, not HD, and the voice
// needs the bandwidth on a weak network.
const VIDEO = { width: { ideal: 640 }, height: { ideal: 360 }, frameRate: { ideal: 24 } };
const AUDIO = { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1 };

const ask = (kinds) =>
  navigator.mediaDevices.getUserMedia({ video: kinds.includes("video") && VIDEO, audio: kinds.includes("audio") && AUDIO }).then((s) => s.getTracks());
// One permission prompt for both; if that fails (say, there is no camera),
// open each on its own so the other still works.
const openMedia = (kinds) =>
  ask(kinds).catch(async (e) => {
    if (kinds.length < 2) throw e;
    const got = (await Promise.allSettled(kinds.map((k) => ask([k])))).flatMap((r) => (r.status === "fulfilled" ? r.value : []));
    if (!got.length) throw e;
    return got;
  });

// The camera and mic, opened in the lobby while `active`. Turning one off
// releases it. The room publishes the open tracks when it connects.
function useLocalMedia(video, audio, active, onBlocked) {
  const tracks = useRef({ video: null, audio: null });
  const [preview, setPreview] = useState(null);
  useEffect(() => {
    if (!active) return;
    const want = { video, audio };
    for (const kind of ["video", "audio"]) {
      if (!want[kind] && tracks.current[kind]) {
        tracks.current[kind].stop();
        tracks.current[kind] = null;
      }
    }
    const live = (k) => tracks.current[k]?.readyState === "live";
    setPreview(live("video") ? tracks.current.video : null);
    const need = ["video", "audio"].filter((k) => want[k] && !live(k));
    if (!need.length) return;
    if (!navigator.mediaDevices?.getUserMedia) return onBlocked(need);
    let stale = false;
    openMedia(need)
      .then((got) => {
        if (stale) return got.forEach((t) => t.stop());
        for (const t of got) tracks.current[t.kind] = t;
        setPreview(live("video") ? tracks.current.video : null);
        const missing = need.filter((k) => !live(k));
        if (missing.length) onBlocked(missing);
      })
      .catch(() => !stale && onBlocked(need));
    return () => {
      stale = true;
    };
  }, [video, audio, active]); // eslint-disable-line react-hooks/exhaustive-deps
  return { tracks: tracks.current, preview };
}

export default function Join() {
  const invite = new URLSearchParams(window.location.search).get("t");
  const [join, setJoin] = useState(null);
  const [error, setError] = useState(invite ? null : "This link is missing its invite token. Open the exact link from your invitation email.");
  const [entered, setEntered] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [left, setLeft] = useState(false);
  // The interviewer ended the call (it closes the room): the link no longer works.
  const [ended, setEnded] = useState(false);
  const [connError, setConnError] = useState(null);
  const [video, setVideo] = useState(true);
  const [audio, setAudio] = useState(true);
  const [deviceNote, setDeviceNote] = useState(null);
  const fetchedAt = useRef(0);
  const media = useLocalMedia(video, audio, !entered, (kinds) => {
    if (kinds.includes("video")) setVideo(false);
    if (kinds.includes("audio")) setAudio(false);
    const what = kinds.length > 1 ? "camera or microphone" : kinds[0] === "video" ? "camera" : "microphone";
    setDeviceNote(`We couldn't turn on your ${what}. Allow access in your browser's site settings, then turn it on again. You can still join and hear the interviewer.`);
  });

  // Exchange the invite for a room token (again on "Try again", and when the
  // lobby sat open long enough for the token to go stale).
  const load = async () => {
    const r = await fetch(`${API}/meetings/join`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ token: invite }) });
    const body = await r.json().catch(() => ({}));
    if (!r.ok) throw new Error(body.error || `Could not join (HTTP ${r.status})`);
    fetchedAt.current = Date.now();
    setJoin(body);
    // Warm up the room connection while the person reads the lobby.
    loadRoom().then((m) => m.prepare(body.ws_url, body.token)).catch(() => {});
  };
  // In the room with no interviewer: the same call makes Combox send one.
  const resendAgent = useRef(() =>
    fetch(`${API}/meetings/join`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ token: invite }) }).catch(() => {}),
  ).current;
  const fail = (e) => setError(e instanceof TypeError ? "We couldn't reach the Combox server. Check your connection and try the link again." : e.message);

  useEffect(() => {
    if (invite) load().catch(fail);
    loadRoom().catch(() => {});
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const enter = async () => {
    if (Date.now() - fetchedAt.current > STALE_MS) {
      setRefreshing(true);
      try {
        await load();
      } catch (e) {
        return fail(e);
      } finally {
        setRefreshing(false);
      }
    }
    setEntered(true);
  };

  const restart = () => {
    setConnError(null);
    setEntered(false);
    setJoin(null);
    load().catch(fail);
  };

  if (error) return <Notice tone="error" title="Can't join this meeting" text={error} />;
  if (connError)
    return (
      <Notice tone="error" title="Couldn't connect to the room" text={connError}>
        <button className="btn btn-lime" style={{ marginTop: 28 }} onClick={restart}>Try again</button>
      </Notice>
    );
  if (ended) return <Notice title="The interview has ended" text="Thanks for your time. The hiring team will follow up with you. You can close this tab; this link no longer works." />;
  // Leaving ends the interview (the interviewer closes it), so there is no rejoin.
  if (left) return <Notice title="You left the interview" text="Thanks for your time. Leaving ended the interview, so this link no longer works. You can close this tab." />;
  if (!entered)
    return (
      <Lobby
        join={join}
        busy={!join || refreshing}
        preview={media.preview}
        note={deviceNote}
        video={video}
        audio={audio}
        setVideo={(on) => (setDeviceNote(null), setVideo(on))}
        setAudio={(on) => (setDeviceNote(null), setAudio(on))}
        onEnter={enter}
      />
    );

  return (
    <Suspense fallback={<Notice title="Joining…" text="Connecting you to the interview room." />}>
      <Room join={join} video={video} audio={audio} tracks={media.tracks} onLeft={() => setLeft(true)} onEnded={() => setEnded(true)} onFailed={setConnError} onNoAgent={resendAgent} />
    </Suspense>
  );
}

function Preview({ track }) {
  const ref = useRef(null);
  useEffect(() => {
    if (ref.current) ref.current.srcObject = track ? new MediaStream([track]) : null;
  }, [track]);
  return <div className="preview">{track ? <video ref={ref} autoPlay muted playsInline /> : <p className="muted">Camera off</p>}</div>;
}

function Lobby({ join, busy, preview, note, video, audio, setVideo, setAudio, onEnter }) {
  const minutes = join?.duration_minutes || 10;
  const expires = join?.expires_at ? new Date(join.expires_at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }) : null;
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
          <h2>{join ? <Kinetic text={join.title} delay={10} /> : <span className="muted">Checking your invitation…</span>}</h2>
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
          <Preview track={preview} />
          {note && <p className="lobby-note" role="alert">{note}</p>}
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
          <button className="btn btn-lime btn-lg btn-block" onClick={onEnter} disabled={busy}>
            {busy ? "Getting your room ready…" : <>Join the interview <ArrowRight size={18} color="currentColor" /></>}
          </button>
        </div>
      </section>
    </div>
  );
}
