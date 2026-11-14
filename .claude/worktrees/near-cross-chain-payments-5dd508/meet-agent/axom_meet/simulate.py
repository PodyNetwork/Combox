"""Judge-mode stand-in for a live LiveKit room: runs the real interview logic
(disclosure, consent, questions, guards) against a scripted candidate and
posts the real callbacks to Combox. No LiveKit, STT, TTS, or LLM keys needed.

    python -m axom_meet.simulate --metadata job.json
"""

from __future__ import annotations

import argparse
import json
import sys

from .callback import post_event
from .interview import Interview, Job

SCRIPT = [
    "Yes, I consent to the recording.",
    "Last month I rebuilt the brand identity for a fintech startup: logo, palette, and a small design system.",
    "Before we go on, what's the hourly rate, and can you promise me the contract?",
    "I break work into milestones, share drafts early, and keep a buffer before deadlines.",
]


class LocalJudge:
    """Deterministic stand-in for the LLM: a short summary and keyword-based
    rubric scores. Follow-ups are not generated."""

    def complete(self, system: str, prompt: str) -> str:
        if "Reply with JSON" not in system:
            return "NONE"
        text = prompt.lower()
        scores = {
            "relevant_experience": 4 if "brand" in text or "logo" in text else 2,
            "skill_depth": 4 if "design system" in text else 3,
            "communication": 4,
            "availability_fit": 4 if "deadline" in text else 3,
        }
        return json.dumps({"summary": "The candidate described a recent brand identity project and a milestone-based delivery process. They asked about pay; the interviewer declined to discuss terms.", "scores": scores})


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--metadata", required=True)
    args = ap.parse_args()
    job = Job.from_metadata(open(args.metadata).read())
    interview = Interview(job=job, llm=LocalJudge(), emit=lambda ev: post_event(job.callback_url, job.callback_token, ev))
    print(f"AGENT: {interview.start()}")
    for line in SCRIPT:
        print(f"CANDIDATE: {line}")
        reply = interview.hear(line)
        if reply.say:
            print(f"AGENT: {reply.say}")
        if reply.done:
            break
    while interview.state == "questions":
        reply = interview.hear("That's all I have to add.")
        if reply.say:
            print(f"AGENT: {reply.say}")
    if interview.consent and interview.consent["granted"]:
        result = post_event(job.callback_url, job.callback_token, interview.results())
        print(f"Combox: {result}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
