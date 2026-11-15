# duowave-airplay

Приёмник AirPlay для Яндекс Станции как приложение DuoWave (`runtime: process`). Внутри — библиотека
[station-airplay](https://github.com/skrashevich/station-airplay) (Apache-2.0); эта программа только связывает её со
шлюзом. Замысел — этап 2 `docs/plans/media-apps.md`, как приложение работает со шлюзом — «Приложения» в
`docs/plugins/AUTHORING.md`, для людей — `docs/plugins/apps/airplay.md`.

Ставится из MagicPlace (пакет `airplay` основного каталога, только на колонку). Руками на колонку не кладётся.

## Что делает

- Слушает `127.0.0.1:$PORT`: `GET /health`; `POST /call/control` — команда плеера (`pause`, `resume`, `next`,
  `previous`, `stop`), её шлюз шлёт на «Алиса, пауза / продолжай / дальше / назад»; `POST /call/status` — что сейчас.
- Телефон заиграл, встал на паузу, ушёл — говорит шлюзу (`system/player` → `playing`): шлюз ставит плеер Алисы на
  паузу и отдаёт голосовые команды сюда. Пока владелец не сказал «да» на «AirPlay → Плеер колонки», письмо
  отклоняется — программа повторяет его через 15 с, потом реже (до 5 мин).
- Имя для телефонов — `DUOWAVE_STATION_NAME` (имя колонки в DuoWave), без него «DuoWave». Сопряжение —
  `$DUOWAVE_APP_DATA/airplay`.
- Порт 7000 занят другим приёмником (YAM `skrashevich/station-airplay`, AirPlay в quasar-bridge) — пишет об этом в
  журнал и пробует снова каждые 30 с.
- SIGTERM — выключает приёмник (до 2,5 с) и выходит: ALSA и порты свободны до SIGKILL надзора.
- Своего голоса нет: у библиотеки IPC Алисы нет, голосом управляет шлюз.

## Сборка

```sh
sh apps/airplay/build.sh                                         # dist/duowave-airplay-linux-aarch64
sh apps/airplay/build.sh ../duowave-packages/packages/airplay    # и сразу в пакет каталога: файл, версия, SHA-256
```

Go 1.26+. `CGO_ENABLED=0 GOOS=linux GOARCH=arm64`, затем `go tool station-pcm-package -arch arm64` — дописывает в
файл модули вывода звука ARMv7 и ARM64 (на Лайт 2 ALSA 32-битная). Без этого шага звука нет. Версия программы —
`VERSION`, версия библиотеки — `go.mod`; сборка повторяется байт в байт (`-buildvcs=false`: от коммитов репозитория файл не зависит).

Проверки: `CGO_ENABLED=0 go test ./...`, `CGO_ENABLED=0 go vet ./...`. На колонке руками (по SSH, с «да» владельца):
`<файл> -check` — сеть и модуль вывода, без звука и без портов.

## Лицензии

Наш код — Apache-2.0. Библиотека приёмника — Apache-2.0 (производная от goplay2), декодер AAC
[go-aac](https://github.com/skrashevich/go-aac) — LGPL-3.0, ALAC — Apache-2.0, purego — Apache-2.0. Исходники всего,
что в файле, — этот каталог и модули из `go.sum`; заменить декодер — `replace` в `go.mod` и `build.sh`. Тексты
лицензий и NOTICE лежат в пакете каталога.

Новая версия station-airplay: `go get github.com/skrashevich/station-airplay@<версия>` и `go get -tool` для
`cmd/station-pcm-package` и `cmd/station-pcm` той же версии, поднять `VERSION`, `build.sh <пакет>`.
