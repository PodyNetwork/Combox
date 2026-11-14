// The real room: participant tiles, live captions from the agent session's
// transcriptions (both the interviewer and the candidate), and controls.
// Rendered inside <LiveKitRoom>.
import { useState } from "react";
import { RoomAudioRenderer, VideoTrack, isTrackReference, useIsSpeaking, useParticipants, useRoomContext, useTrackToggle, useTracks, useTranscriptions } from "@livekit/components-react";
import { ParticipantKind, Track } from "livekit-client";
import { Cpu } from "iconsax-react";
import { CaptionBand, ControlBar, Wave, initials } from "./Captions.jsx";
import { Orb } from "./ui.jsx";

const isAgent = (p) => !!p && (p.kind === ParticipantKind.AGENT || /^agent/i.test(p.identity));
const nameOf = (p, fallback) => (isAgent(p) ? "ForeMan interviewer" : p?.isLocal ? "You" : p?.name || fallback || "Guest");

export default function LiveRoom({ onDeviceIssue = () => {} }) {
  const tracks = useTracks([{ source: Track.Source.Camera, withPlaceholder: true }], { onlySubscribed: false });
  const participants = useParticipants();
  const transcriptions = useTranscriptions();
  // Permission or hardware failures surface as a notice instead of a silent no-op.
  const deviceError = (what) => () => onDeviceIssue(`We couldn't turn on your ${what}. Allow access in your browser's site settings, or check that it's connected.`);
  const mic = useTrackToggle({ source: Track.Source.Microphone, onDeviceError: deviceError("microphone") });
  const cam = useTrackToggle({ source: Track.Source.Camera, onDeviceError: deviceError("camera") });
  const room = useRoomContext();
  const [cc, setCc] = useState(true);

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

  return (
    <div className="stage">
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
        onLeave={() => room.disconnect()}
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
