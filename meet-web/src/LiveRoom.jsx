// The interview room, loaded on its own (it carries LiveKit) while the
// person reads the lobby. `Room` connects and publishes the camera and mic
// opened in the lobby; `LiveRoom` is the call itself: participant tiles, live
// captions from the agent session's transcriptions (both the interviewer and
// the candidate), and controls.
//
// Weak networks: the voice comes first (the camera goes off on a poor
// connection), and a dropped connection is retried for two minutes, as long
// as the interviewer waits, with a notice saying so. The interview itself is
// voice only: someone who missed a line asks the interviewer to repeat it.
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { LiveKitRoom, RoomAudioRenderer, StartAudio, VideoTrack, isTrackReference, useIsMuted, useIsSpeaking, useParticipants, useRoomContext, useTrackToggle, useTracks, useTranscriptions } from "@livekit/components-react";
import { AudioPresets, ConnectionQuality, DefaultReconnectPolicy, DisconnectReason, MediaDeviceFailure, ParticipantKind, Room as LkRoom, RoomEvent, Track, VideoPresets } from "livekit-client";
import { Clock, Cpu, MicrophoneSlash1, Record } from "iconsax-react";
import { Alerts, CaptionBand, ControlBar, Wave, initials } from "./Captions.jsx";
import { Orb, Wordmark } from "./ui.jsx";

