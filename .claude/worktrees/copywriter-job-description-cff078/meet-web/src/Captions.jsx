// Meet-style live captions under the participant tiles, plus the call
// control bar, used in the LiveKit room.
import { CallSlash, Cpu, Microphone2, MicrophoneSlash1, Subtitle, Video, VideoSlash } from "iconsax-react";
import { Orb } from "./ui.jsx";

export const initials = (name = "?") =>
  name.split(/\s+/).map((p) => p[0]).filter(Boolean).slice(0, 2).join("").toUpperCase() || "?";

// Three bars that bounce while someone is talking.
export function Wave({ on }) {
  return (
    <span className={`wave ${on ? "on" : ""}`} aria-hidden="true">
      <i />
      <i />
      <i />
    </span>
  );
}

function Speaker({ line }) {
  if (line.ai) return <Orb className="cap-orb" />;
  return <span className="cap-avatar">{initials(line.name)}</span>;
}

// lines: [{ id, name, ai, text, final }]. The newest line is bright and
// streams in word by word; the one before it stays above, dimmed.
export function CaptionBand({ lines }) {
  const shown = lines.filter((l) => l.text?.trim()).slice(-2);
  return (
    <section className="captions" aria-label="Live captions">
      <p className="cap-label">
        <Subtitle size={13} variant="Bold" color="currentColor" /> Live captions · English
      </p>
      <div className="cap-lines" aria-live="polite">
        {shown.length === 0 && <p className="cap-empty">Captions appear here as people speak.</p>}
        {shown.map((l, i) => {
          const latest = i === shown.length - 1;
          return (
            <div key={l.id} className={`cap-line ${latest ? "" : "old"} ${l.ai ? "ai" : ""}`}>
              <Speaker line={l} />
              <div className="cap-body">
                <span className="cap-name">
                  {l.name}
                  {l.ai && <Cpu size={12} variant="Bulk" color="currentColor" />}
                </span>
                <p className="cap-text">
                  {l.text.split(/(\s+)/).map((w, wi) => (
                    <span key={wi} className={latest ? "w" : undefined}>{w}</span>
                  ))}
                  {latest && !l.final && <span className="caret" aria-hidden="true" />}
                </p>
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}

export function ControlBar({ mic, cam, cc, onMic, onCam, onCc, onLeave }) {
  return (
    <div className="mock-bar" role="toolbar" aria-label="Call controls">
      <button className={`round ${mic ? "" : "off"}`} aria-pressed={!mic} aria-label={mic ? "Mute microphone" : "Unmute microphone"} title={mic ? "Mute" : "Unmute"} onClick={onMic}>
        {mic ? <Microphone2 size={22} variant="Bold" color="currentColor" /> : <MicrophoneSlash1 size={22} variant="Bold" color="currentColor" />}
      </button>
      <button className={`round ${cam ? "" : "off"}`} aria-pressed={!cam} aria-label={cam ? "Turn camera off" : "Turn camera on"} title={cam ? "Stop video" : "Start video"} onClick={onCam}>
        {cam ? <Video size={22} variant="Bold" color="currentColor" /> : <VideoSlash size={22} variant="Bold" color="currentColor" />}
      </button>
      <button className={`round ${cc ? "on" : ""}`} aria-pressed={cc} aria-label={cc ? "Turn captions off" : "Turn captions on"} title="Captions" onClick={onCc}>
        <Subtitle size={22} variant={cc ? "Bold" : "Linear"} color="currentColor" />
      </button>
      <button className="round leave" onClick={onLeave}>
        <CallSlash size={20} variant="Bold" color="currentColor" /> Leave
      </button>
    </div>
  );
}
