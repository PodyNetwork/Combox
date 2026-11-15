# syntax=docker/dockerfile:1
# Combox meeting agent (LiveKit Agents worker). Build from meet-agent/ (BuildKit):
#   docker build -t axom-meet-agent meet-agent
FROM python:3.12.15-slim
RUN useradd --system --uid 10001 --create-home agent
WORKDIR /app
COPY pyproject.toml ./
# Deepgram does all voice by default. Build with --build-arg EXTRAS=cartesia
# to add the optional Cartesia text-to-speech provider.
ARG EXTRAS=""
# The dependencies first, from pyproject.toml alone, so changing the agent's
# code doesn't reinstall them. A current pip, patience for the large downloads
# (onnxruntime, av) on slow or flaky networks, and pip's download cache kept
# between builds (a cache mount: not part of the image).
RUN --mount=type=cache,target=/root/.cache/pip \
    pip install --upgrade pip \
 && EXTRAS="$EXTRAS" python -c "import os, tomllib; p = tomllib.load(open('pyproject.toml', 'rb'))['project']; extra = [d for e in os.environ['EXTRAS'].split(',') if e for d in p['optional-dependencies'][e]]; print('\n'.join(p['dependencies'] + extra))" > /tmp/requirements.txt \
 && pip install --retries 10 --timeout 120 --only-binary=:all: -r /tmp/requirements.txt
# Fetch the plugins' model files (Silero VAD) into the image before the
# agent's code is copied, so a code change doesn't download them again.
USER agent
RUN python -c "import importlib.util; from livekit.agents import Plugin; from livekit.plugins import deepgram, silero; importlib.util.find_spec('livekit.plugins.cartesia') and __import__('livekit.plugins.cartesia'); [p.download_files() for p in Plugin.registered_plugins]"
USER root
COPY axom_meet axom_meet
RUN --mount=type=cache,target=/root/.cache/pip \
    pip install --no-deps --retries 10 --timeout 120 .
USER agent
CMD ["axom-meet", "start"]
