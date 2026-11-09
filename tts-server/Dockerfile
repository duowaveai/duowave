# syntax=docker/dockerfile:1.7
# Our own voice for the voice gateway: Silero and Piper behind an OpenAI-compatible API, what compose.yaml runs as
# `tts`. The voices are what the gateway downloads: mount at /models the volume the gateway writes as /data/voices;
# a voice downloaded later speaks after a rescan, which the gateway asks for. amd64 and arm64.
#
# Piper (piper-tts, GPL-3.0, with espeak-ng inside) lives in an environment of its own, /opt/piper, and runs only as
# its own command line, a process per voice — never inside the server. An image with it carries GPL code: its
# source is https://github.com/OHF-voice/piper1-gpl (v1.8.0) and the piper-tts 1.8.0 sdist on PyPI.
#
#   docker build -t duowave-tts tts-server
#   docker build -t duowave-tts:piper --build-arg SILERO=0 tts-server   # Piper only: no torch, far smaller
ARG PYTHON_IMAGE=python:3.12-slim

# ---------------------------------------
# Builder: the server's environment in /opt/venv, Piper's in /opt/piper
# ---------------------------------------
FROM ${PYTHON_IMAGE} AS builder
# Silero needs torch (CC BY-NC weights: non-commercial use). cpu, or a CUDA build: cu126, cu128, cu130, cu132.
ARG SILERO=1
ARG TORCH=cpu
ARG TARGETARCH
ENV PIP_DISABLE_PIP_VERSION_CHECK=1
COPY requirements.txt requirements-piper.txt /tmp/req/
# PyTorch from its own index: PyPI's Linux build carries CUDA, gigabytes a processor never uses.
RUN --mount=type=cache,target=/root/.cache/pip set -eu; \
    python -m venv /opt/venv; \
    if [ "$SILERO" = 1 ]; then \
        case "$TORCH" in \
            cpu | cu132) torch=2.15.0 ;; \
            cu126 | cu130) torch=2.14.1 ;; \
            cu128) torch=2.11.0 ;; \
            *) echo "TORCH: cpu, cu126, cu128, cu130 or cu132" >&2; exit 1 ;; \
        esac; \
        arch=${TARGETARCH:-$(dpkg --print-architecture)}; \
        if [ "$TORCH" != cpu ] && [ "$arch" != amd64 ]; then echo "CUDA builds are amd64 only" >&2; exit 1; fi; \
        /opt/venv/bin/pip install --index-url "https://download.pytorch.org/whl/$TORCH" "torch==$torch"; \
        grep -v -E '^torch==' /tmp/req/requirements.txt > /tmp/req/rest.txt; \
    else \
        grep -E '^numpy==' /tmp/req/requirements.txt > /tmp/req/rest.txt; \
    fi; \
    /opt/venv/bin/pip install -r /tmp/req/rest.txt
RUN --mount=type=cache,target=/root/.cache/pip python -m venv /opt/piper \
    && /opt/piper/bin/pip install -r /tmp/req/requirements-piper.txt

# ---------------------------------------
# Runtime
# ---------------------------------------
FROM ${PYTHON_IMAGE}
# 1000: the gateway image's user too, so the voices volume the two share is theirs whoever starts first.
ARG UID=1000
ARG GID=1000
RUN groupadd --gid "$GID" app \
    && useradd --uid "$UID" --gid app --create-home --home-dir /home/app --shell /usr/sbin/nologin app \
    && mkdir -p /models /cache && chown app:app /models /cache
COPY --from=builder /opt/venv /opt/venv
COPY --from=builder /opt/piper /opt/piper
WORKDIR /app
COPY server.py ./

# TTS_MODELS, not the gateway's TTS_MODELS_DIR: a gateway .env passed in by mistake does not move the folder.
ENV PATH=/opt/venv/bin:$PATH \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    TTS_HOST=0.0.0.0 \
    TTS_PORT=8091 \
    TTS_MODELS=/models \
    PIPER_BIN=/opt/piper/bin/piper \
    XDG_CACHE_HOME=/cache
USER app
VOLUME ["/models", "/cache"]

# Not for the open network: no sign-in unless TTS_API_KEY is set (the gateway sends its TTS_OWN_API_KEY).
EXPOSE 8091
HEALTHCHECK --interval=30s --timeout=5s --start-period=2m --retries=3 \
    CMD ["python", "-c", "import os, urllib.request; urllib.request.urlopen('http://127.0.0.1:%s/health' % os.environ.get('TTS_PORT', '8091'), timeout=4)"]

CMD ["python", "server.py"]
