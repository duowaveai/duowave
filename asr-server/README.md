# ASR server (GigaAM)

> Приложение DuoWave (и шлюз со сборкой `local-asr`) распознаёт само — без этого сервера: GigaAM v3 в формате GGUF
> через transcribe.cpp, см. [desktop/README.md](../desktop/README.md). Этот сервер — для шлюза в Docker, на другой
> машине или для тех, кому нужен PyTorch: шлюз уходит на него с `ASR_BASE_URL` / `ASR_PROVIDER=openai`.

Распознавание речи для шлюза: [GigaAM](https://github.com/salute-developers/GigaAM) (MIT) за
OpenAI-совместимым `POST /v1/audio/transcriptions`. Шлюз шлёт сюда одну фразу (WAV 16 кГц моно)
после конца речи — тот же вызов, что к OpenAI, поэтому их можно менять без изменений в коде.
Рядом с GigaAM — Parakeet TDT v3 (CC BY 4.0) и, по желанию, Whisper.

Модель по умолчанию — `v3_e2e_rnnt`: с пунктуацией и регистром («Алиса, привет.»). На Apple
M-серии фраза в 1–3 с распознаётся за 60–100 мс (`mps`), на CPU — в несколько раз дольше.

## Установка

Нужен Python 3.10+ и [uv](https://docs.astral.sh/uv/) (или обычный `pip`); git не нужен — GigaAM
берётся архивом закреплённого коммита:

```bash
cd asr-server
uv venv .venv --python 3.12
uv pip install -p .venv/bin/python -r requirements.txt -c constraints.txt
# Whisper — отдельно: PyAV внутри faster-whisper несёт FFmpeg с x264 и x265 под GPL
uv pip install -p .venv/bin/python -r requirements-whisper.txt -c constraints.txt
```

Версии закреплены по рабочему окружению (06.11): `requirements.txt` — то, что нужно серверу,
`constraints.txt` — всё, что под ним. На Linux torch из PyPI — сборка с CUDA на гигабайты: для
процессора сначала поставить пару torch и torchaudio из `https://download.pytorch.org/whl/cpu`, для
NVIDIA — из индекса своей CUDA (как это делает `Dockerfile`). Веса скачиваются при первом запуске
в `~/.cache/gigaam` и кэш Hugging Face, а с `--cache-dir` — туда.

## Запуск

```bash
.venv/bin/python server.py --model v3_e2e_rnnt --device auto --port 8090
```

- `--device auto` выбирает `cuda`, затем `mps`, иначе `cpu`.
- `--model none` — запуститься без модели и ждать её по HTTP. Без `--model` и `GIGAAM_MODEL` сервер
  берёт модель, загруженную по HTTP в прошлый раз (`--model-file`, по умолчанию `chosen-model` рядом:
  его же читает `deploy/macos/run-asr.sh`), иначе `v3_e2e_rnnt`. Запомненная модель, которая не
  загрузилась, уступает модели по умолчанию; заданная явно — останавливает сервер, как раньше.
- `--host 0.0.0.0` — если шлюз на другой машине. Тогда закрыть порт от чужих или задать
  `--api-key`: без него аутентификации нет.
- `--api-key` (`ASR_API_KEY`) — всё, кроме `/health`, только с `Authorization: Bearer <ключ>`. Шлюз
  шлёт свой `ASR_API_KEY`: одинаковое значение на обеих сторонах.
- `--cache-dir` (`ASR_CACHE_DIR`) — куда качать веса: `<папка>/gigaam` и `<папка>/huggingface`.
- Переменные окружения вместо флагов: `ASR_HOST`, `ASR_PORT`, `GIGAAM_MODEL`, `GIGAAM_DEVICE`,
  `ASR_CACHE_DIR`, `ASR_MODEL_FILE`, `ASR_API_KEY`.

Проверка: `curl http://127.0.0.1:8090/health` и, с любым WAV,

```bash
curl -F file=@phrase.wav -F model=gigaam-v3 http://127.0.0.1:8090/v1/audio/transcriptions
```

Каждый запрос — строка в логе: длина звука, текст, время. Запросы выполняются по одному (одна
модель, одна очередь GPU).

## Смена модели без перезапуска

```bash
curl -H 'Content-Type: application/json' -d '{"model": "v3_ctc"}' http://127.0.0.1:8090/v1/models/load
```

Новая модель грузится рядом с работающей и заменяет её после пробного распознавания; пока она
грузится (новая может скачиваться минуты), отвечает старая, а если не загрузилась — старая и
остаётся. Память на этот миг — под обе. Одна загрузка за раз: другая модель в это время — 409.

- Ответ `200` — модель работает (`"changed": false` — она и работала); `400` — такой модели нет или
  её движок не установлен; `500` — не загрузилась, причина в `error.message`.
- С `"wait": false` — сразу `202`, а конец загрузки виден в `/health`: поле `loading` пропало, в
  `model` — новая модель или в `error` — почему нет. Так делает шлюз: загрузка дольше, чем кто-либо
  ждёт ответа.
- Имена — как в каталоге пульта: `v3_e2e_rnnt`, `v3_ctc`, `multilingual_ctc`, …, `parakeet-0.6b-v3`,
  `whisper-large-v3-turbo`. Путь к файлу по HTTP не принимается: чекпойнт GigaAM — это pickle.
- Загруженная модель запоминается в `--model-file`: следующий запуск начнётся с неё.

`/health` отвечает `model`, `device`, `requests`, `loading` (что грузится сейчас), `error` (почему не
загрузилась последняя) и `features: ["load"]` — по нему шлюз узнаёт, что модель можно сменить по HTTP.

## Docker

```bash
docker build -t duowave-asr asr-server                                   # процессор, amd64 и arm64
docker build -t duowave-asr:cuda --build-arg TORCH=cu126 asr-server      # NVIDIA, только amd64
docker run -d --name asr -p 127.0.0.1:8090:8090 -v asr-cache:/cache duowave-asr
```

- Слушает `0.0.0.0:8090` внутри контейнера, работает не от root (uid 1000), проверка здоровья — `/health`.
- Веса и запомненная модель — в томе `/cache`: первый запуск качает GigaAM v3 (~450 МБ) и только
  потом отвечает.
- `TORCH`: `cpu` (по умолчанию), `cu126` (подходит самым старым драйверам), `cu128`, `cu130`, `cu132`.
- Whisper — `--build-arg WHISPER=1`: в образ попадёт FFmpeg с кодеками под GPL, такой образ раздают
  вместе с исходниками этих частей.
- `GIGAAM_MODEL` закрепляет модель при каждом запуске — выбор из пульта тогда забывается после
  перезапуска контейнера. Без неё контейнер начинает с выбранной в пульте.
- Порт наружу не открывать: в `compose.yaml` шлюз ходит сюда по имени `asr`. Иначе — `ASR_API_KEY`.
