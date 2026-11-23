# Pretend stations on one Mac

This branch's gateway in the station role, several side by side, each on its own address `10.77.0.x`, with a test
Turbo beside them. They let you check a home of several stations by hand without a second real station. Why and how —
`docs/plans/pretend-stations.md`.

The live DuoWave app on the Mac (port 7780), its database and the real station are not touched: pretend stations
announce nothing on the network, and the test Turbo looks for stations only at the `10.77.0.x` addresses.

## How to bring up three pretend stations

1. **Addresses** — once after every Mac reboot, with the administrator's password:

   ```bash
   sudo scripts/pretend/addresses.sh 3
   ```

   Adds `10.77.0.1` (the test Turbo) and `10.77.0.2`–`10.77.0.4` (the stations) to the loopback `lo0`. Only this Mac
   sees them.

2. **Start** — from the branch's root:

   ```bash
   scripts/pretend/run.sh --build 3
   ```

   `--build` builds the branch's gateway and puts a copy in `~/.duowave-pretend/bin/` (later runs can skip
   `--build`). This brings up:

   | Who | Panel | Folder |
   | --- | --- | --- |
   | Test Turbo | http://127.0.0.1:17780/panel | `~/.duowave-pretend/turbo/` |
   | Kitchen | http://10.77.0.2:8790/panel | `~/.duowave-pretend/stations/Kitchen/` |
   | Bedroom | http://10.77.0.3:8790/panel | `~/.duowave-pretend/stations/Bedroom/` |
   | Nursery | http://10.77.0.4:8790/panel | `~/.duowave-pretend/stations/Nursery/` |

   Each folder holds `gateway.log` and the database in `data/state/`. The Turbo's panel opens from this Mac without a
   code. A station's panel asks for the first-setup code, which is in its log:

   ```bash
   grep 'первая настройка' ~/.duowave-pretend/stations/Kitchen/gateway.log
   ```

   The panel comes from `panel/dist`. Missing — `pnpm install && pnpm --filter panel build`.

3. **The terminal** after the start is `duowave pretend`, the "voice" of all the stations:
   - `🔊 Bedroom: …connection code… 47 19, 82 …` — what the station would say aloud;
   - `💬 Kitchen: …` — the station's answer to a phrase;
   - type `Bedroom: what's the weather` (or `2: what's the weather`) and Enter — the phrase goes to that station as if
     spoken aloud.

   `--say` — codes and answers in the Mac's voice too.

4. **Stop** — Ctrl+C stops all the gateways. **Clean sheet** — `scripts/pretend/run.sh --clean 3` (erases the
   stations' and the Turbo's folders, keeps the program). **Remove the addresses** —
   `sudo scripts/pretend/addresses.sh off` or a reboot.

From 1 to 10 stations: `sudo scripts/pretend/addresses.sh 10`, then `scripts/pretend/run.sh 10`.

## The whole check in one command

`scripts/pretend/check.sh` repeats the check that was first done by hand on 23.11, and prints PASS or FAIL per step.
Run it after any change to the home, ears or join code, to see that nothing broke.

```bash
scripts/pretend/run.sh --build --clean 3     # terminal 1: a clean set; leave it running
scripts/pretend/check.sh                     # terminal 2: about two minutes
```

What it does, in order:

1. **Search** — the test Turbo finds the three stations, each with its SSH greeting.
2. **Hub password** — sets Kitchen's panel password with the first-setup code from its log, as after an install.
3. **Turbo joins by code** — asks Kitchen in, reads the code Kitchen "says" (its `🔊` line), types it on the Turbo.
4. **Ear by code** — asks Bedroom in as an ear, types the code Bedroom says.
5. **Ear's voice through the hub** — the hub takes Bedroom's voice by itself (no "Allow", no reboot), and the phrase
   "what's the weather" typed to Bedroom is answered.
6. **Turbo's records** — a minute later the Turbo still has one record of Kitchen and one of Bedroom, each with its
   device (the panel tells the home's stations from those "not in the home" by it).
7. **Home status** at the hub — "voice through DuoWave" and "Turbo on the line" are green.

It talks to the gateways' HTTP API and types into `duowave pretend` through `~/.duowave-pretend/pretend.in` (what the
terminal shows is kept in `pretend.out`). The test password is made once and kept in `~/.duowave-pretend/test-password`
— never in git; the panels take it too. After the check the set stays up, ready to look at in the panels.

## What this checks

- A station hub and its ears: Kitchen is the hub, Bedroom and Nursery are ears.
- "Will become an ear later" in the "Your home" step.
- Joining the home by code: the station "says" the code as a `🔊` line in the terminal.
- Several stations in "Devices" and in "Home status".

Where to start: the test Turbo's panel → first steps → "A home already exists" → Kitchen → the code from the
`🔊 Kitchen` line → then "Devices" → "Make an ear" on Bedroom → the code from `🔊 Bedroom`.

## What this cannot check

- Sound: how a station speaks, hears and wakes on its word. The voice here is text in the terminal.
- The real Yandex: the pretend one answers any phrase with "Yandex: you said «…»".
- Installing DuoWave on a station over SSH: a pretend station's SSH only greets and lets nobody in. A pretend station
  always has DuoWave already.
- Switching the station's voice to Yandex and back, updating and removing DuoWave on the station — a pretend station
  has no firmware files: the panel answers "This is a pretend speaker".
- The Lite 2's RAM and storage, the station's build: the computer's build runs.
- The DuoWave app's window with the code: the test Turbo is a separate gateway, its panel is in a browser.
- How the search hears stations across the whole network: the test Turbo asks only the `10.77.0.x` addresses.

## How it works (briefly)

- `DUOWAVE_ROLE=station` + `DUOWAVE_PRETEND=station` — the gateway in the station role, listening only on its own
  address (`DUOWAVE_PRETEND_ADDRESS`), answering the mDNS question `_yandexio._tcp` on `:5353` and greeting as dropbear
  on `:2222`. "Firmware" files live in its own folder (`DUOWAVE_STATION_ROOT`), without mounts or reboot.
- `DUOWAVE_PRETEND=turbo` — the test Turbo: listens on every address (its panel on `127.0.0.1`), tells the stations
  `10.77.0.1`.
- `DUOWAVE_PRETEND_STATIONS` — on all of them: the station search asks only these addresses. No announcements on the
  network, no questions to the whole network.
- Connections between `10.77.0.x` addresses leave from the gateway's own address: otherwise the Mac would use the
  destination address, and every station would see the others under its own address.
- Without `DUOWAVE_PRETEND` the gateway behaves as always.
