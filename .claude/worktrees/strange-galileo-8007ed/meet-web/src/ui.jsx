// API base, including the /t/<company> prefix on the multi-tenant platform.
export { API } from "./base.js";

export function Orb({ className = "" }) {
  return (
    <div className={`orb ${className}`} aria-hidden="true">
      <div className="orb-halo" />
      <div className="orb-ring" />
      <div className="orb-core" />
    </div>
  );
}

// Giant word with the orb punching through it (solid behind, outline in front).
export function Breakthrough({ word }) {
  // Fit the word to the container width: roughly 0.82em per letter.
  const size = `${Math.min(27, 96 / (word.length * 0.82)).toFixed(1)}cqw`;
  return (
    <div className="breakthrough" aria-hidden="true" style={{ "--bt-size": size }}>
      <p className="bt-word">{word}</p>
      <Orb />
      <p className="bt-word bt-front">{word}</p>
    </div>
  );
}

export function Wordmark() {
  return (
    <span className="wordmark">
      <svg className="mark" viewBox="0 0 32 32" aria-hidden="true">
        <circle cx="16" cy="16" r="15" fill="#d4ff3a" />
        <path d="M8.5 18.5a7.5 7.5 0 0 1 15 0z" fill="#07070a" />
        <rect x="5.5" y="18.5" width="21" height="3" rx="1.5" fill="#07070a" />
        <rect x="14.6" y="10.2" width="2.8" height="6.4" rx="1.4" fill="#d4ff3a" />
      </svg>
      ForeMan
    </span>
  );
}

export function Notice({ title, text, tone, children }) {
  return (
    <main className="notice">
      <div>
        <Orb className={tone === "error" ? "orb-error" : ""} />
        <h1>{title}</h1>
        <p>{text}</p>
        {children}
      </div>
    </main>
  );
}

export const human = (s) => String(s ?? "").replaceAll("_", " ");

// Letters rise into place once.
export function Kinetic({ text, delay = 0 }) {
  let n = delay;
  return (
    <span className="kinetic" aria-label={text}>
      {String(text).split(" ").map((w, wi, all) => (
        <span key={wi} className="kinetic-word" aria-hidden="true">
          {[...w].map((ch, ci) => <span key={ci} className="kinetic-ch" style={{ "--c": n++ }}>{ch}</span>)}
          {wi < all.length - 1 && <span className="kinetic-ch">&nbsp;</span>}
        </span>
      ))}
    </span>
  );
}
