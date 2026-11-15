// The interview room, loaded on its own (it carries LiveKit) while the
// person reads the lobby. `Room` connects and publishes the camera and mic
// opened in the lobby; `LiveRoom` is the call itself: participant tiles, live
// captions from the agent session's transcriptions (both the interviewer and
// the candidate), and controls.
import { useEffect, useRef, useState } from "react";
import { LiveKitRoom, RoomAudioRenderer, StartAudio, VideoTrack, isTrackReference, useIsSpeaking, useParticipants, useRoomContext, useTrackToggle, useTracks, useTranscriptions } from "@livekit/components-react";
import { AudioPresets, ConnectionQuality, DisconnectReason, MediaDeviceFailure, ParticipantKind, Room as LkRoom, RoomEvent, Track, VideoPresets } from "livekit-client";
import { Clock, Cpu, Record } from "iconsax-react";
import { CaptionBand, ControlBar, Wave, initials } from "./Captions.jsx";
import { Orb, Wordmark } from "./ui.jsx";

const AGENT_RESEND_MS = 35000;
const isAgent = (p) => !!p && (p.kind === ParticipantKind.AGENT || /^agent/i.test(p.identity));
const nameOf = (p, fallback) => (isAgent(p) ? "Combox interviewer" : p?.isLocal ? "You" : p?.name || fallback || "Guest");
// Built for weak mobile networks: the voice comes first. Speech-quality Opus
// with DTX (near silence costs nothing) and RED (each packet carries the last
// one, so a lost packet isn't heard), sent at high priority; the camera is
// small, sent as one encoding, and gives way first.
const ROOM_OPTIONS = {
  adaptiveStream: true,
  dynacast: true,
  audioCaptureDefaults: { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1 },
  videoCaptureDefaults: { resolution: VideoPresets.h360.resolution },
  publishDefaults: {
    audioPreset: { ...AudioPresets.speech, priority: "high" },
    dtx: true,
    red: true,
    videoEncoding: { ...VideoPresets.h360.encoding, priority: "low" },
    // One camera encoding: extra layers only help many viewers on different
    // networks, and each one costs the person's computer CPU during the call.
    simulcast: false,
    degradationPreference: "maintain-framerate",
  },
};
const BLOCKED = "Your camera or microphone is blocked. Allow access in your browser's site settings, then turn it back on below. You can still hear the interviewer.";

// Warm up the connection to the room server while the person is in the lobby.
let prepared = null;
export function prepare(url, token) {
  prepared = new LkRoom(ROOM_OPTIONS);
  prepared.prepareConnection(url, token);
}

