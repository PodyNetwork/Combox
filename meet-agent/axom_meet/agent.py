"""LiveKit Agents entrypoint. Runs as a Coolify service beside the
self-hosted LiveKit server and is dispatched explicitly by Combox
(agent name `axom-interviewer`) with the job in the dispatch metadata.

Providers (DECISIONS M6-04): voice is Deepgram alone by default, for both
speech-to-text and text-to-speech (DEEPGRAM_API_KEY); Cartesia text-to-speech
is optional (see `voice.py`). LLM Anthropic Claude (ANTHROPIC_API_KEY,
AXOM_MEET_LLM_MODEL). For each meeting the agent asks
Combox for the company's own keys and Claude model (Integrations, AI models:
the same ones its AI workers use);
keys the company has not set come from this agent's environment.

Interviews are timed: the agent waits for the candidate only until the join
link expires, and ends the interview at `max_duration_secs` (15 minutes, sent by Combox).

Speed: the agent is sent when the join page opens, so it loads the
company's keys, starts its session (voice connections, audio track),
records its opening line and starts writing the questions while the person
is still in the lobby; it speaks the moment they enter.

Questions: Claude writes the interview questions from the role
(`interview.plan_questions`), from the moment the agent arrives; the owner's
own questions, if any, come first. Follow-ups come from a fast model with a
hard time cap (`ClaudeLLM(fast=True)`), so an answer is never met with a long
silence.

Presence (DECISIONS M6-14): Combox sends the agent whenever someone joins
an open meeting and it is not in the room. While the call runs, the agent
follows whoever is still in the room when the person it was talking to
leaves, and welcomes people back after a dropped connection.

Ending the call ends it for everyone: when the interview is over (all
questions asked, time up, consent declined, or the participant asks to end
it) the agent says goodbye, closes its session and deletes the room, which
disconnects the participant. The same happens when the last person in the
room leaves (the Leave button), or when the room has stayed empty for
`EMPTY_ROOM_GRACE_SECS` after a dropped connection. Either way Combox gets
the outcome before the job exits (results, or "no consent"), and Combox
then expires the meeting's join links.

The LLM never drives the conversation: every spoken line comes from
`Interview`, and the LLM only writes questions and follow-ups (filtered by
guards) and the summary and scores.
"""

from __future__ import annotations

import asyncio
import dataclasses
import functools
import logging
import os
import time

from livekit import rtc
from livekit.agents import Agent, AgentSession, JobContext, JobProcess, StopResponse, WorkerOptions, cli, room_io
from livekit.plugins import silero

from . import guards
from .callback import fetch_keys, post_event
from .interview import CLOSING, DECLINED, REASK, Checkpoint, Interview, Job, plan_questions
from .playback import presynthesize, ready_audio, replay
from .turns import MAX_WAIT_SECS, TURN_HANDLING, AnswerEndDetector
from .voice import build_stt, build_tts, tts_provider

log = logging.getLogger("axom-meet")
AGENT_NAME = "axom-interviewer"
# Time the job gets after the call ends to write the summary and post it.
REPORT_TIMEOUT_SECS = 120
# How long the agent waits in an empty room for someone whose connection
# dropped to come back before it ends the call.
EMPTY_ROOM_GRACE_SECS = 120
# Said the moment an answer ends while the follow-up is being decided, so
# the participant never waits in silence (rotated; audio made in advance).
ACKNOWLEDGEMENTS = ("Thank you.", "Got it.", "Okay, thanks.", "Thanks for that.")
# How long the consent answer may wait for the questions being written
# (they are usually ready long before); past it, the fallback ones are asked.
QUESTIONS_WAIT_SECS = 8
# Longest a follow-up may take to write; past it, the next question is asked.
FOLLOW_UP_TIMEOUT_SECS = 4.0
# Longest the company's keys may take to load; past it, the agent's own are used.
KEYS_TIMEOUT_SECS = 5.0
# The worker takes interviews until the machine is this busy (LiveKit's
# default, 0.7, turned calls away on a shared server and left people waiting).
LOAD_THRESHOLD = 0.9
# Worker processes kept loaded and ready for the next call. Each holds the
# speech models (~150 MB); one is enough, since another loads as soon as it
# is taken (LiveKit's default of up to 4 starved small machines of memory).
IDLE_PROCESSES = int(os.environ.get("AXOM_MEET_IDLE_PROCESSES", "1"))
# Attempts to report an event to Combox before giving up.
CALLBACK_ATTEMPTS = 3
# Leaving on purpose (the Leave button, or the room being deleted), as
# opposed to a dropped connection.
LEFT_ON_PURPOSE = {rtc.DisconnectReason.CLIENT_INITIATED, rtc.DisconnectReason.ROOM_DELETED, rtc.DisconnectReason.USER_REJECTED}
# Someone who starts talking again this soon after their answer was handed
# over, before they have heard the reply to it in full, had not finished it
# (`Interviewer._carried_on`): nobody answers a question they haven't heard.
RESUME_WINDOW_SECS = 3.0
# Sound shorter than this (a cough, a click) is not someone carrying on.
MIN_RESUME_SECS = 0.3


