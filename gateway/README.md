# duowave — the voice gateway in Rust

One program: the gateway for Yandex Station with its panel built in. It speaks the station's protocol, answers with
our own skills and models or relays to Yandex, keeps its data in one folder, and runs the same on a PC, a server, a
Raspberry Pi and on the station itself. It replaced the Node gateway, which is gone from the tree and kept in git
history at the tag `node-gateway-last` (the plan and its progress: `../docs/plans/rust-gateway.md`; the porting
rules: `PORTING.md`).

## Build and run

```sh
pnpm --filter panel build          # the panel, built into the program (without it /panel says it is missing)
cargo build -p duowave --release
DATA_DIR=./data ./target/release/duowave serve
```

The configuration is the one the Node gateway had: the same environment variables and `.env` (`../.env.example`),
read from `DATA_DIR/.env` first, then `./.env`. Everything the gateway writes goes under `DATA_DIR`: the database
(`state/duowave.sqlite` and its key `state/secrets.key`), the session journal, the content.

Commands:

| Command | What it does |
|---|---|
| `duowave serve` (or nothing) | the gateway |
| `duowave health` | exit 0 when the gateway on `PORT` answers `/health` — for Docker and service managers |
| `duowave update [--check]` | look for a new version in the feed and install it (restart the gateway after) |
| `duowave version` | the version |

The developer's commands (`src/tools`; the Node gateway's `pnpm replay`, `journal`, `animations`, `call-sim`,
`livekit-token` with the same options and output), the configuration read as for `serve`:

| Command | What it does |
|---|---|
| `duowave replay <file.in.ogg> [--no-asr] [--wav out.wav]` | a recorded request (`audio/` of a session's journal) through the speech pipeline: the same decoding, gain, VAD and end of utterance as live, plus recognition — interim, final and all audio after the wake word; `--wav` writes what recognition got. Settings from the environment: `VAD_END_SILENCE_MS=900 duowave replay …` |
| `duowave journal [--all]` | the session journal live (`RECORDER_DIR`), one line per event without the routine; `--all` prints every event, the station's side connections too |
| `duowave animations [journal dir] [content dir]` | light animations Yandex sent in relay mode, from the journals into `CONTENT_DIR/animations/<time>_<hash>.b64` for the LED skill |
| `duowave call-sim [--url ws://127.0.0.1:<PORT>/calls/ws] [--device default] [--play voice.wav] [--record heard.wav] [--answer-after 1500] [--call "Мама"]` | a station for our calls without the station: joins the calls hub from this computer, answers (or calls `--call`), plays the WAV (a tone without one) as its microphone and writes what it heard. PC build only |
| `duowave livekit-token --name "Мама" [--days 30]` | a LiveKit room access `{"url","token"}` for an app or the LiveKit page («Или вставьте доступ»), the same as `GET /calls/livekit/token` gives at home; LiveKit as the panel set it up (the gateway's database). PC build only |

The station's build has `replay`, `journal` and `animations`; it has no calls, so neither `call-sim` nor
`livekit-token`.

## Builds

| Build | Command |
|---|---|
| this computer | `cargo build -p duowave --release` |
| Linux x86_64, static | `cargo zigbuild -p duowave --release --target x86_64-unknown-linux-musl` |
| Linux aarch64 (Raspberry Pi), static | `cargo zigbuild -p duowave --release --target aarch64-unknown-linux-musl` |
| the station (aarch64, the smallest) | `cargo zigbuild -p duowave --profile station --target aarch64-unknown-linux-musl --no-default-features --features station` |
| Windows | in CI (`.github/workflows`); locally only checked: `cargo zigbuild -p duowave --target x86_64-pc-windows-gnu` |

`cargo zigbuild` needs zig and cargo-zigbuild (`brew install zig cargo-zigbuild`) and the targets
(`rustup target add aarch64-unknown-linux-musl x86_64-unknown-linux-musl`). TLS is rustls with ring and the Mozilla
roots built in, so a build needs no system certificates or OpenSSL — the station has neither.

Features: `pc` (default) — the station connect over SSH, our own model servers, the voice lab, calls; `station` — the
build for the station itself, none of those. `silero` (Silero VAD; ONNX Runtime is downloaded at build time),
`webrtc` and `livekit` (calls; LiveKit downloads a prebuilt libwebrtc) add what needs native libraries; without
`silero` the end of speech is heard by energy.

## On the station

The station's own gateway: no computer needed, in a lighter form — relay to Yandex with our commands and skills,
the brain on cloud models, MCP servers over the network, the panel at `http://<station>:<port>/panel`; no local models,
voice lab or calls. The yse package (`../station-client/yap/build.sh`) carries the station build:

```sh
duowave gateway install 8790          # on the station: the gateway as a service, its data in /data/opt/duowave/gateway
duowave connect 127.0.0.1:8790        # the station's voice to its own gateway (a copy and the way back are kept)
```

Every change on a station is the owner's decision (the station rules in the repository's CLAUDE.md).

## Updates

The gateway checks the release feed (`latest.json`, `UPDATE_FEED_URL`, https only; the release build names the
repository's) at start and every `UPDATE_CHECK_HOURS`, offers a newer version in the panel («Дом» → «Обновления») and
installs it only when someone with the right `system.update` agrees: the archive is downloaded, its minisign signature
checked against the keys built in (`updates.pub`), its `release.json` too — the archive is installed only as the very
version offered, built for this platform and newer than the running one, so a feed in the wrong hands cannot take the
gateway back to an older release — the program replaced (the one before stays as `duowave.previous`), and the gateway
restarts as the new one. An urgent release is the exception: the feed's `critical_below` (scripts/release/critical.txt)
makes an older gateway install it by itself at its next check, unless `UPDATE_AUTO_CRITICAL=false`. Inside the desktop
app, in Docker or as a Home Assistant add-on it offers the update and leaves the install to the app (urgent ones go at
once), the image or the add-on.

## Packages and the catalog

Packages of every kind (plugins, apps, characters, themes) live in `PACKAGES_DIR` (`DATA_DIR/packages` by default),
`<id>/<version>/` each. The catalog is one static feed of packages of every kind (`catalog.json`; `PACKAGES_FEED_URL`,
`off` turns it off; a release build may name one with `DUOWAVE_PACKAGES_FEED`). The panel lists it in «Дом» →
«Пакеты», and the gateway installs a package from it as a `.vgp` archive (tar.gz with `manifest.json` and `assets/`)
after checking its size and SHA-256 against the feed; nothing is installed without someone with the right asking.
The feed has no signature yet, so only packages without code (characters, themes) are installed from it. Characters
(«Мозг» → «Персонажи», `/api/v1/characters`) are the first kind: a character's prompt, tones, avatar and voice
snapshots for the voice lab's engines (docs/plans/characters.md).
