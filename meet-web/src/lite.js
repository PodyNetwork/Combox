// Low-end phones and laptops (4 GB of memory or less, 4 cores or fewer, or
// data saver on) get the pages without looping animations (`html.lite` in
// style.css): on such a device they take CPU the call needs. `?lite=1` or
// `?lite=0` in the address forces it on or off.
export function lowEnd() {
  const forced = new URLSearchParams(window.location.search).get("lite");
  if (forced === "0" || forced === "1") return forced === "1";
  const n = navigator;
  return (n.deviceMemory > 0 && n.deviceMemory <= 4) || (n.hardwareConcurrency > 0 && n.hardwareConcurrency <= 4) || !!n.connection?.saveData;
}

document.documentElement.classList.toggle("lite", lowEnd());