@dataclasses.dataclass
class Answer:
    """The answer to a question last handed to the interview, kept until the
    participant moves on, so a part they add after a pause joins it."""

    before: Checkpoint
    text: str
    at: float
    # When the reply to it finished playing without being talked over.
    heard_at: float | None = None


class ClaudeLLM:
    """Minimal synchronous completion client for questions, follow-ups and summaries.

    `fast=True` is for follow-ups, which the participant waits on in silence:
    a fast model (AXOM_MEET_FAST_MODEL), low effort, a short reply, and a hard
    time cap with no retries. Past the cap the interview moves to the next
    question instead of making the participant wait.
    """

    def __init__(self, api_key: str | None = None, model: str | None = None, *, fast: bool = False, effort: str | None = None) -> None:
        import anthropic

        client = anthropic.Anthropic(api_key=api_key)
        self.fast = fast
        if fast:
            self.client = client.with_options(timeout=FOLLOW_UP_TIMEOUT_SECS, max_retries=0)
            self.model = os.environ.get("AXOM_MEET_FAST_MODEL", "claude-haiku-5-5")
        else:
            self.client = client
            self.model = model or os.environ.get("AXOM_MEET_LLM_MODEL", "claude-sonnet-5-5")
        self.effort = effort

    def complete(self, system: str, prompt: str) -> str:
        # Thinking counts toward max_tokens: leave room so JSON replies are never cut off.
        opts = {"max_tokens": 300, "output_config": {"effort": "low"}} if self.fast else {"max_tokens": 4000}
        if self.effort and not self.fast:
            opts["output_config"] = {"effort": self.effort}
        msg = self.client.messages.create(model=self.model, system=system, messages=[{"role": "user", "content": prompt}], **opts)
        return "".join(getattr(b, "text", "") for b in msg.content)


def post_with_retry(job: Job, event: dict) -> bool:
    """Post an event to Combox, retrying briefly. A failure is logged, never
    raised: it must not keep the call open (Combox's expiry sweep closes a
    meeting that never reports)."""
    for attempt in range(1, CALLBACK_ATTEMPTS + 1):
        try:
            post_event(job.callback_url, job.callback_token, event)
            return True
        except Exception as e:  # noqa: BLE001
            log.warning("meeting %s: posting %s failed (attempt %s): %s", job.meeting_id, event.get("event"), attempt, e)
            if attempt < CALLBACK_ATTEMPTS:
                time.sleep(attempt)
    return False


def is_agent(p: rtc.RemoteParticipant) -> bool:
    return p.kind == rtc.ParticipantKind.PARTICIPANT_KIND_AGENT


