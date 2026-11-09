# Приложение «Spotify» — пилот этапа 9

Статус: **ждёт проверки аккаунта владельцем** (решение 5 плана `docs/plans/plugins.md`). Надзор, Host API по localhost
и установка приложений в шлюзе готовы и проверены на эталонном приложении `duowave-echo-app`
(`gateway/src/bin/echo_app.rs`, тест `gateway/tests/apps.rs`). Само приложение Spotify живёт **не в дереве шлюза** —
отдельной папкой или репозиторием: librespot — неофициальный клиент, в открытый каталог от имени проекта его не кладём.

## Сначала — проверка аккаунта (около 15 минут, колонку не трогает)

У части аккаунтов Spotify не выдаёт ключи расшифровки неофициальным клиентам (librespot, issue #1649). Проверяет
владелец на Mac, своим аккаунтом Premium:

```sh
brew install librespot          # или: cargo install librespot
librespot --name "Проверка DuoWave" --backend pipe --device /dev/null -v
```

В Spotify на телефоне выбрать устройство «Проверка DuoWave» и включить любой трек. В выводе librespot:

- `Loading <track>` и затем без ошибок — ключи выдаются, пилот делаем;
- `Unable to load key` / `audio key error` — Spotify не отдаёт ключ этому аккаунту, первым приложением Spotify не берём.

## Как устроено

- Вход — **Spotify Connect**: приложение объявляет устройство в домашней сети (mDNS, discovery librespot), человек
  выбирает «Колонка в гостиной» в Spotify на телефоне — вход передаёт сам Spotify. OAuth с адресом возврата шлюза не
  годится: Spotify принимает только HTTPS или `127.0.0.1`.
- Звук — librespot отдаёт PCM; приложение кодирует его в MP3 (или отдаёт Ogg/Opus) и раздаёт по HTTP на своём порту;
  шлюз ставит этот поток колонке через свой `/media` и `audio_play` — тогда «пауза», «дальше», «громче» работают
  встроенными навыками. Приложение на Mac — поток по сети; на колонке — несжатый PCM по localhost.
- Голосом: «Алиса, включи Spotify» — фраза приложения (`contributes.intents`, возможность `music`); поиск и плейлисты —
  инструменты «Мозга» (`spotify_search`, `spotify_play`), ответ — действие `{say, play}`.

## Манифест

```json
{
  "schema": 1,
  "kind": "app",
  "runtime": "process",
  "id": "spotify",
  "version": "0.1.0",
  "title": "Spotify",
  "api": "^1",
  "runs": ["pc", "station", "stationServer"],
  "prefer": "home",
  "budget": { "memoryMb": 30, "storageMb": 15 },
  "health": { "path": "/health", "intervalS": 15 },
  "permissions": ["media.source", "voice.intents", "llm.tools", "kv", "net:*.spotify.com", "net:*.scdn.co", "lan.mdns"],
  "targets": {
    "linux-aarch64": { "file": "bin/spotify-linux-aarch64", "sha256": "…" },
    "darwin-arm64": { "file": "bin/spotify-darwin-arm64", "sha256": "…" },
    "linux-x86_64": { "file": "bin/spotify-linux-x86_64", "sha256": "…" }
  },
  "contributes": {
    "intents": [{ "phrases": ["включи спотифай", "включи spotify"], "handler": "resume", "capability": "music" }],
    "tools": [
      { "name": "spotify_search", "description": "Ищет треки, альбомы и плейлисты в Spotify", "handler": "search",
        "input": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] },
        "annotations": { "readOnlyHint": true } },
      { "name": "spotify_play", "description": "Включает найденное в Spotify на колонке", "handler": "play",
        "input": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] } }
    ],
    "media": [{ "id": "now", "kind": "music", "handler": "now" }]
  }
}
```

## Программа

Обёртка над крейтами librespot (`librespot-core`, `-playback`, `-connect`, `-discovery`), статически для колонки
(`aarch64-unknown-linux-musl`, `cargo zigbuild`, как сам шлюз). Что она обслуживает на `127.0.0.1:$PORT`, как
`duowave-echo-app`:

- `GET /health`;
- `POST /call/resume`, `/call/search`, `/call/play`, `/call/now` — функции из манифеста, JSON в ответ;
- `GET /stream` — поток для колонки (шлюз отдаёт его через `/media`).

Окружение от шлюза: `PORT`, `DUOWAVE_URL`, `DUOWAVE_APP_TOKEN`, `DUOWAVE_APP_ID`, `DUOWAVE_APP_DATA` (там кэш учётных
данных librespot). Host API — `/api/v1/apps/host/…` с токеном (журнал, `kv`, речь).

## Ограничения

- Нужен Premium; часть аккаунтов не проходит проверку выше.
- Несколько колонок — своя сессия librespot на каждую и своя память (20–30 МБ на сессию); одна музыка везде — один поток.
- Правовой риск: librespot — против условий Spotify. Для своего дома допустимо; в каталог — только сторонним
  приложением со своим автором.
