"""Speech providers. Deepgram handles all voice by default: speech-to-text
(Nova) and text-to-speech (Aura), with one key, DEEPGRAM_API_KEY.

Cartesia is optional. Set AXOM_MEET_TTS=cartesia (with CARTESIA_API_KEY) and
install the extra (`pip install 'axom-meet[cartesia]'`) to use it for
text-to-speech instead; speech-to-text stays on Deepgram.

    AXOM_MEET_TTS        deepgram (default) | cartesia
    AXOM_MEET_TTS_VOICE  optional voice/model, e.g. aura-2-thalia-en
    AXOM_MEET_STT_MODEL  optional Deepgram STT model, e.g. nova-3
    AXOM_MEET_STT_LANGUAGE  optional, default "en" (any English accent;
                         "en-US" tunes for American English only)

Settings are read through `key`: the company's own keys first, then this
agent's environment (see `agent.entrypoint`).
"""

from __future__ import annotations

import os
from collections.abc import Callable, Mapping

TTS_PROVIDERS = ("deepgram", "cartesia")

Key = Callable[[str], "str | None"]


def _lookup(key: Key | Mapping[str, str] | None) -> Key:
    if key is None:
        return lambda name: os.environ.get(name) or None
    if isinstance(key, Mapping):
        return lambda name: key.get(name) or None
    return key


def tts_provider(key: Key | Mapping[str, str] | None = None) -> str:
    name = (_lookup(key)("AXOM_MEET_TTS") or "deepgram").strip().lower()
    if name not in TTS_PROVIDERS:
        raise ValueError(f"AXOM_MEET_TTS must be one of {', '.join(TTS_PROVIDERS)}, not {name!r}")
    return name


def build_stt(key: Key | Mapping[str, str] | None = None):
    from livekit.plugins import deepgram

    get = _lookup(key)
    opts = {"api_key": get("DEEPGRAM_API_KEY"), "language": get("AXOM_MEET_STT_LANGUAGE") or "en"}
    if model := get("AXOM_MEET_STT_MODEL"):
        opts["model"] = model
    return deepgram.STT(**opts)


def build_tts(key: Key | Mapping[str, str] | None = None):
    get = _lookup(key)
    voice = get("AXOM_MEET_TTS_VOICE")
    if tts_provider(get) == "cartesia":
        try:
            from livekit.plugins import cartesia
        except ImportError as e:  # the optional extra is not installed
            raise RuntimeError("AXOM_MEET_TTS=cartesia needs the optional extra: pip install 'axom-meet[cartesia]'") from e
        opts = {"api_key": get("CARTESIA_API_KEY")}
        if voice:
            opts["voice"] = voice
        return cartesia.TTS(**opts)
    from livekit.plugins import deepgram

    opts = {"api_key": get("DEEPGRAM_API_KEY")}
    if voice:
        opts["model"] = voice
    return deepgram.TTS(**opts)
