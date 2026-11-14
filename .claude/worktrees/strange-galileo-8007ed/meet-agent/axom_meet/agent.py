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

The LLM never drives the conversation: every spoken line comes from
`Interview`, and the LLM only proposes follow-up questions (filtered by
guards) and writes the summary and scores.
"""

from __future__ import annotations

import asyncio
import logging
import os

from livekit.agents import Agent, AgentSession, JobContext, StopResponse, WorkerOptions, cli
from livekit.plugins import silero

from .callback import fetch_keys, post_event
from .interview import Interview, Job
from .voice import build_stt, build_tts, tts_provider

log = logging.getLogger("axom-meet")
AGENT_NAME = "axom-interviewer"


class ClaudeLLM:
    """Minimal synchronous completion client for follow-ups and summaries."""

    def __init__(self, api_key: str | None = None, model: str | None = None) -> None:
        import anthropic

        self.client = anthropic.Anthropic(api_key=api_key)
        self.model = model or os.environ.get("AXOM_MEET_LLM_MODEL", "claude-sonnet-5-5")

    def complete(self, system: str, prompt: str) -> str:
        msg = self.client.messages.create(model=self.model, max_tokens=800, system=system, messages=[{"role": "user", "content": prompt}])
        return "".join(getattr(b, "text", "") for b in msg.content)


class Interviewer(Agent):
    def __init__(self, interview: Interview) -> None:
        super().__init__(instructions="You are an AI interviewer. Speak only the lines you are given.")
        self.interview = interview
        self._timer: asyncio.Task | None = None
        self._finished = False

    async def on_enter(self) -> None:
        # The clock starts with the disclosure.
        line = self.interview.start()
        self._timer = asyncio.create_task(self._time_limit())
        await self.session.say(line, allow_interruptions=False)

    async def on_user_turn_completed(self, turn_ctx, new_message) -> None:
        text = new_message.text_content or ""
        if not self._finished:
            reply = await asyncio.to_thread(self.interview.hear, text)
            if reply.say:
                handle = self.session.say(reply.say)
                if reply.done:
                    await handle
                    await self._finish()
        # Never let the default LLM reply speak.
        raise StopResponse()

    async def _time_limit(self) -> None:
        """Hard stop at the interview's time limit."""
        await asyncio.sleep(self.interview.remaining())
        if self._finished:
            return
        reply = self.interview.time_up()
        log.info("interview %s reached its time limit", self.interview.job.meeting_id)
        self.session.interrupt()
        if reply.say:
            await self.session.say(reply.say, allow_interruptions=False)
        await self._finish()

    async def _finish(self) -> None:
        if self._finished:
            return
        self._finished = True
        if self._timer and self._timer is not asyncio.current_task():
            self._timer.cancel()
        iv = self.interview
        try:
            if iv.consent and iv.consent["granted"]:
                results = await asyncio.to_thread(iv.results)
                await asyncio.to_thread(post_event, iv.job.callback_url, iv.job.callback_token, results)
        finally:
            self.session.shutdown()


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
        post_event(job.callback_url, job.callback_token, event)

    # The company's own keys first, then this agent's environment.
    company = await asyncio.to_thread(fetch_keys, job.callback_url, job.callback_token)

    def key(name: str) -> str | None:
        return company.get(name) or os.environ.get(name) or None

    anthropic_key = key("ANTHROPIC_API_KEY")
    # The company's Claude model (the one its AI workers use), if it chose one.
    llm = ClaudeLLM(anthropic_key, company.get("AXOM_AI_MODEL")) if anthropic_key else None
    interview = Interview(job=job, llm=llm, emit=emit)
    log.info("meeting %s: voice stt=deepgram tts=%s, limit %ss", job.meeting_id, tts_provider(key), job.max_duration_secs)
    session = AgentSession(stt=build_stt(key), tts=build_tts(key), vad=silero.VAD.load())
    await session.start(agent=Interviewer(interview), room=ctx.room)


def main() -> None:
    cli.run_app(WorkerOptions(entrypoint_fnc=entrypoint, agent_name=AGENT_NAME))


if __name__ == "__main__":
    main()