class Interviewer(Agent):
    def __init__(self, interview: Interview, questions: asyncio.Task | None = None, opening: asyncio.Task | None = None, tts=None) -> None:
        super().__init__(instructions="You are an AI interviewer. Speak only the lines you are given.")
        self.interview = interview
        self._questions = questions
        # The opening line's audio, synthesized while waiting for the participant.
        self._opening = opening
        self._timer: asyncio.Task | None = None
        self._finished = False
        self._reported = False
        self.started = False
        self.opening_presynthesized = False
        # Audio for every line known in advance (fixed lines, then the
        # questions once written), so they play with no speech round trip.
        self._tts = tts
        self._lines: dict[str, asyncio.Task] = {}
        self._acks = 0
        self._answer: Answer | None = None
        # When the participant started speaking (now), and when they first
        # spoke for real since the last answer was handed over.
        self._speech_started: float | None = None
        self._resumed_at: float | None = None
        self._closing_fallback: asyncio.Task | None = None
        for line in (*ACKNOWLEDGEMENTS, REASK, CLOSING, DECLINED):
            self._warm(line)
        self._since = time.monotonic()
        if questions is not None:
            questions.add_done_callback(self._warm_questions)

    def _warm(self, text: str) -> None:
        if self._tts is not None and text and text not in self._lines:
            self._lines[text] = asyncio.create_task(presynthesize(self._tts, text))

    def _warm_questions(self, task: asyncio.Task) -> None:
        if not task.cancelled() and task.exception() is None:
            log.info("meeting %s: questions written in %.1fs", self.interview.job.meeting_id, time.monotonic() - self._since)
            for q in task.result():
                self._warm(q)

    def _first_question_ready(self) -> bool:
        task = self._questions
        if task is None or not task.done() or task.cancelled() or task.exception() is not None:
            return False
        written = task.result()
        return bool(written) and ready_audio(self._lines.get(written[0])) is not None

    def _acknowledge(self) -> None:
        self._say(ACKNOWLEDGEMENTS[self._acks % len(ACKNOWLEDGEMENTS)])
        self._acks += 1

    def _say(self, text: str, **kwargs):
        """Say a line, from audio made in advance when it is ready."""
        frames = ready_audio(self._lines.get(text))
        return self.session.say(text, **({"audio": replay(frames)} if frames else {}), **kwargs)

    async def greet(self) -> None:
        """The participant is in the room: say the disclosure at once. (The
        session starts earlier, while they are in the lobby, so `on_enter`
        says nothing.)"""
        # The clock starts with the disclosure.
        line = self.interview.start()
        self.started = True
        self._timer = asyncio.create_task(self._time_limit())
        frames = ready_audio(self._opening)
        if frames is None and self._opening:
            self._opening.cancel()
        self.opening_presynthesized = frames is not None
        audio = {"audio": replay(frames)} if frames else {}
        await self.session.say(line, allow_interruptions=False, **audio)

    async def _ensure_questions(self) -> None:
        """Use the questions being written; past the wait, the owner's own or the fallback ones."""
        task, self._questions = self._questions, None
        if task is None:
            return
        job = self.interview.job
        try:
            questions = await asyncio.wait_for(task, timeout=QUESTIONS_WAIT_SECS)
        except Exception as e:  # noqa: BLE001 (a timeout or a model error: fall back)
            log.warning("meeting %s: writing questions failed (%s); using the fallback", job.meeting_id, e or type(e).__name__)
            questions = plan_questions(dataclasses.replace(job, generate_questions=False), None)
        job.questions = questions
        for q in questions:
            self._warm(q)
        log.info("meeting %s: %s questions ready", job.meeting_id, len(questions))

    def user_speaking(self, speaking: bool) -> None:
        """The participant started or stopped making sound (Silero)."""
        now = time.monotonic()
        if speaking:
            self._speech_started = now
            return
        started, self._speech_started = self._speech_started, None
        if started is not None and self._resumed_at is None and now - started >= MIN_RESUME_SECS:
            self._resumed_at = started

    def _carried_on(self, answer: Answer | None, resumed_at: float | None) -> bool:
        """This turn continues the last answer: the participant paused, the
        answer was handed over, and they started again within moments, before
        the reply to it had played out."""
        return (
            answer is not None
            and resumed_at is not None
            and self.interview.ended_by is None
            and resumed_at - answer.at <= RESUME_WINDOW_SECS
            and (answer.heard_at is None or resumed_at < answer.heard_at)
        )

    def _reply_done(self, answer: Answer, handle) -> None:
        if handle.interrupted:
            return
        answer.heard_at = time.monotonic()
        # The reply played out, so any sound before it (a cough) did not
        # interrupt it and was not the answer going on.
        self._resumed_at = None

    async def on_user_turn_completed(self, turn_ctx, new_message) -> None:
        text = new_message.text_content or ""
        resumed_at, self._resumed_at = self._resumed_at, None
        if not self._finished:
            iv = self.interview
            answer, self._answer = self._answer, None
            # Answer at once whenever the real reply needs a moment (the
            # questions or a follow-up still being written); it comes after.
            if iv.state == "awaiting_consent":
                if guards.classify_consent(text) == "granted" and not self._first_question_ready():
                    self._acknowledge()
                await self._ensure_questions()
            else:
                if self._carried_on(answer, resumed_at):
                    # They had paused, not finished: take the reply back and
                    # hear the whole answer as one.
                    log.info("meeting %s: the participant carried on after a pause; hearing the whole answer", iv.job.meeting_id)
                    if self._closing_fallback:
                        self._closing_fallback.cancel()
                    iv.rewind(answer.before)
                    text = f"{answer.text} {text}"
                if iv.state == "questions":
                    self._answer = Answer(iv.checkpoint(), text, time.monotonic())
                if iv.will_think() and text.strip():
                    self._acknowledge()
            answer = self._answer
            reply = await asyncio.to_thread(iv.hear, text)
            # The closing line after the last answer can be talked over, in
            # case that answer wasn't finished; every other goodbye is said
            # in full before the call ends.
            closing = reply.done and answer is not None and iv.state == "done" and not iv.timed_out
            if reply.say:
                handle = self._say(reply.say, allow_interruptions=not reply.done or closing)
                if answer is not None:
                    handle.add_done_callback(functools.partial(self._reply_done, answer))
                if reply.done:
                    await handle
                    if closing and handle.interrupted:
                        self._closing_fallback = asyncio.create_task(self._close_unless_carried_on())
                        raise StopResponse()
            if reply.done:
                await self._finish()
        # Never let the default LLM reply speak.
        raise StopResponse()

    async def _close_unless_carried_on(self) -> None:
        """The closing line was talked over: give the participant time to
        finish (their turn then reopens the interview), else end the call."""
        quiet_since = time.monotonic()
        while time.monotonic() - quiet_since < MAX_WAIT_SECS + RESUME_WINDOW_SECS:
            await asyncio.sleep(0.25)
            if self._speech_started is not None:
                quiet_since = time.monotonic()
        if self.interview.finished():
            await self._finish()

    async def welcome_back(self) -> None:
        """Someone came back to an empty room: repeat where the interview is."""
        if self._finished or not self.started:
            return
        await asyncio.sleep(1.5)  # let them subscribe to the agent's audio first
        if not self._finished and (line := self.interview.resume()):
            self.session.say(line)

    async def _time_limit(self) -> None:
        """Hard stop at the interview's time limit."""
        await asyncio.sleep(self.interview.remaining())
        if self._finished:
            return
        reply = await asyncio.to_thread(self.interview.time_up)
        log.info("interview %s reached its time limit", self.interview.job.meeting_id)
        self.session.interrupt()
        if reply.say:
            await self.session.say(reply.say, allow_interruptions=False)
        await self._finish()

    async def _finish(self) -> None:
        """End the call: closing the session deletes the room, which
        disconnects the participant; the results are posted on job shutdown
        (`report`), so nobody waits in the room for the summary."""
        if self._finished:
            return
        self._finished = True
        if self._timer and self._timer is not asyncio.current_task():
            self._timer.cancel()
        self.session.shutdown()

    async def end_call(self, why: str) -> None:
        """End the call without a goodbye (nobody is left to hear it)."""
        if self._finished:
            return
        log.info("meeting %s: %s; ending the interview", self.interview.job.meeting_id, why)
        await self._finish()

    async def report(self) -> None:
        """Give Combox the outcome, once, however the call ended: results if
        consent was given, otherwise a recorded "no consent"."""
        if self._reported:
            return
        self._reported = True
        self._finished = True
        if self._timer:
            self._timer.cancel()
        iv = self.interview
        if not iv.finished():
            # The call ended before the interview did: the participant left
            # (or the session failed).
            log.info("meeting %s: the participant left; ending the interview", iv.job.meeting_id)
            await asyncio.to_thread(iv.left)
        if iv.consent and iv.consent["granted"]:
            try:
                results = await asyncio.to_thread(iv.results)
                await asyncio.to_thread(post_with_retry, iv.job, results)
            except Exception:
                log.exception("meeting %s: posting the results failed", iv.job.meeting_id)


