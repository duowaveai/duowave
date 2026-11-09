# syntax=docker/dockerfile:1.7
# Recognition for the voice gateway: GigaAM and Parakeet behind an OpenAI-compatible API, what compose.yaml runs as
# `asr`. PyTorch for the processor by default, amd64 and arm64; for an NVIDIA card a CUDA build (amd64 only). The
# weights download on first use into /cache — a volume, so they stay; the model loaded from the panel is remembered
# there too.
#
#   docker build -t duowave-asr asr-server
#   docker build -t duowave-asr:cuda --build-arg TORCH=cu126 asr-server
#   docker build -t duowave-asr:whisper --build-arg WHISPER=1 asr-server   # + Whisper: GPL FFmpeg inside
ARG PYTHON_IMAGE=python:3.12-slim

# ---------------------------------------
# Builder: the environment in /opt/venv
# ---------------------------------------
FROM ${PYTHON_IMAGE} AS builder
# cpu, or a CUDA build of PyTorch: cu126 (the widest range of drivers), cu128, cu130, cu132.
ARG TORCH=cpu
# Whisper through faster-whisper: its PyAV bundles FFmpeg with GPL x264 and x265, so not by default.
ARG WHISPER=0
ARG TARGETARCH
ENV PIP_DISABLE_PIP_VERSION_CHECK=1
RUN python -m venv /opt/venv
ENV PATH=/opt/venv/bin:$PATH
COPY requirements.txt requirements-whisper.txt constraints.txt /tmp/req/
# PyTorch first, from its own index: PyPI's Linux build carries CUDA, gigabytes a processor never uses. Each CUDA
# index has its own versions: the newest pair it carries.
RUN --mount=type=cache,target=/root/.cache/pip set -eu; \
    case "$TORCH" in \
        cpu) torch=2.15.0 torchaudio=2.11.1 ;; \
        cu132) torch=2.15.0 torchaudio=2.11.1 ;; \
        cu126 | cu130) torch=2.14.1 torchaudio=2.11.0 ;; \
        cu128) torch=2.11.0 torchaudio=2.11.0 ;; \
        *) echo "TORCH: cpu, cu126, cu128, cu130 or cu132" >&2; exit 1 ;; \
    esac; \
    arch=${TARGETARCH:-$(dpkg --print-architecture)}; \
    if [ "$TORCH" != cpu ] && [ "$arch" != amd64 ]; then echo "CUDA builds are amd64 only" >&2; exit 1; fi; \
    pip install --index-url "https://download.pytorch.org/whl/$TORCH" "torch==$torch" "torchaudio==$torchaudio"
# The rest from PyPI at the tested versions; the torch above stays (requirements.txt pins the processor's pair).
RUN --mount=type=cache,target=/root/.cache/pip set -eu; \
    grep -v -E '^(torch|torchaudio)==' /tmp/req/requirements.txt > /tmp/req/rest.txt; \
    pip install -r /tmp/req/rest.txt -c /tmp/req/constraints.txt; \
    if [ "$WHISPER" = 1 ]; then pip install -r /tmp/req/requirements-whisper.txt -c /tmp/req/constraints.txt; fi

# ---------------------------------------
# Runtime
# ---------------------------------------
FROM ${PYTHON_IMAGE}
# 1000: the gateway image's user too, so a volume the two share is theirs whoever starts first.
ARG UID=1000
ARG GID=1000
RUN groupadd --gid "$GID" app \
    && useradd --uid "$UID" --gid app --create-home --home-dir /home/app --shell /usr/sbin/nologin app \
    && mkdir -p /cache && chown app:app /cache
COPY --from=builder /opt/venv /opt/venv
WORKDIR /app
COPY server.py ./

# Everything downloaded or remembered goes to /cache: GigaAM's weights in gigaam/, Parakeet's (and Whisper's) in
# huggingface/, the model chosen in the panel in chosen-model. GIGAAM_MODEL pins one at every start instead.
ENV PATH=/opt/venv/bin:$PATH \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    ASR_HOST=0.0.0.0 \
    ASR_PORT=8090 \
    ASR_CACHE_DIR=/cache \
    ASR_MODEL_FILE=/cache/chosen-model \
    HF_HOME=/cache/huggingface \
    XDG_CACHE_HOME=/cache \
    NUMBA_CACHE_DIR=/cache/numba
USER app
VOLUME /cache

# Not for the open network: no sign-in unless ASR_API_KEY is set (the gateway sends its own ASR_API_KEY).
EXPOSE 8090
# The first start downloads the model (~450 MB for GigaAM v3) before it answers.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10m --retries=3 \
    CMD ["python", "-c", "import os, urllib.request; urllib.request.urlopen('http://127.0.0.1:%s/health' % os.environ.get('ASR_PORT', '8090'), timeout=4)"]

CMD ["python", "server.py"]
