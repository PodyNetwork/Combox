// A one-choice dropdown that looks like the date picker's field, instead of
// the browser's own <select> (which draws the system control and menu). The
// list floats under the field with fixed positioning (above it when there is
// no room), so a panel's rounded, clipped edges never cut it off.
// Keyboard: arrows or Home/End to move, Enter or Space to pick, Escape to close.
import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { Icon } from "./ui.jsx";

const GAP = 6;
const EDGE = 8;

/** options: [{ value, label }]. `small`: a compact pill with the label for screen readers only. */
export default function Dropdown({ label, value, onChange, options, small = false }) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const [pos, setPos] = useState(null);
  const ref = useRef(null);
  const fieldRef = useRef(null);
  const listRef = useRef(null);
  const current = options.find((o) => o.value === value) ?? options[0];

  const place = () => {
    const field = fieldRef.current?.getBoundingClientRect();
    const list = listRef.current;
    if (!field || !list) return;
    const h = list.offsetHeight;
    const below = window.innerHeight - field.bottom - GAP - EDGE;
    const top = below >= h || field.top - GAP - EDGE < h ? field.bottom + GAP : field.top - GAP - h;
    const width = Math.max(field.width, small ? 96 : 180);
    const left = Math.max(EDGE, Math.min(field.left, window.innerWidth - width - EDGE));
    setPos({ top, left, width });
  };

  useLayoutEffect(() => {
    if (!open) return setPos(null);
    place();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const away = (e) => !ref.current?.contains(e.target) && setOpen(false);
    document.addEventListener("pointerdown", away);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      document.removeEventListener("pointerdown", away);
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open]);

  const show = () => {
    setActive(Math.max(0, options.indexOf(current)));
    setOpen(true);
  };
  const pick = (o) => {
    onChange(o.value);
    setOpen(false);
    fieldRef.current?.focus();
  };
  const key = (e) => {
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
        e.preventDefault();
        show();
      }
      return;
    }
    const last = options.length - 1;
    const moves = { ArrowDown: Math.min(last, active + 1), ArrowUp: Math.max(0, active - 1), Home: 0, End: last };
    if (e.key in moves) {
      e.preventDefault();
      setActive(moves[e.key]);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      pick(options[active]);
    } else if (e.key === "Escape") {
      // Close the list only, not a dialog around it.
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
    } else if (e.key === "Tab") {
      setOpen(false);
    }
  };

  return (
    <div ref={ref} className={small ? "inline-block text-xs" : "block min-w-0 text-sm"}>
      <span id={`${id}-label`} className={small ? "sr-only" : "text-mute"}>{label}</span>
      <button
        ref={fieldRef}
        type="button"
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={`${id}-list`}
        aria-labelledby={`${id}-label ${id}-value`}
        aria-activedescendant={open ? `${id}-${active}` : undefined}
        onClick={() => (open ? setOpen(false) : show())}
        onKeyDown={key}
        className={`flex items-center border bg-black/40 text-left transition ${small ? "gap-1.5 rounded-full px-3 py-1.5" : "mt-1.5 w-full gap-2.5 rounded-xl px-3.5 py-2.5"} focus:outline-none focus-visible:border-lime/60 focus-visible:ring-4 focus-visible:ring-lime/10 ${open ? "border-lime/60 ring-4 ring-lime/10" : "border-white/10 hover:border-white/20"}`}
      >
        <span id={`${id}-value`} className={`min-w-0 flex-1 truncate ${value ? "text-fog" : "text-mute"}`}>{current?.label}</span>
        <Icon name="chevronRight" size={14} className={`shrink-0 text-dim transition-transform ${open ? "-rotate-90" : "rotate-90"}`} />
      </button>
      {open && (
        <ul
          ref={listRef}
          id={`${id}-list`}
          role="listbox"
          aria-labelledby={`${id}-label`}
          style={pos ? { top: pos.top, left: pos.left, width: pos.width } : { visibility: "hidden" }}
          className="pop fixed z-50 max-h-72 overflow-y-auto rounded-xl border border-white/10 bg-raised p-1 shadow-[0_16px_48px_-12px_rgb(0_0_0/0.75)]"
        >
          {options.map((o, i) => {
            const chosen = o.value === value;
            return (
              <li
                key={o.value}
                id={`${id}-${i}`}
                role="option"
                aria-selected={chosen}
                onPointerEnter={() => setActive(i)}
                onClick={() => pick(o)}
                className={`flex cursor-pointer items-center gap-2 rounded-lg px-3 py-2 text-sm ${i === active ? "bg-white/8 text-fog" : "text-mute"}`}
              >
                <span className="min-w-0 flex-1 truncate">{o.label}</span>
                {chosen && <Icon name="check" size={14} className="shrink-0 text-lime" />}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
