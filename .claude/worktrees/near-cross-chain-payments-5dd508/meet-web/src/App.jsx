// Participant-facing surfaces, routed by path (under /t/<company>/ on the
// multi-tenant platform):
//   /jobs, /jobs/<id>       the public jobs board (ForeMan Jobs)
//   /briefs                 the freelancer portal (briefs + deliverables)
//   /join?t=<invite token>  the interview room (default for any other path)
import Jobs from "./Jobs.jsx";
import Join from "./Join.jsx";
import Portal from "./Portal.jsx";
import { PATH } from "./base.js";

const isJobs = PATH === "/jobs" || PATH.startsWith("/jobs/");
const isBriefs = PATH === "/briefs" || PATH.startsWith("/briefs/");
if (isJobs) document.title = "ForeMan Jobs";
else if (isBriefs) document.title = "ForeMan Briefs";

export default function App() {
  if (isJobs) return <Jobs />;
  if (isBriefs) return <Portal />;
  return <Join />;
}