const AGENT_RESEND_MS = 35000;
// A dropped connection is retried for about two minutes: the interviewer
// waits that long in an empty room (LiveKit's own policy gives up after 45 s).
const RETRY_DELAYS_MS = [0, 300, 1200, 2700, 4800, ...Array(14).fill(7000)];
const isAgent = (p) => !!p && (p.kind === ParticipantKind.AGENT || /^agent/i.test(p.identity));
const nameOf = (p, fallback) => (isAgent(p) ? "Combox interviewer" : p?.isLocal ? "You" : p?.name || fallback || "Guest");
// Built for weak mobile networks: the voice comes first. Speech-quality Opus
// with DTX (near silence costs nothing) and RED (each packet carries the last
// one, so a lost packet isn't heard), sent at high priority; the camera is
// small, sent as one encoding, and gives way first.
const ROOM_OPTIONS = {
  reconnectPolicy: new DefaultReconnectPolicy(RETRY_DELAYS_MS),
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
// The interviewer is told on this data topic when the microphone sends
// complete silence (a dead or blocked device), so it names the problem
// instead of asking the person to "check the microphone" (agent MIC_TOPIC).
const MIC_TOPIC = "axom.mic";
// Sent when the person presses Leave (agent LEAVE_TOPIC).
const LEAVE_TOPIC = "axom.leave";
const SILENT_MIC = "We're not getting any sound from your microphone. Check that it's connected and selected in your device's sound settings, then turn it off and on below.";
const BLOCKED = "Your camera or microphone is blocked. Allow access in your browser's site settings, then turn it back on below. You can still hear the interviewer.";

// Set when the person presses Leave. A page that unloads (a reload, a closed
// tab) also disconnects "on purpose" to LiveKit, and must not be taken for
// leaving: a reload comes straight back in.
let leaving = false;

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
        <RoomTimer minutes={join.duration_minutes || 10} endsAt={join.expires_at} />
        <span className="ai-note"><Cpu size={15} variant="Bulk" color="currentColor" /> AI interviewer present · asks consent before recording</span>
        {/* The issues button (`Alerts`), rendered here from inside the call;
            the slot keeps its size so the bar never shifts. */}
        <span className="alerts-slot" id="room-alerts" />
      </header>
      <div className="room-stage">
        <LiveKitRoom
          room={room}
          serverUrl={join.ws_url}
          token={join.token}
          connect
          onConnected={publish}
          onDisconnected={(reason) => {
            if (reason === DisconnectReason.CLIENT_INITIATED) return leaving ? onLeft() : undefined;
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
          <LiveRoom deviceIssue={deviceIssue} onDeviceIssue={setDeviceIssue} onNoAgent={onNoAgent} />
        </LiveKitRoom>
      </div>
    </div>
  );
}

// Time left in the interview. The interviewer ends it at the limit; this
// counts down from when the candidate entered the room.
// Counts down to `endsAt` (the join link's end: the interview's time limit
// from the first join), so a reload or a rejoin shows the time really left
// rather than starting over; never more than the limit from entering.
function RoomTimer({ minutes, endsAt }) {
  const [end] = useState(() => {
    const limit = Date.now() + minutes * 60000;
    const t = Date.parse(endsAt);
    return Number.isFinite(t) ? Math.min(t, limit) : limit;
  });
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  const left = Math.max(0, Math.ceil((end - now) / 1000));
  const mm = String(Math.floor(left / 60)).padStart(2, "0");
  const ss = String(left % 60).padStart(2, "0");
  return (
    <span className={`timer${left <= 60 ? " timer-low" : ""}`} role="timer" aria-label={`${Math.ceil(left / 60)} minutes left`}>
      <Clock size={14} variant="Bold" color="currentColor" /> {mm}:{ss} left
    </span>
  );
}

function LiveRoom({ deviceIssue, onDeviceIssue, onNoAgent }) {
  const tracks = useTracks([{ source: Track.Source.Camera, withPlaceholder: true }], { onlySubscribed: false });
  const participants = useParticipants();
  const transcriptions = useTranscriptions();
  // Permission or hardware failures surface as a notice instead of a silent no-op.
  const deviceError = (what) => () => onDeviceIssue(`We couldn't turn on your ${what}. Allow access in your browser's site settings, or check that it's connected.`);
  const mic = useTrackToggle({ source: Track.Source.Microphone, onDeviceError: deviceError("microphone") });
  const cam = useTrackToggle({ source: Track.Source.Camera, onDeviceError: deviceError("camera") });
  const room = useRoomContext();
  const [cc, setCc] = useState(true);
  const [weak, setWeak] = useState(false);
  const [reconnecting, setReconnecting] = useState(false);
  // On a poor connection the camera is turned off, so the voice gets the
  // bandwidth; the person can turn it back on.
  useEffect(() => {
    const lp = room.localParticipant;
    const onQuality = (q, p) => {
      if (p !== lp) return;
      const poor = q === ConnectionQuality.Poor || q === ConnectionQuality.Lost;
      setWeak(poor);
      if (!poor || !lp.isCameraEnabled) return;
      lp.setCameraEnabled(false).catch(() => {});
      onDeviceIssue("Your connection is weak, so your camera was turned off to keep the conversation clear. Captions stay on.");
    };
    const down = () => setReconnecting(true);
    const up = () => setReconnecting(false);
    room.on(RoomEvent.ConnectionQualityChanged, onQuality);
    room.on(RoomEvent.Reconnecting, down);
    room.on(RoomEvent.SignalReconnecting, down);
    room.on(RoomEvent.Reconnected, up);
    return () => {
      room.off(RoomEvent.ConnectionQualityChanged, onQuality);
      room.off(RoomEvent.Reconnecting, down);
      room.off(RoomEvent.SignalReconnecting, down);
      room.off(RoomEvent.Reconnected, up);
    };
  }, [room, onDeviceIssue]);
  // Until the interviewer has said something, show what it is doing, so a
  // quiet first few seconds don't look like a broken call.
  const agent = participants.find(isAgent);
  // LiveKit checks the microphone once it starts: complete silence means a
  // dead or blocked device. Show it and tell the interviewer (again if it
  // joins later); being heard clears it.
  const [micSilent, setMicSilent] = useState(false);
  const localSpeaking = useIsSpeaking(room.localParticipant);
  useEffect(() => {
    const onSilent = () => setMicSilent(true);
    room.on(RoomEvent.LocalAudioSilenceDetected, onSilent);
    return () => room.off(RoomEvent.LocalAudioSilenceDetected, onSilent);
  }, [room]);
  useEffect(() => {
    if (localSpeaking) setMicSilent(false);
  }, [localSpeaking]);
  useEffect(() => {
    if (!micSilent || !agent) return;
    room.localParticipant.publishData(new TextEncoder().encode("silent"), { reliable: true, topic: MIC_TOPIC }).catch(() => {});
  }, [micSilent, agent, room]);
  // Problems the person may need to fix, behind one caution button (a
  // muted microphone is not one: the button and their tile show it).
  const issues = [
    deviceIssue && { id: "device", text: deviceIssue, dismiss: () => onDeviceIssue(null) },
    micSilent && { id: "mic", text: SILENT_MIC },
    weak && !reconnecting && { id: "weak", text: "Your connection is weak. If you miss something, ask the interviewer to repeat it." },
  ].filter(Boolean);
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

  // One caption line per spoken segment. What a person says is sent again,
  // whole, each time more of it is recognized, as a new stream with the same
  // segment id: the line keeps that id, so it grows in place.
  const lines = transcriptions.map((t) => {
    const p = participants.find((x) => x.identity === t.participantInfo.identity);
    return {
      id: t.streamInfo.attributes?.["lk.segment_id"] || t.streamInfo.id,
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
        {reconnecting && <p className="ai-status net-status"><span className="ai-status-dot" />Reconnecting. Stay on this page: the interviewer is waiting for you.</p>}
        {/* Shown only when the browser blocked sound until a click. */}
        <StartAudio label="Tap to hear the interviewer" className="start-audio" />
      </div>
      <AlertsInBar issues={issues} />
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
        onLeave={async () => {
          // Leaving ends the interview for good, so ask first.
          if (!window.confirm("Leave and end the interview? You won't be able to rejoin.")) return;
          leaving = true;
          // Tell the interviewer it is on purpose: any other disconnect (a
          // reload, a lost connection) is waited out. A moment for it to go
          // out; if it is lost, the interviewer ends the call after its wait.
          try {
            await room.localParticipant.publishData(new TextEncoder().encode("leave"), { reliable: true, topic: LEAVE_TOPIC });
            await new Promise((r) => setTimeout(r, 300));
          } catch {
            // Disconnect anyway.
          }
          room.disconnect();
        }}
      />
      <RoomAudioRenderer />
    </div>
  );
}

// The issues button lives in the top bar, outside the LiveKit room's tree.
function AlertsInBar({ issues }) {
  const [slot, setSlot] = useState(null);
  useEffect(() => setSlot(document.getElementById("room-alerts")), []);
  return slot ? createPortal(<Alerts issues={issues} />, slot) : null;
}

function Tile({ trackRef }) {
  const p = trackRef.participant;
  const speaking = useIsSpeaking(p);
  const micOff = useIsMuted({ participant: p, source: Track.Source.Microphone });
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
        {!ai && micOff ? (
          <span className="mic-off" role="img" aria-label="Microphone off" title="Microphone off">
            <MicrophoneSlash1 size={15} variant="Bold" color="currentColor" />
          </span>
        ) : (
          <Wave on={speaking} />
        )}
      </span>
    </div>
  );
}
