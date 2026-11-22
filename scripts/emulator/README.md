# Скрипты колонки в эмуляторе

Скрипты, которые шлюз кладёт на колонку, проверяются без колонки:

- `voice.sh` — наша копия `quasar.cfg` поверх файла прошивки (bind-mount);
- `S95duowave` — запуск при загрузке;
- `gateway.sh` — служба шлюза на колонке.

Проверка — `station-client/tests/station-check.sh`. Она работает только там, где её изменения ничего не стоят: в
эмуляторе (имя хоста `qemu-*`) или в привилегированном контейнере. На настоящей колонке она отказывается
запускаться.

## Два уровня

1. **Контейнер с busybox** — в обычном CI на каждый коммит (`ci.yml`, шаг «The station's scripts on a real kernel»).
   Настоящее ядро Linux и busybox. Раздел прошивки изображает tmpfs, перемонтированный только для чтения, с
   выдуманным `quasar.cfg`. Локально:

   ```bash
   docker run --rm --privileged --network none -v "$PWD/station-client:/w:ro" -e STATION_CHECK_SANDBOX=1 busybox:1.36 sh /w/tests/station-check.sh /w
   ```

2. **Эмулятор КУЯСЭ** (`station-emulator.yml`, вручную или при изменении `station-client/`). Внутри — четыре
   колонки из публичных OTA Яндекса, закреплённых по SHA-256: настоящий корень прошивки, её busybox, её
   `quasar.cfg`. ВМ работает в режиме снимка, поэтому все изменения исчезают вместе с ней.

   | Плата (`--board`) | Колонка | Прошивка |
   | --- | --- | --- |
   | `station2` | Станция 2 | 0.373.1.13 |
   | `yandexmini` | Мини | 1.373.4.13 |
   | `yandexmicro` | Лайт 1 | 0.372.1.8 |
   | `yandexmini2` | Мини 2 | 0.372.1.8 |

   Сборка эмулятора и образы Станции 2 и Мини — те же, на которых проверяет себя Quasar Bridge (Apache-2.0).
   Ссылки Лайт 1 и Мини 2 — официальные, из каталога автора эмулятора
   ([iot-quasar-dataset](https://huggingface.co/datasets/vypivshiy/iot-quasar-dataset)); сами файлы скачиваются
   с Яндекса. Плата Лайт 2 в эмуляторе есть, но публичной ссылки на её OTA нет, поэтому её проверяет только живая
   колонка. Лайт 1 и Мини 2 ближе всего к ней: тот же процессор Amlogic A113X, `/data` так же ведёт в
   `/bkupgrade/data`.

   Запустить у себя можно только на Linux, нужны `qemu-system-aarch64`, `ipxe-qemu`, `adb`, Rust и Python:

   ```bash
   bash scripts/emulator/build.sh /tmp/kuyase
   python3 -m venv /tmp/venv && /tmp/venv/bin/pip install ubi-reader==0.8.16
   /tmp/venv/bin/python scripts/emulator/prepare.py --board station2 --emulator /tmp/kuyase --output /tmp/firmware
   python3 scripts/emulator/run.py --board station2 --emulator /tmp/kuyase --output /tmp/results
   ```

   Скачивается: исходники и портативный архив КУЯСЭ 1.4.0 с GitHub и OTA выбранной колонки с
   `quasar.s3.yandex.net` — сотни мегабайт.

## Чего здесь нет

- Звука.
- Сети колонки и самого Яндекса.
- Родного ядра колонок: у эмулятора одно своё ядро на все платы.
- Прошивки Лайт 2: её корня и её сборки busybox.

Это проверяет только живая колонка, вместе с владельцем — по правилам из `CLAUDE.md`.
