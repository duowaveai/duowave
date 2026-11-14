# Чужое в проекте: библиотеки, модели, шрифт

Код проекта — Apache-2.0, документация — CC BY 4.0. Всё ниже — чужое, со своими лицензиями. Проверено 06.11.2026;
крейты шлюза — `cargo metadata`, пакеты пульта — `pnpm licenses list --prod`.

## Шлюз (crates.io)

Проверено 06.11.2026 по `cargo metadata`: в сборке для колонки 295 пакетов, в сборке для ПК — 367, со всеми
возможностями (`silero`, `webrtc`, `livekit`) — 528. Почти все — MIT и/или Apache-2.0; остальные — BSD-2/3-Clause,
ISC, Zlib, Unicode-3.0, а корни сертификатов webpki-roots — CDLA-Permissive-2.0. Где лицензия на выбор и среди
вариантов есть LGPL (r-efi) или Unlicense (memchr и соседи), действует MIT. Ни GPL, ни «некоммерческих» лицензий.

Заметное:

- unsafe-libopus — BSD-3-Clause: libopus 1.3.1, переведённый в Rust (c2rust); © Xiph.Org и авторы Opus.
- SQLite (через rusqlite, вшит в программу) — общественное достояние.
- ring — Apache-2.0 и ISC; rustls — Apache-2.0, ISC или MIT.
- rmcp — Apache-2.0; russh — Apache-2.0; webrtc-rs — MIT или Apache-2.0; livekit — Apache-2.0.
- transcribe.cpp (крейты transcribe-cpp и transcribe-cpp-sys 0.4.1, возможность `local-asr`, сборка приложения) —
  MIT, © handy-computer; внутри — ggml (MIT, © The ggml authors) и miniz (MIT). Собирается из исходников C++ при
  сборке, вшивается в программу. Распознавание в отдельном процессе-исполнителе устроено по образцу Handy
  ([cjpais/Handy](https://github.com/cjpais/Handy), MIT, © CJ Pais): тот же способ — программа с флагом, кадры по
  stdin и stdout, конец исполнителя с концом stdin; код написан заново.
- uv (Astral) — Apache-2.0 или MIT на выбор: приложение несёт программу uv 0.13.5 рядом со своей
  (`scripts/release/fetch-uv.py`), ею ставит Python для своего голоса и голосовой лаборатории.
- MIDI-клавиатура (возможность `midi`, приложение и программа для Mac и Windows): midir — MIT; coremidi — MIT; midly —
  Unlicense; на Linux — ALSA через alsa и alsa-sys (Apache-2.0 или MIT), сама libasound — системная (LGPL-2.1,
  подключается динамически).
- Домашняя папка (`src/bus/providers/files`): image с zune-jpeg, png, image-webp и gif — картинки уменьшенными (MIT или
  Apache-2.0; zune-jpeg ещё и Zlib); lopdf — текст PDF (MIT); zip — текст документа Word (MIT); unicode-normalization
  — имена в NFC и NFD (MIT или Apache-2.0); encoding_rs — буквы RTF в Windows-1251 ((Apache-2.0 или MIT) и
  BSD-3-Clause). В приложении — tauri-plugin-dialog (Apache-2.0 или MIT) с rfd (MIT): окно выбора папки.
- Готовые библиотеки, которые скачиваются при сборке с возможностями для ПК: ONNX Runtime (`silero`, через ort) —
  MIT, © Microsoft; libwebrtc для LiveKit (`livekit`) — BSD-3-Clause, © The WebRTC project authors, со своими
  сторонними частями (их список — в архиве libwebrtc).

## Пульт

- Preact — MIT.
- Шрифт Manrope (@fontsource/manrope) — SIL Open Font License 1.1.
- Шрифты Inter, Inter Tight и JetBrains Mono (@fontsource/inter, @fontsource/inter-tight, @fontsource/jetbrains-mono) —
  SIL Open Font License 1.1.

## В репозитории

- `assets/silero_vad.onnx` — Silero VAD, MIT, © Silero Team (`assets/SILERO_VAD_LICENSE`).
- Ничего от Яндекса: ни звуков, ни анимаций подсветки, ни прошивки, ни схем protobuf. Анимации каждый шлюз снимает у
  себя, из ответов Яндекса в журнале сессий; протокол колонки описан нами (`gateway/proto/station-protocol.proto`).
- Каталог новостных лент (`gateway/src/skills/lookups/news_catalog.json`) — только названия изданий и адреса их
  RSS-лент, собраны 06.11.2026. Подсказкой служили открытые списки с лицензией CC0:
  [awesome-rss-feeds](https://github.com/plenaryapp/awesome-rss-feeds) и Wikidata (свойство P1019, «адрес
  веб-ленты»). Адреса — те, что публикуют сами издания; каждая лента проверена запросом с домашнего Mac.
  Заголовки лент шлюз читает на лету, не хранит и никуда не передаёт, кроме колонки и модели.

## Серверы распознавания и голоса (Python, ставятся отдельно)

- torch — BSD-3-Clause; numpy, scipy — BSD; librosa — ISC; transformers — Apache-2.0.
- GigaAM — MIT (Сбер, SaluteDevices).
- faster-whisper — MIT, но его PyAV несёт x264 и x265 под GPL: в образ по умолчанию не входит.
- **Piper — GPL-3.0** (piper-tts вместе с espeak-ng). Сервер голоса запускает его отдельным процессом: код GPL не
  работает внутри нашего. Образ голоса (`tts-server/Dockerfile`) несёт Piper — его исходники:
  [OHF-voice/piper1-gpl v1.8.0](https://github.com/OHF-voice/piper1-gpl) и sdist piper-tts 1.8.0 на PyPI; образ
  только с Piper — `--build-arg SILERO=0`.
- Свой голос: OmniVoice (k2-fsa) — код Apache-2.0; CosyVoice 3 (FunAudioLLM) — Apache-2.0; VoxCPM (OpenBMB) —
  Apache-2.0. Веса — см. ниже.

## Модели

Веса скачиваются с Hugging Face и models.silero.ai по кнопке в пульте и в репозиторий не входят. Лицензия модели
решает, где её можно использовать.

Распознавание на компьютере (приложение, `ASR_PROVIDER=local`) берёт те же модели в формате GGUF — файлы, которые
публикует Handy (`handy-computer/*-gguf` на Hugging Face, ревизии и SHA-256 закреплены в
`gateway/src/speech/asr/local/catalog.rs`): GigaAM v3 — MIT, Parakeet TDT v3 — CC BY 4.0, Whisper — у файлов GGUF
Apache-2.0, исходные веса — MIT. Приложение их не несёт: модель скачивается при первом запуске.

| Модель | Лицензия | Дома, некоммерчески | За деньги, в облаке |
|---|---|---|---|
| GigaAM v1–v3 | MIT | да | да |
| Parakeet TDT 0.6B v3 (NVIDIA) | CC BY 4.0 | да, с указанием авторов | да, с указанием авторов |
| Whisper | MIT | да | да |
| Silero TTS `v4_ru`, `v5_ru` (silero.ai) | CC BY-NC 4.0 (MIT — только `v5_cis_base`) | да, с указанием авторов | нет |
| OmniVoice | веса CC BY-NC 4.0 (данные Emilia); токенизатор Higgs — лицензия Boson | да, с указанием авторов | нет |
| CosyVoice 3 | Apache-2.0, но на странице модели — «только для академических целей» | да | после юриста |
| VoxCPM 2 (OpenBMB) | Apache-2.0 — код и веса; карточка запрещает выдавать чужой голос за живого человека | да | да |
| Piper: denis, dmitri | CC0 | да | да |
| Piper: ruslan | CC BY-NC-SA 4.0 | да, с указанием авторов | нет |
| Piper: irina | не указана (RHVoice) | лучше не брать | нет |
| Другие голоса Piper | у каждого своя — в его MODEL_CARD на Hugging Face | проверять | проверять |

Пульт показывает лицензию на карточке каждой модели и голоса.

## Указание авторов (CC BY)

- Parakeet TDT 0.6B v3 — NVIDIA, CC BY 4.0.
- Silero TTS — Silero Team (silero.ai), CC BY-NC 4.0.
- OmniVoice — k2-fsa, веса CC BY-NC 4.0.
- Piper ruslan — rhasspy/piper-voices, CC BY-NC-SA 4.0.

## Товарные знаки

«Яндекс», «Алиса» и «Яндекс Станция» — товарные знаки их владельцев. Проект с Яндексом не связан и работает с
колонками, на которых владелец сам получил root.