// tracks: { video, audio } MediaStreamTracks opened in the lobby (or null).
export default function Room({ join, video, audio, tracks, onLeft, onEnded, onFailed, onNoAgent }) {
  const [room] = useState(() => {
    const r = prepared || new LkRoom(ROOM_OPTIONS);
    prepared = null;
    return r;
  });
  const connected = useRef(false);
  const [deviceIssue, setDeviceIssue] = useState(null);

  // Publish the lobby's camera and mic the moment the room connects: no
  // second permission prompt, and turning them off and on again is instant.
  // Anything the lobby could not open is tried once more here.
  const publish = async () => {
    connected.current = true;
    const lp = room.localParticipant;
    const sends = [
      [tracks.video, video, Track.Source.Camera, () => lp.setCameraEnabled(true)],
      [tracks.audio, audio, Track.Source.Microphone, () => lp.setMicrophoneEnabled(true)],
    ].map(([track, on, source, open]) => (track?.readyState === "live" ? lp.publishTrack(track, { source }) : on ? open() : null));
    const results = await Promise.allSettled(sends);
    if (results.some((r) => r.status === "rejected")) setDeviceIssue(BLOCKED);
  };

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
          room={room}
          serverUrl={join.ws_url}
          token={join.token}
          connect
          onConnected={publish}
          onDisconnected={(reason) => {
            if (reason === DisconnectReason.CLIENT_INITIATED) return onLeft();
            if (!connected.current) return onFailed("The interview room didn't answer. It may not have started yet, or your network is blocking it.");
            if (reason === DisconnectReason.DUPLICATE_IDENTITY) return onFailed("You joined this interview from another tab or device, so this one was disconnected.");
            if (reason === DisconnectReason.ROOM_DELETED) return onEnded();
            onFailed("You were disconnected from the interview. Check your connection, then rejoin.");
          }}
          onError={(e) => {
            // A blocked or missing camera/microphone is not a connection
            // failure: stay in the call (the interviewer can still be heard)
            // and say how to fix it. Leaving the room here would end the interview.
            if (MediaDeviceFailure.getFailure(e) || /permission|notallowed|notfound|device/i.test(`${e?.name} ${e?.message}`)) return setDeviceIssue(BLOCKED);
            onFailed(e?.message ? `The room connection failed: ${e.message}` : "The room connection failed.");
          }}
          onMediaDeviceFailure={() => setDeviceIssue(BLOCKED)}
        >
          <LiveRoom onDeviceIssue={setDeviceIssue} onNoAgent={onNoAgent} />
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

function LiveRoom({ onDeviceIssue, onNoAgent }) {
  const tracks = useTracks([{ source: Track.Source.Camera, withPlaceholder: true }], { onlySubscribed: false });
  const participants = useParticipants();
  const transcriptions = useTranscriptions();
  // Permission or hardware failures surface as a notice instead of a silent no-op.
  const deviceError = (what) => () => onDeviceIssue(`We couldn't turn on your ${what}. Allow access in your browser's site settings, or check that it's connected.`);
  const mic = useTrackToggle({ source: Track.Source.Microphone, onDeviceError: deviceError("microphone") });
  const cam = useTrackToggle({ source: Track.Source.Camera, onDeviceError: deviceError("camera") });
  const room = useRoomContext();
  const [cc, setCc] = useState(true);
  // On a poor connection the camera is turned off, so the voice gets the
  // bandwidth; the person can turn it back on.
  useEffect(() => {
    const lp = room.localParticipant;
    const onQuality = (q, p) => {
      if (p !== lp || (q !== ConnectionQuality.Poor && q !== ConnectionQuality.Lost) || !lp.isCameraEnabled) return;
      lp.setCameraEnabled(false).catch(() => {});
      onDeviceIssue("Your connection is weak, so your camera was turned off to keep the conversation clear. Captions stay on.");
    };
    room.on(RoomEvent.ConnectionQualityChanged, onQuality);
    return () => room.off(RoomEvent.ConnectionQualityChanged, onQuality);
  }, [room, onDeviceIssue]);
  // Until the interviewer has said something, show what it is doing, so a
  // quiet first few seconds don't look like a broken call.
  const agent = participants.find(isAgent);
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    if (agent) return setSlow(false);
    const t = setTimeout(() => setSlow(true), 20000);
    return () => clearTimeout(t);
  }, [agent]);
  // LiveKit drops a send when no interviewer is running at that moment (it
  // was restarting, say) and never retries. While none is here, ask Combox
  // again: it sends a new one if the last send is over 30 s old and absent.
  useEffect(() => {
    if (agent || !onNoAgent) return;
    const t = setInterval(onNoAgent, AGENT_RESEND_MS);
    return () => clearInterval(t);
  }, [agent, onNoAgent]);

  const lines = transcriptions.map((t) => {
    const p = participants.find((x) => x.identity === t.participantInfo.identity);
    return {
      id: t.streamInfo.id,
      name: nameOf(p, t.participantInfo.identity),
      ai: isAgent(p),
      text: t.text,
      final: t.streamInfo.attributes?.["lk.transcription_final"] === "true",
    };
  });

  const aiSpoke = lines.some((l) => l.ai);
  const status = aiSpoke
    ? null
    : !agent
      ? slow
        ? "The AI interviewer is taking longer than usual to join. Please stay on this page."
        : "The AI interviewer is joining. It will greet you in a few seconds."
      : "The AI interviewer is here and about to speak.";

  return (
    <div className="stage">
      <div className="stage-top" aria-live="polite">
        {status && <p className="ai-status"><span className="ai-status-dot" />{status}</p>}
        {/* Shown only when the browser blocked sound until a click. */}
        <StartAudio label="Tap to hear the interviewer" className="start-audio" />
      </div>
      <div className="stage-grid">
        {tracks.map((t) => (
          <Tile key={`${t.participant.identity}-${t.source}`} trackRef={t} />
        ))}
      </div>
      {cc && <CaptionBand lines={lines} />}
      <ControlBar
        mic={mic.enabled}
        cam={cam.enabled}
        cc={cc}
        onMic={() => mic.toggle().catch(deviceError("microphone"))}
        onCam={() => cam.toggle().catch(deviceError("camera"))}
        onCc={() => setCc(!cc)}
        onLeave={() => {
          // Leaving ends the interview for good, so ask first.
          if (window.confirm("Leave and end the interview? You won't be able to rejoin.")) room.disconnect();
        }}
      />
      <RoomAudioRenderer />
    </div>
  );
}

function Tile({ trackRef }) {
  const p = trackRef.participant;
  const speaking = useIsSpeaking(p);
  const ai = isAgent(p);
  const name = nameOf(p);
  const video = isTrackReference(trackRef) && trackRef.publication && !trackRef.publication.isMuted;
  return (
    <div className={`tile ${speaking ? "speaking" : ""}`}>
      {video ? (
        <VideoTrack trackRef={trackRef} className="tile-video" />
      ) : ai ? (
        <Orb className={speaking ? "speaking" : ""} />
      ) : (
        <div className="avatar">{initials(name)}</div>
      )}
      <span className="who">
        {ai && <Cpu size={15} variant="Bulk" color="var(--violet)" />}
        {name}
        <Wave on={speaking} />
      </span>
    </div>
  );
}