def prewarm(proc: JobProcess) -> None:
    """One-time setup when a worker process starts, so a call does none of
    it: the person who joined hears the interviewer within seconds."""
    proc.userdata["vad"] = silero.VAD.load()
    import anthropic  # noqa: F401 (imported now, used per call)
    from livekit.plugins import deepgram  # noqa: F401


async def entrypoint(ctx: JobContext) -> None:
    started = time.monotonic()
    job = Job.from_metadata(ctx.job.metadata)
    await ctx.connect()

    def emit(event: dict) -> None:
        # Consent is reported to Combox the moment it is given or declined.
        post_with_retry(job, event)

    async def prepare() -> tuple[Interview, asyncio.Task, asyncio.Task, AgentSession]:
        """Everything that does not need the participant: the company's keys,
        the clients, the voice, the opening line's audio, and the questions
        (written from here on, so they are ready by the time consent is given)."""
        company = await asyncio.to_thread(fetch_keys, job.callback_url, job.callback_token, KEYS_TIMEOUT_SECS)

        # The company's own keys first, then this agent's environment.
        def key(name: str) -> str | None:
            return company.get(name) or os.environ.get(name) or None

        anthropic_key = key("ANTHROPIC_API_KEY")
        # The company's Claude model (the one its AI workers use), if it chose one.
        llm = ClaudeLLM(anthropic_key, company.get("AXOM_AI_MODEL")) if anthropic_key else None
        if llm is None:
            log.warning("meeting %s: no Anthropic key; using the owner's or the fallback questions", job.meeting_id)
        interview = Interview(job=job, llm=llm, follow_up_llm=ClaudeLLM(anthropic_key, fast=True) if llm else None, emit=emit)
        # Written at low effort: at the default the model thought for over a
        # minute, and the person's "yes" to consent waits on the questions.
        writer = ClaudeLLM(anthropic_key, company.get("AXOM_AI_MODEL"), effort="low") if llm else None
        questions = asyncio.create_task(asyncio.to_thread(plan_questions, dataclasses.replace(job), writer))
        log.info("meeting %s: voice stt=deepgram tts=%s, limit %ss", job.meeting_id, tts_provider(key), job.max_duration_secs)
        tts = build_tts(key)
        opening = asyncio.create_task(presynthesize(tts, job.disclosure))
        # The local speech detector (Silero) hears when the person stops and
        # when they talk over the interviewer; `turns` decides whether a
        # pause ends the answer (0.7 s after a finished sentence, up to 2.5 s
        # mid-thought, learning how this person pauses). Deepgram alone was
        # tried: an answer spoken over the interviewer was not handed over
        # until the person left. LiveKit's end-of-turn model is off: it held
        # ~244 MB per process and, on a busy machine, missed its 1 s deadline.
        vad = ctx.proc.userdata.get("vad") or silero.VAD.load()
        session = AgentSession(stt=build_stt(key), tts=tts, vad=vad, turn_handling={"turn_detection": AnswerEndDetector(), **TURN_HANDLING})
        return interview, questions, opening, session

    # The agent arrives when the join page opens: everything below is set up
    # while the person is still in the lobby, so when they enter, the
    # interviewer's audio track is already in the room and its first line is
    # already recorded.
    interview, questions, opening, session = await prepare()
    interviewer = Interviewer(interview, questions, opening, session.tts)
    joined = 0.0

    @session.on("agent_state_changed")
    def _speaking(ev) -> None:
        # The first time the interviewer is heard: how long the person waited.
        if ev.new_state == "speaking" and not getattr(_speaking, "done", False):
            _speaking.done = True
            now = time.monotonic()
            log.info(
                "meeting %s: first words %.1fs after someone was in the room (%.1fs after the job arrived; opening line %s)",
                job.meeting_id, now - joined, now - started, "pre-synthesized" if interviewer.opening_presynthesized else "spoken live",
            )

    @session.on("user_state_changed")
    def _user_state(ev) -> None:
        interviewer.user_speaking(ev.new_state == "speaking")

    async def close_room() -> None:
        # `delete_room_on_close` starts this as soon as the session closes;
        # awaiting it here makes sure job shutdown does not cancel it. A room
        # that is already gone is fine.
        await ctx.delete_room()

    ctx.add_shutdown_callback(close_room)

    @session.on("close")
    def _closed(ev) -> None:
        # The session closed: the interview is over, everyone left, or it
        # failed. The room is deleted with it (`delete_room_on_close`); end
        # the job too.
        log.info("meeting %s: call ended (%s)", job.meeting_id, ev.reason.value)
        ctx.shutdown(reason=f"call ended: {ev.reason.value}")

    await session.start(
        agent=interviewer,
        room=ctx.room,
        # People leaving is handled below: the call ends only when nobody is
        # left, so one person leaving never cuts off another.
        room_options=room_io.RoomOptions(close_on_disconnect=False, delete_room_on_close=True),
    )

    # Wait for the candidate only until the join link expires.
    try:
        await asyncio.wait_for(ctx.wait_for_participant(), timeout=job.seconds_until_join_deadline())
    except asyncio.TimeoutError:
        log.info("meeting %s: nobody joined before the link expired; leaving", job.meeting_id)
        ctx.shutdown(reason="join link expired")
        return
    joined = time.monotonic()
    # However the call ends, Combox gets the outcome before the job exits.
    ctx.add_shutdown_callback(interviewer.report)
    greeting = asyncio.create_task(interviewer.greet())  # noqa: F841 (kept so it is not garbage-collected)

    empty: dict[str, asyncio.Task | None] = {"timer": None}

    def people() -> list[rtc.RemoteParticipant]:
        return [p for p in ctx.room.remote_participants.values() if not is_agent(p)]

    async def end_if_still_empty() -> None:
        await asyncio.sleep(EMPTY_ROOM_GRACE_SECS)
        if not people():
            await interviewer.end_call("nobody came back to the room")

    @ctx.room.on("participant_disconnected")
    def _left(p: rtc.RemoteParticipant) -> None:
        if is_agent(p):
            return
        others = people()
        if others:
            # Keep talking to whoever is still here.
            rio = session.room_io
            linked = rio.linked_participant if rio else None
            if rio and (linked is None or linked.identity == p.identity or linked.identity not in {o.identity for o in others}):
                rio.set_participant(others[0].identity)
            return
        if p.disconnect_reason in LEFT_ON_PURPOSE:
            # The last person pressed Leave: that ends the interview.
            asyncio.create_task(interviewer.end_call("the participant left"))
        elif empty["timer"] is None or empty["timer"].done():
            # A dropped connection: wait for them to come back.
            empty["timer"] = asyncio.create_task(end_if_still_empty())

    @ctx.room.on("participant_connected")
    def _joined(p: rtc.RemoteParticipant) -> None:
        if is_agent(p):
            return
        timer, empty["timer"] = empty["timer"], None
        if timer and not timer.done():
            timer.cancel()
        if [o.identity for o in people()] == [p.identity]:
            # The room was empty: they are back.
            asyncio.create_task(interviewer.welcome_back())

    # Everyone may have left already: treat it like a dropped connection.
    if not people():
        empty["timer"] = asyncio.create_task(end_if_still_empty())


def main() -> None:
    # Log the agent's own stalls only when they are long: writing each report
    # (a stack trace) itself held the agent up on a busy machine.
    os.environ.setdefault("LIVEKIT_AGENTS_LOOP_BLOCK_WARN_MS", "500")
    # The end-of-turn model is not used (see `entrypoint`): don't load its weights.
    os.environ.setdefault("LIVEKIT_AGENTS_PRELOAD_LOCAL_INFERENCE", "0")
    cli.run_app(
        WorkerOptions(
            entrypoint_fnc=entrypoint,
            prewarm_fnc=prewarm,
            agent_name=AGENT_NAME,
            load_threshold=LOAD_THRESHOLD,
            num_idle_processes=IDLE_PROCESSES,
            # Loading the speech models took 17-35 s on a busy machine. Past
            # this limit LiveKit kills the spare process and loads it again,
            # in a loop that starves the call in progress; let it finish.
            initialize_process_timeout=120,
            shutdown_process_timeout=REPORT_TIMEOUT_SECS,
        )
    )


if __name__ == "__main__":
    main()
