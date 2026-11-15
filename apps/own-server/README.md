# duowave-own-server

The «VPN» app's program (`runtime: process`, station only): a person's own servers — a subscription or links — become
local entrances for the DuoWave gateway, one per connection. The gateway decides who goes through which and who goes
directly (its module `egress`). The plan is `docs/plans/own-server.md`, how apps work with the gateway is «Приложения»
in `docs/plugins/AUTHORING.md`.

Installed from MagicPlace (package `own-server` of the main catalog). Never put on the station by hand.

## How it works

- **One file, two processes.** The control (this crate) speaks the app protocol on `127.0.0.1:$PORT`; the engine is
  [shoes](https://github.com/cfal/shoes) (MIT), pinned to commit `d1742b9`, started from this same file with
  `DUOWAVE_OWN_SERVER_ROLE=engine` — shoes gives the outside only its command line. New settings: the YAML is rewritten
  and the engine gets SIGHUP. It falls: started again after 1, 2, 4 … 30 s. It never outlives the control
  (`PR_SET_PDEATHSIG`, and a look at the parent every 2 s).
- **Links → shoes YAML** (`src/links.rs`): `vless://` (Reality with Vision, TLS, XHTTP, WebSocket), `hysteria2://`,
  `hy2://`, `tuic://` (ALPN `h3` by default), `ss://` (SIP002 and the old base64 form, AEAD and 2022 ciphers),
  `trojan://`, `anytls://`, `wireguard://` and a WireGuard `.conf` text. A link it does not understand is skipped with
  the reason; the rest work.
- **Subscriptions** (`src/subscription.rs`): base64 or lines, name from `profile-title`; read at start, every 6 hours
  and by «Обновить», directly first and then through the connection itself; the last good list is kept.
- **Groups per connection** (`src/config.rs`): each server its own group, `<id>-auto` (url-test of
  `https://www.gstatic.com/generate_204`, every 5 min, tolerance 50 ms), `<id>-choice` (selector, `auto` by default).
  The entrance `mixed` on `127.0.0.1:<port>` (a random login per run) sends everything the choice's way — never
  directly. The Clash API is on its own local port with a random secret, for the control only.
- **Data** (`$DUOWAVE_APP_DATA`, owner-only): `connections.json` — the connections as pasted (not the app's settings:
  the form is one line and gives secrets back masked), `state.json` — choices, ports, the last good subscription, the
  servers' countries. Links never reach the journal or the window.
- **The gateway** learns the entrances by the bus letter `system/network` → `connections`
  (`{ items: [{ id, name, country, url, state, current }] }`) after the engine starts, on every change and every 30 s.
- **Functions** (`POST /call/<name>`, answered within the gateway's 2 s; longer work finishes in the background):
  `status`, `add`, `remove`, `rename`, `select`, `check`, `refresh`. `GET /health`.
- Words for people in Russian and English (`src/i18n.rs`), by the household's language.

## Build

```sh
sh apps/own-server/build.sh                                          # dist/duowave-own-server-linux-aarch64
sh apps/own-server/build.sh ../duowave-packages/packages/own-server  # and into the catalog package: file, version, SHA-256
```

`cargo-zigbuild` and zig, Rust 1.99.1. Small over fast: `opt-level = "z"`, one codegen unit, fat LTO, stripped. Checks:
`cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.

## Licenses

Our code — Apache-2.0. shoes — MIT; its WireGuard part (GotaTun) — MPL-2.0, used unmodified. The catalog package
carries the texts (`LICENSE`, `NOTICE`, `THIRD_PARTY_NOTICES.md`, `docs/licenses/`) and `SOURCE.md`.
