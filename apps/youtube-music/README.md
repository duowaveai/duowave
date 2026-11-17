# YouTube Music — приложение DuoWave на Node

Музыка с YouTube на колонке: поиск и микс через InnerTube, звук через yt-dlp и ffmpeg в MP3. Работает на компьютере.
Подробно — [`docs/plugins/apps/youtube-music.md`](../../docs/plugins/apps/youtube-music.md).

```
duowave plugin install apps/youtube-music   # поставить в свой шлюз
node --test test/                     # проверки без сети
```

Руками, без шлюза: `PORT=8931 DUOWAVE_APP_TOKEN=test DUOWAVE_APP_DATA=/tmp/yt node server.mjs`, потом
`POST /call/playPhrase {"query": "…"}` с `Authorization: Bearer test` и `GET /stream/<id>.mp3`.
`DUOWAVE_YTDLP` — свой yt-dlp вместо скачанного, `FFMPEG` — свой ffmpeg.
