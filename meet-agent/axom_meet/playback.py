"""How the interviewer's voice reaches the room: smooth, and at once.

Smooth: LiveKit Agents buffers only 200 ms of the agent's speech ahead of the
network. When the agent's process is held up for longer than that (a busy
machine; stalls of 0.25-1.9 s were seen in real calls) the buffer runs dry
and the voice breaks up. `use_audio_buffer` raises it, so short stalls are
absorbed. Interruptions still cut speech off at once (the library clears the
buffer), and captions stay in step with what is heard (it accounts for what
is buffered).

At once: the opening line (the AI disclosure) is the same for every
interview, so it is synthesized while the agent waits for the participant
(`presynthesize`) and played the moment they enter, with no round trip to the
speech provider.
"""

from __future__ import annotations

import asyncio
import logging
from collections.abc import AsyncIterator

from livekit import rtc
from livekit.agents.voice.room_io import _output, room_io

log = logging.getLogger("axom-meet")

# Speech buffered ahead of the network. LiveKit Agents' default is 200 ms;
# a call on a busy machine stalled for up to 1.9 s at a time, so 3 s.
AUDIO_BUFFER_MS = 3000
def use_audio_buffer(ms: int = AUDIO_BUFFER_MS) -> None:
    """Make the agent's room audio output buffer `ms` of speech. The library
    has no option for it, so its output class is swapped for a subclass
    (`tests` check the hook is still there: livekit-agents is pinned)."""

    class BufferedAudioOutput(_output._ParticipantAudioOutput):
        def __init__(self, room: rtc.Room, *, sample_rate: int, num_channels: int, **kwargs) -> None:
            super().__init__(room, sample_rate=sample_rate, num_channels=num_channels, **kwargs)
            # Not started or published yet: replacing the source is safe.
            self._audio_source = rtc.AudioSource(sample_rate, num_channels, queue_size_ms=ms)

    room_io._ParticipantAudioOutput = BufferedAudioOutput


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
