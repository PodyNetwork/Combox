"""How the interviewer's voice reaches the room: at once, and without breaks.

At once: the opening line (the AI disclosure) is the same for every
interview, so it is synthesized while the agent waits for the participant
(`presynthesize`) and played the moment they enter, with no round trip to the
speech provider; the fixed lines and questions likewise.

Without breaks (`use_steady_voice`): LiveKit Agents keeps only 200 ms of
speech queued ahead of the network, fed one 50 ms frame at a time by the
agent's event loop. Whenever that loop is held up for longer (a busy machine,
a garbage collection; stalls of 0.5-1.2 s show in real calls) the queue runs
dry and the voice breaks up. `SteadyAudioOutput` keeps `BUFFER_MS` queued
instead, in the native sender, which plays on through a stall.

A deeper queue alone was tried (DECISIONS PX-09) and removed (PX-12): when
the participant makes a short sound (a cough, "mm"), LiveKit pauses the
interviewer and throws away whatever is queued, then resumes past it, so up
to the whole queue was skipped mid-sentence. Here a pause takes the unplayed
speech back out of the queue and plays it when the voice resumes: nothing is
skipped, whatever the queue's size. Interruptions still cut the voice off at
once, and captions stay in step (the played position is accounted for).
"""

from __future__ import annotations

import asyncio
import logging
import time
from collections import deque
from collections.abc import AsyncIterator

from livekit import rtc
from livekit.agents.voice.room_io import _output, room_io

log = logging.getLogger("axom-meet")

# Speech kept queued ahead of the network (LiveKit Agents' own is 200 ms).
BUFFER_MS = 2000


class _Frames:
    """The frames waiting to be sent: a queue that can take frames back at
    its front (what `utils.aio.Chan`, which the library uses here, cannot)."""

    def __init__(self) -> None:
        self._queue: deque[rtc.AudioFrame] = deque()
        self._ready = asyncio.Event()

    def send_nowait(self, frame: rtc.AudioFrame) -> None:
        self._queue.append(frame)
        self._ready.set()

    def put_back(self, frames: list[rtc.AudioFrame]) -> None:
        """`frames`, in playing order, go ahead of everything waiting."""
        self._queue.extendleft(reversed(frames))
        if self._queue:
            self._ready.set()

    def empty(self) -> bool:
        return not self._queue

    def recv_nowait(self) -> rtc.AudioFrame:
        frame = self._queue.popleft()
        if not self._queue:
            self._ready.clear()
        return frame

    async def recv(self) -> rtc.AudioFrame:
        while not self._queue:
            self._ready.clear()
            await self._ready.wait()
        return self.recv_nowait()


def _tail(frame: rtc.AudioFrame, secs: float) -> rtc.AudioFrame | None:
    """The last `secs` of a frame (the part not played yet)."""
    samples = min(round(secs * frame.sample_rate), frame.samples_per_channel)
    if samples <= 0:
        return None
    if samples == frame.samples_per_channel:
        return frame
    data = frame.data[-samples * frame.num_channels :].tobytes()
    return rtc.AudioFrame(data, frame.sample_rate, frame.num_channels, samples)


class _Source(rtc.AudioSource):
    """An audio source whose playout is not over while speech taken back by
    a pause is still waiting to be sent again."""

    output: "SteadyAudioOutput | None" = None

    async def wait_for_playout(self) -> None:
        while True:
            await super().wait_for_playout()
            out = self.output
            if out is None or (out._audio_buf.empty() and out._pending is None):
                return
            await out._forwarding_idle.wait()
            await asyncio.sleep(0)


class SteadyAudioOutput(_output._ParticipantAudioOutput):
    """The agent's room audio output with a deep send queue that loses
    nothing when paused (see the module notes). It replaces the library's
    frame queue and its forwarding loop; everything else is the library's."""

    def __init__(self, room: rtc.Room, *, sample_rate: int, num_channels: int, buffer_ms: int = BUFFER_MS, **kwargs) -> None:
        super().__init__(room, sample_rate=sample_rate, num_channels=num_channels, **kwargs)
        # Not started or published yet: replacing the source and queue is safe.
        self._audio_source = _Source(sample_rate, num_channels, queue_size_ms=buffer_ms + 1000)
        self._audio_source.output = self
        self._audio_buf = _Frames()
        self._buffer_secs = buffer_ms / 1000
        # Frames handed to the source that may not have played yet, oldest first.
        self._sent: deque[rtc.AudioFrame] = deque()
        # The frame the forwarding loop holds and has not handed over yet.
        self._pending: rtc.AudioFrame | None = None

    def pause(self) -> None:
        super().pause()
        self._take_back()

    def _take_back(self) -> None:
        """Paused: pull what the source has not played yet back out of it, to
        be played when the voice resumes (the library drops it instead)."""
        back: list[rtc.AudioFrame] = []
        unplayed = self._audio_source.queued_duration
        taken = 0.0
        while self._sent and unplayed - taken > 0.001:
            frame = _tail(self._sent.pop(), unplayed - taken)
            if frame is None:
                break
            back.append(frame)
            taken += frame.duration
        self._sent.clear()
        back.reverse()
        if self._pending is not None:
            back.append(self._pending)
            self._pending = None
        if taken:
            self._source_pushed_duration -= taken
            # The playhead stops here; the next run starts from the same spot.
            self._report_run(offset=self._source_pushed_duration, ended_at=time.time())
            self._audio_source.clear_queue()
        self._audio_buf.put_back(back)

    async def _forward_audio(self) -> None:
        frames = self._audio_buf
        source = self._audio_source
        while True:
            frame = await frames.recv()
            self._forwarding_idle.clear()
            self._pending = frame
            try:
                # Until the frame is handed over, dropped, or taken back by a pause.
                while self._pending is frame:
                    if not self._playback_enabled.is_set():
                        self._take_back()
                        await self._playback_enabled.wait()
                        break
                    if self._interrupted_event.is_set() or self._pushed_duration == 0:
                        # Interrupted: this speech is over, drop what is left of it.
                        self._pending = None
                        self._sent.clear()
                        if self._interrupted_event.is_set() and self._flush_task:
                            await self._flush_task
                        break
                    room = self._buffer_secs - source.queued_duration - frame.duration
                    if room < 0:
                        # The queue is full: wait here, not inside the source,
                        # so a pause finds no frame half handed over.
                        await asyncio.sleep(min(-room, 0.05))
                        continue
                    self._pending = None
                    if not self._first_frame_event.is_set():
                        self._first_frame_event.set()
                        self.on_playback_started(created_at=time.time())
                    if self._dry_at is not None and time.time() >= self._dry_at:
                        # The source ran out before this frame arrived.
                        self._report_run(offset=self._source_pushed_duration, ended_at=self._dry_at)
                    self._source_pushed_duration += frame.duration
                    self._sent.append(frame)
                    while len(self._sent) > 1 and sum(f.duration for f in self._sent) - self._sent[0].duration > self._buffer_secs + 1:
                        self._sent.popleft()
                    dry_at = time.time() + source.queued_duration + frame.duration
                    await source.capture_frame(frame)
                    self._dry_at = dry_at
            finally:
                self._pending = None
                self._forwarding_idle.set()


def use_steady_voice() -> None:
    """Make the agent's room audio output a `SteadyAudioOutput`. The library
    has no option for it, so the class it builds is swapped for the subclass
    (`tests` check the hook is still there: livekit-agents is pinned)."""
    room_io._ParticipantAudioOutput = SteadyAudioOutput


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
