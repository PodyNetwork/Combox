"""Lines spoken at once: the opening line (the AI disclosure) is the same for
every interview, so it is synthesized while the agent waits for the
participant (`presynthesize`) and played the moment they enter, with no round
trip to the speech provider; the fixed lines and questions likewise.

The agent's voice uses LiveKit's own 200 ms send buffer. A 3 s buffer was
tried (DECISIONS PX-09) and removed (PX-12): whenever the participant made a
short sound (a cough, "mm"), LiveKit paused the interviewer and threw the
buffered speech away, so up to 3 s of its words were skipped mid-sentence.
"""

from __future__ import annotations

import asyncio
import logging
from collections.abc import AsyncIterator

from livekit import rtc

log = logging.getLogger("axom-meet")


async def presynthesize(tts, text: str) -> list[rtc.AudioFrame]:
    """Speak `text` ahead of time; the frames are played later with `replay`."""
    frames: list[rtc.AudioFrame] = []
    async with tts.synthesize(text) as stream:
        async for chunk in stream:
            frames.append(chunk.frame)
    if not frames:
        raise RuntimeError("the speech provider returned no audio")
    return frames


async def replay(frames: list[rtc.AudioFrame]) -> AsyncIterator[rtc.AudioFrame]:
    for frame in frames:
        yield frame


def ready_audio(task: asyncio.Task | None) -> list[rtc.AudioFrame] | None:
    """The pre-synthesized frames if they are ready now; never waits."""
    if task is None or not task.done() or task.cancelled():
        return None
    if task.exception() is not None:
        log.warning("pre-synthesizing the opening line failed (%s); speaking it live", task.exception())
        return None
    return task.result()
