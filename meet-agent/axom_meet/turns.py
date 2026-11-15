"""When a participant's answer has ended.

Silero (local VAD) hears when the person stops making sound. On its own it
handed every pause of 0.7 s over as a finished answer, so a person who
paused to think ("...for a coffee company, and [pause] I handled the edit")
was cut off: the interviewer said "Thank you" or the next question over them,
and the rest of the answer was taken as the answer to the next question.

Two cheap checks fix that without slowing replies to finished answers:

- `AnswerEndDetector` reads the words heard so far. Deepgram punctuates a
  sentence only when it ends, so no closing punctuation, or a last word that
  leads somewhere ("and", "because", "um"), means the person is mid-thought:
  the agent then waits up to `MAX_WAIT_SECS` instead of `MIN_WAIT_SECS`. No
  model, no memory, no network: a few string checks.
- `TURN_HANDLING` uses LiveKit's dynamic endpointing, which learns how long
  this person pauses between sentences (and from any time they talk over a
  reply that came too soon) and waits that long, between the two limits.

What still slips through (a long pause after a full sentence) is caught after
the fact: see `Interviewer` in `agent.py`, which merges an answer the person
carries on with into one.
"""

from __future__ import annotations

import re

# The wait after the person stops when the answer sounds finished (the reply
# speed measured in DECISIONS PX-10), and the longest wait when it doesn't.
MIN_WAIT_SECS = 0.7
MAX_WAIT_SECS = 2.5

TURN_HANDLING = {
    "endpointing": {"mode": "dynamic", "min_delay": MIN_WAIT_SECS, "max_delay": MAX_WAIT_SECS},
}

# Last words that (almost) never end a sentence, so more is coming even when
# Deepgram put a full stop after them: joining words, articles, possessives
# and fillers (Deepgram transcribes "um", "uh"). Words that often do end one
# ("so", "to", "it", "do": "I think so.", "Yes, I do.") are left out: a
# needless long wait after a finished answer is the cost of a wrong entry.
LEADS_ON = frozenset("and but or nor because cause although whereas unless the a an my our your their um uh er erm hmm uhm".split())
_ENDS_SENTENCE = re.compile(r"[.?!…][\"')\]]*$")
_LAST_WORD = re.compile(r"([\w']+)\W*$")


def sounds_finished(text: str) -> bool:
    """True when what was heard so far reads as a finished thought."""
    text = text.strip()
    if not text or not _ENDS_SENTENCE.search(text):
        return False
    m = _LAST_WORD.search(text)
    return not (m and m.group(1).lower() in LEADS_ON)


class AnswerEndDetector:
    """A LiveKit turn detector (the protocol of `livekit.agents.voice.turn`)
    that answers from the words alone: "likely over" (the short wait) or
    "unlikely" (the long one)."""

    model = "axom-answer-end"
    provider = "axom"

    async def supports_language(self, language) -> bool:
        return True

    async def unlikely_threshold(self, language) -> float:
        return 0.5

    async def predict_end_of_turn(self, chat_ctx, *, timeout: float | None = None) -> float:
        last = next((m for m in reversed(chat_ctx.items) if getattr(m, "role", None) == "user"), None)
        return 0.95 if last is not None and sounds_finished(last.text_content or "") else 0.05
