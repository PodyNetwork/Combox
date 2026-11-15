// Participant-facing surfaces, routed by path (under /t/<company>/ on the
// multi-tenant platform):
//   /jobs, /jobs/<id>       the public jobs board (Combox Jobs)
//   /briefs                 the freelancer portal (briefs + deliverables)
//   /join?t=<invite token>  the interview room (default for any other path)
// Each surface loads only its own code: the interview room (LiveKit) is
// further split off inside Join.
import { Suspense, lazy } from "react";
import Join from "./Join.jsx";
import { PATH } from "./base.js";

const Jobs = lazy(() => import("./Jobs.jsx"));
const Portal = lazy(() => import("./Portal.jsx"));

const isJobs = PATH === "/jobs" || PATH.startsWith("/jobs/");
const isBriefs = PATH === "/briefs" || PATH.startsWith("/briefs/");
if (isJobs) document.title = "Combox Jobs";
else if (isBriefs) document.title = "Combox Briefs";

export default function App() {
  if (isJobs || isBriefs) return <Suspense fallback={null}>{isJobs ? <Jobs /> : <Portal />}</Suspense>;
  return <Join />;
}
