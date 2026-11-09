# omnivoice-server — синтез клонированным голосом (sidecar)

[OmniVoice](https://github.com/k2-fsa/OmniVoice) (код Apache-2.0, веса CC BY-NC 4.0 — только некоммерчески; 600+ языков) — zero-shot клонирование тембра:
«голос» = `models/<имя>/voice_prompt.pt`, полученный из 3–10 секунд речи (без обучения).
Отдельный процесс рядом с `tts-server/`: omnivoice нужен свой torch, а в один python-процесс
два torch не грузятся. Тот же OpenAI-совместимый API, что у tts-server — гейтвей переключается
парой переменных в `.env`, код шлюза не меняется.

> Голосов в репозитории нет и не будет: `models/` — ваша папка, она создаётся пустой и в git не попадает
> (`.gitignore`). Код не знает ни одного голоса по имени.

## Установка и запуск

```bash
./setup.sh                                  # venv (python 3.11) + torch 2.8 + omnivoice, ~5 мин
deploy/macos/run-omnivoice.sh               # старт; логи → logs/omnivoice.log
curl -s localhost:8092/health               # {"loaded": {"omnivoice": ["<имя>"]}, …}
# первый старт качает ~2 ГБ весов k2-fsa/OmniVoice с HuggingFace в ./hf-cache
```

Папка самодостаточна (venv, кэш весов, модели) — переносится на другой хост копированием
(на Apple Silicon считается на MPS, иначе CPU; Linux+NVIDIA — сменить torch на cu-сборку).

## API

- `POST /v1/audio/speech` — `{"model":"omnivoice","voice":"<имя>","input":"…","response_format":"wav"|"pcm"}`; без `voice` —
  единственный голос в `models/`
  → WAV 16-bit mono 24 kHz. Голоса нет — 500 со списком загруженных (без тихой подмены, как в tts-server).
- `GET /v1/models` — движок и голоса; `GET /health` — `{"status","loaded":{"omnivoice":[…]},"requests"}`
  (формат совместим с панельным индикатором ttsUp).
- **`POST /v1/voices?name=X&ref_text=Y`** — тело: байты клипа (wav, 5–10 с чистой речи) → новый
  `voice_prompt.pt`, подхватывается без рестарта. Транскрипцию клипа берёт на себя встроенный Whisper,
  если `ref_text` не задан. Это и есть «обучение голосом из UI»: клип → голос за ~30 секунд.

Пример:

```bash
curl -s "localhost:8092/v1/voices?name=test" --data-binary @clip.wav
curl -s localhost:8092/v1/audio/speech \
  -d '{"model":"omnivoice","voice":"test","input":"Привет, это мой голос"}' -o /tmp/ov.wav && afplay /tmp/ov.wav
```

## Подключение к гейтвею (тест) и откат

`.env`:

```
TTS_OWN_URL=http://127.0.0.1:8092/v1
TTS_OWN_MODEL=omnivoice
TTS_OWN_VOICE=<имя>           # папка голоса в models/
TTS_OWN_TIMEOUT_MS=20000      # RTF ~1.5 на MPS; запас на холодные фразы
```

и рестарт гейтвея. Откат — вернуть прежние значения (8091 + silero/piper) и перезапуститься.
Числа и латиница в тексте по-прежнему разворачивает шлюз (`sayNumbers`/`sayLatin`) — omnivoice это только на пользу.

Известная грань этапа-теста: кнопка «перезапустить» в панели рестартует tts-server (8091), не этот
sidecar — рестарт вручную `pkill -f omnivoice-server/server.py` + `deploy/macos/run-omnivoice.sh`
(под launchd — поднимется сам; plist появится на этапе полной интеграции).

## Будущая интеграция в панель (этап 2, эскиз)

Цель: карточка «Твой голос» в «Голосе», без библиотек в самом шлюзе — всё живёт в venv sidecar:

1. **Каталог**: `engine: 'omnivoice'` в enum `voice_models` (новая миграция в `gateway/migrations/`),
   карточка в `gateway/src/models/showcase.json`; голоса динамические → список из `GET /v1/models` sidecar, не из БД.
2. **Создание голоса из UI**: записать клип в панели (MediaRecorder) или загрузить файл →
   `POST /api/v1/models/voice-clone` (тонкий прокси в ModelsService) → `POST :8092/v1/voices` →
   голос сразу в `/health` и `/v1/models`; `needsRestart` не нужен.
3. **Синтез**: `OwnSpeech` уже подходит как есть (пара model/voice); при сосуществовании с silero/piper —
   два baseUrl по engine или роутинг в `VoiceService`.
4. **Рестарт/health**: третий `which: 'omnivoice'` в `ModelServers` (SCRIPTS/PROCESS/LAUNCHD) и в
   `POST /api/v1/models/servers/:which/restart`; `isLoaded` по `loaded.omnivoice`.
5. **Установка из UI**: `setup.sh` вызывается кнопкой (как загрузка моделей), статусы — по `/health`.
