"""LiveKit Agents entrypoint. Runs as a Coolify service beside the
self-hosted LiveKit server and is dispatched explicitly by ForeMan
(agent name `axom-interviewer`) with the job in the dispatch metadata.

Providers (DECISIONS M6-04): voice is Deepgram alone by default, for both
speech-to-text and text-to-speech (DEEPGRAM_API_KEY); Cartesia text-to-speech
is optional (see `voice.py`). LLM Anthropic Claude (ANTHROPIC_API_KEY,
AXOM_MEET_LLM_MODEL). For each meeting the agent asks
ForeMan for the company's own keys and Claude model (Integrations, AI models:
the same ones its AI workers use);
keys the company has not set come from this agent's environment.

Interviews are timed: the agent waits for the candidate only until the join
link expires, and ends the interview at `max_duration_secs` (15 minutes, sent by ForeMan).

Questions: while the agent introduces itself and asks for consent, Claude
writes the interview questions from the role (`interview.plan_questions`);
the owner's own questions, if any, come first.

Presence (DECISIONS M6-14): ForeMan sends the agent whenever someone joins
an open meeting and it is not in the room. While the call runs, the agent
follows whoever is still in the room when the person it was talking to
leaves, and welcomes people back after a dropped connection.

Ending the call ends it for everyone: when the interview is over (all
questions asked, time up, consent declined, or the participant asks to end
it) the agent says goodbye, closes its session and deletes the room, which
disconnects the participant. The same happens when the last person in the
room leaves (the Leave button), or when the room has stayed empty for
`EMPTY_ROOM_GRACE_SECS` after a dropped connection. Either way ForeMan gets
the outcome before the job exits (results, or "no consent"), and ForeMan
then expires the meeting's join links.

The LLM never drives the conversation: every spoken line comes from
`Interview`, and the LLM only writes questions and follow-ups (filtered by
guards) and the summary and scores.
"""

from __future__ import annotations

import asyncio
import dataclasses
import logging
import os
import time

from livekit import rtc
from livekit.agents import Agent, AgentSession, JobContext, StopResponse, WorkerOptions, cli, room_io
from livekit.plugins import silero

from .callback import fetch_keys, post_event
from .interview import Interview, Job, plan_questions
from .voice import build_stt, build_tts, tts_provider

log = logging.getLogger("axom-meet")
AGENT_NAME = "axom-interviewer"
# Time the job gets after the call ends to write the summary and post it.
REPORT_TIMEOUT_SECS = 120
# How long the agent waits in an empty room for someone whose connection
# dropped to come back before it ends the call.
EMPTY_ROOM_GRACE_SECS = 120
# How long the consent answer may wait for the questions being written.
QUESTIONS_WAIT_SECS = 20
# Attempts to report an event to ForeMan before giving up.
CALLBACK_ATTEMPTS = 3
# Leaving on purpose (the Leave button, or the room being deleted), as
# opposed to a dropped connection.
LEFT_ON_PURPOSE = {rtc.DisconnectReason.CLIENT_INITIATED, rtc.DisconnectReason.ROOM_DELETED, rtc.DisconnectReason.USER_REJECTED}


class ClaudeLLM:
    """Minimal synchronous completion client for questions, follow-ups and summaries."""

    def __init__(self, api_key: str | None = None, model: str | None = None) -> None:
        import anthropic

        self.client = anthropic.Anthropic(api_key=api_key)
        self.model = model or os.environ.get("AXOM_MEET_LLM_MODEL", "claude-sonnet-5-5")

    def complete(self, system: str, prompt: str) -> str:
        msg = self.client.messages.create(model=self.model, max_tokens=800, system=system, messages=[{"role": "user", "content": prompt}])
        return "".join(getattr(b, "text", "") for b in msg.content)


def post_with_retry(job: Job, event: dict) -> bool:
    """Post an event to ForeMan, retrying briefly. A failure is logged, never
    raised: it must not keep the call open (ForeMan's expiry sweep closes a
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
    def __init__(self, interview: Interview, questions: asyncio.Task | None = None) -> None:
        super().__init__(instructions="You are an AI interviewer. Speak only the lines you are given.")
        self.interview = interview
        self._questions = questions
        self._timer: asyncio.Task | None = None
        self._finished = False
        self._reported = False
        self.started = False

    async def on_enter(self) -> None:
        # The clock starts with the disclosure.
        line = self.interview.start()
        self.started = True
        self._timer = asyncio.create_task(self._time_limit())
        await self.session.say(line, allow_interruptions=False)

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
        log.info("meeting %s: %s questions ready", job.meeting_id, len(questions))

    async def on_user_turn_completed(self, turn_ctx, new_message) -> None:
        text = new_message.text_content or ""
        if not self._finished:
            if self.interview.state == "awaiting_consent":
                await self._ensure_questions()
            reply = await asyncio.to_thread(self.interview.hear, text)
            if reply.say:
                # The goodbye is said in full before the call ends.
                handle = self.session.say(reply.say, allow_interruptions=not reply.done)
                if reply.done:
                    await handle
            if reply.done:
                await self._finish()
        # Never let the default LLM reply speak.
        raise StopResponse()

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
        """Give ForeMan the outcome, once, however the call ended: results if
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


async def entrypoint(ctx: JobContext) -> None:
    job = Job.from_metadata(ctx.job.metadata)
    await ctx.connect()
    # Wait for the candidate only until the join link expires.
    wait = job.seconds_until_join_deadline()
    try:
        await asyncio.wait_for(ctx.wait_for_participant(), timeout=wait)
    except asyncio.TimeoutError:
        log.info("meeting %s: nobody joined before the link expired; leaving", job.meeting_id)
        ctx.shutdown(reason="join link expired")
        return

    def emit(event: dict) -> None:
        # Consent is reported to ForeMan the moment it is given or declined.
        post_with_retry(job, event)

    # The company's own keys first, then this agent's environment.
    company = await asyncio.to_thread(fetch_keys, job.callback_url, job.callback_token)

    def key(name: str) -> str | None:
        return company.get(name) or os.environ.get(name) or None

    anthropic_key = key("ANTHROPIC_API_KEY")
    # The company's Claude model (the one its AI workers use), if it chose one.
    llm = ClaudeLLM(anthropic_key, company.get("AXOM_AI_MODEL")) if anthropic_key else None
    if llm is None:
        log.warning("meeting %s: no Anthropic key; using the owner's or the fallback questions", job.meeting_id)
    interview = Interview(job=job, llm=llm, emit=emit)
    # Written while the agent introduces itself and asks for consent.
    questions = asyncio.create_task(asyncio.to_thread(plan_questions, dataclasses.replace(job), llm))
    log.info("meeting %s: voice stt=deepgram tts=%s, limit %ss", job.meeting_id, tts_provider(key), job.max_duration_secs)
    session = AgentSession(stt=build_stt(key), tts=build_tts(key), vad=silero.VAD.load())
    interviewer = Interviewer(interview, questions)
    # However the call ends, ForeMan gets the outcome before the job exits.
    ctx.add_shutdown_callback(interviewer.report)

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


def main() -> None:
    cli.run_app(WorkerOptions(entrypoint_fnc=entrypoint, agent_name=AGENT_NAME, shutdown_process_timeout=REPORT_TIMEOUT_SECS))


if __name__ == "__main__":
    main()
