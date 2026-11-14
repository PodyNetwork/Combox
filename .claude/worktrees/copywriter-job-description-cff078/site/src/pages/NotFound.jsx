import { ArrowRight } from "lucide-react";
import { Orb } from "../components/ui.jsx";

export default function NotFound() {
  return (
    <section className="hero" style={{ textAlign: "center" }}>
      <div className="wrap">
        <Orb className="halted" style={{ width: "min(240px, 50vw)", margin: "0 auto 40px" }} />
        <span className="eyebrow">404 · not in the org chart</span>
        <h1 className="h-l mt-m">This page doesn't exist.</h1>
        <p className="lead mt-m" style={{ margin: "24px auto 0" }}>The agents looked in every department. Try one of these instead.</p>
        <div className="hero-actions" style={{ justifyContent: "center" }}>
          <a className="btn btn-lime btn-lg" href="/">Home <ArrowRight className="arrow" aria-hidden="true" /></a>
          <a className="btn btn-ghost btn-lg" href="/how-it-works/">How it works</a>
          <a className="btn btn-ghost btn-lg" href="/get-started/">Get started</a>
        </div>
      </div>
    </section>
  );
}
