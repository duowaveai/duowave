# Шлюз как служба Linux (systemd)

Для тех, кто ставит без Docker: Debian, Ubuntu, Raspberry Pi OS (64-битная) и другие системы с systemd. Шлюз — одна
программа `duowave` на Rust с пультом внутри ([gateway/](../../gateway/README.md)).

```sh
pnpm install                                # один раз: зависимости пульта
deploy/linux/install.sh                     # собирает пульт и шлюз, ставит службу duowave; sudo спросит сам
DUOWAVE=./duowave deploy/linux/install.sh   # или готовая программа из выпуска (duowave-linux-x86_64, -aarch64)
```

- Сборка внутри — `pnpm --filter panel build` и `cargo build -p duowave --release`, на x86_64 и aarch64 с
  `--features silero` (Silero VAD); нужен Rust ([rustup.rs](https://rustup.rs)). Программа из выпуска —
  статическая, без Silero: конец фразы она слышит по громкости. Собирать — от своего имени, не через `sudo`.
- Программа — `/opt/duowave/duowave`, у пользователя службы: шлюз обновляет её сам (пульт, «Дом» →
  «Обновления»), прежняя остаётся рядом как `duowave.previous`.
- Данные и настройки — `/var/lib/duowave` (`DATA_DIR`): база, журналы, голоса и `.env` для того, чего нет в
  пульте. Установщик кладёт туда `PORT=7780`: пульт — `http://<этот компьютер>:7780/panel`, колонка ходит на этот
  же порт. Порт может быть любым свободным; сменили — переподключите колонку (пульт предложит). Служба работает в
  папке проекта: её `.env` читается после `/var/lib/duowave/.env`.
- Порт ниже 1024 (например, 80, как до 0.1.1) служба тоже займёт без root: у неё есть право `CAP_NET_BIND_SERVICE` и
  больше никаких.
- Журнал: `journalctl -u duowave -f` — там и код первой настройки, если пульт открыт не с этого компьютера.
- Проверить: `DATA_DIR=/var/lib/duowave /opt/duowave/duowave health` в папке проекта — код 0, если
  `/health` отвечает.
- Обновить из исходников: `git pull && deploy/linux/install.sh` — соберёт и перезапустит службу.
- Удалить службу: `deploy/linux/uninstall.sh` (уберёт и программу из `/opt/duowave`); данные в
  `/var/lib/duowave` остаются. Колонку сначала верните к Яндексу: в пульте «Колонки» → колонка → «Вернуть к
  Яндексу».

Распознавание и голос на этом же компьютере — отдельные службы `duowave-asr` и `duowave-tts`; проще всего
их взять из Docker: `docker compose --profile full up -d asr tts`.
