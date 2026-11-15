// A QR code for a deposit address, drawn as one SVG path in the browser
// (qrcode-generator, pinned). Dark modules on white with a quiet zone, the
// contrast phone cameras need, whatever the dashboard's theme.
import { useMemo } from "react";
import qrcode from "qrcode-generator";

export function Qr({ value, size = 184, label, className = "" }) {
  const { d, n } = useMemo(() => {
    const qr = qrcode(0, "M");
    qr.addData(value);
    qr.make();
    const count = qr.getModuleCount();
    let path = "";
    for (let r = 0; r < count; r++) for (let c = 0; c < count; c++) if (qr.isDark(r, c)) path += `M${c + 4} ${r + 4}h1v1h-1z`;
    return { d: path, n: count + 8 };
  }, [value]);
  return (
    <svg
      viewBox={`0 0 ${n} ${n}`}
      width={size}
      height={size}
      role="img"
      aria-label={label || `QR code for ${value}`}
      shapeRendering="crispEdges"
      className={`block rounded-2xl bg-white ${className}`}
    >
      <path d={d} fill="#0a0a0b" />
    </svg>
  );
}
