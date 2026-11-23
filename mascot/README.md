# The DuoWave mascot

One library for every place the mascot appears: the panel, the desktop app, the sites. It has two layers:

- **The engine** (`src/engine/`) does everything that is the same for any mascot: the mood vocabulary, taps and the
  pointer, sound, the frame loop (paused off screen), which body draws, and the fallbacks. It knows nothing about how
  anyone looks.
- **Characters** (`src/characters/`) say who is drawn. A character brings its look for each mood, its motion (a rig)
  and one or two bodies that draw it. Deevi is the first.

```
host ──moods, taps──▶ engine ──moods, reactions, time──▶ rig ──state──▶ body ──▶ pixels
```

Hosts speak only in moods, so a host doesn't change when the character does.

The library has no words. Names, labels and lines come from the host's own catalog (the panel's i18n, the site's
content), in every language the host has. English is the source; other languages are translations of it.

## Changing the mascot

`src/characters/index.ts` holds the list of characters and `CURRENT`, the one DuoWave shows wherever a host doesn't name
one. To put someone else in Deevi's place everywhere, add them (see «Adding a character»), list them in `CHARACTERS` and
point `CURRENT` at them. Then add their name to each host's catalog: in the panel, one line in `NAMES` in
`panel/src/ui/mascot.tsx`.

A host can also name a character for one place, for example a persona with its own figure:
`createMascot(el, { character: CHARACTERS.deevi })` or `<Mascot character={…} />` in the panel.

The logo mark (`@duowave/mascot/mark`, `src/characters/deevi/mark.ts`) is DuoWave's logo, Deevi's face. It stays when the
mascot changes.

## Use

```ts
import { createMascot } from '@duowave/mascot';

const mascot = createMascot(document.getElementById('stage')!, { mood: 'idle' });
mascot.setMood('listen', true); // true: the character's entrance move and chirp
mascot.dispose();
```

The container needs a non-zero width and height. The mascot puts its drawing inside, absolutely positioned, with a
transparent background; the page paints the stage (`lookOf(character, mood).color` is the mood's colour). In the panel,
use the Preact component `panel/src/ui/mascot.tsx`:

```tsx
<Mascot mood={stage} style={{ width: 160, height: 176 }} />
```

`@duowave/mascot/engine` is the engine alone, for a host that brings its own character and wants none of ours in its
bundle.

## Moods

The vocabulary every character understands (`src/engine/moods.ts`). A character must have a look for `idle`; for a mood
it has no look for, it shows the fallback (and that one's fallback, down to `idle`), so a new character can start small.

| id | Meaning in the product | Fallback |
| --- | --- | --- |
| `idle` | Waiting | — |
| `tender` | Touched | `success` |
| `wink` | Winking | `success` |
| `sleep` | Asleep until called | `idle` |
| `listen` | Listening | `idle` |
| `think` | Thinking | `idle` |
| `speak` | Speaking | `idle` |
| `success` | Done | `idle` |
| `surprise` | Surprised | `idle` |
| `sad` | Didn't work | `idle` |

Besides moods there are two reactions: `tap`, and `wake`. A tap on a sleeping mascot wakes it into `listen`; the engine
does this for every character, and the character only shows it.

## Bodies

A character has a **light** body, always there and shown at once, and may have a **rich** one, loaded on demand (its own
chunk). For Deevi, light is flat SVG and rich is 3D.

`body: 'auto'` (default) shows the light body at once and swaps in the rich one when it has loaded. It stays light when:

- the character has no rich body;
- the container is under the rich body's `minSize` (default 96 px) on either side;
- `maxLive` rich bodies (default 4) already run on the page (a page has room for about 16 WebGL contexts);
- the rich body fails to start (no WebGL) or its chunk fails to load.

If WebGL loses its context later, the light body takes over. `body: 'light'` never loads the rich body; `body: 'rich'`
tries it whatever the size. The handle's `body` property says which one is drawing now. The rig outlives the bodies, so
a swap keeps the mood and the motion: a mascot in mid-hop lands in the other body.

Both bodies stop drawing when the container is off screen (IntersectionObserver) or the tab is hidden.

### Options

| Option | Default | Meaning |
| --- | --- | --- |
| `character` | `CURRENT` | Who to draw. |
| `mood` | `'idle'` | First mood. |
| `body` | `'auto'` | `auto`, `light` or `rich` (see above). |
| `interactive` | `true` | Tap to react, eyes follow the pointer, Enter and Space tap; a body that turns also turns when dragged. |
| `sound` | `false` | Chirps on mood changes and taps (browsers allow sound only after a user gesture). |
| `shadow` | `true` | Soft ground shadow. |
| `effectColor` | the container's CSS `color` | Colour of the effects (Deevi: waves, thinking dots, sparkles). |
| `onMoodChange(id, mood)` | — | After every mood change except the first; a tap waking a sleeping mascot changes the mood too. |
| `onTap(id)` | — | A tap; return `false` to skip the character's reaction. |

### Handle

`setMood(id, animate)`, `getMood()`, `tap()`, `setSound(on)`, `setEffectColor(css)`, `resize()`, `dispose()`, `body`,
`character`. `setMood` returns `false` for an unknown id; `getMood` returns the mood the host asked for, even when the
character shows a fallback for it.

## Adding a character

`demo/sample-character.ts` is the smallest working one: a round body with two eyes, seven looks, a light body, no rich
one. Start from it. A character (`Character` in `src/engine/types.ts`) is:

1. **`id`** and **`colors`** (`body`, `ink`).
2. **`looks`**: a colour per mood; `idle` is required, the rest fall back.
3. **`tunes`** (optional): notes in Hz per mood and reaction.
4. **`rig(ctx, first)`**: the character's motion and state. The engine calls `setMood(id, animate)` with a mood the
   character has a look for, `react('tap' | 'wake')`, and `step(dt, pointer)` each frame; `step` returns the state the
   bodies draw. All motion, springs and randomness live here; `ctx.sound(key)` plays a tune, `ctx.reduced` asks for less
   motion.
5. **`light`**: `{ create(host) }` returning a body: `draw(state, dt, pointer)`, `resize(w, h)`, `setEffectColor(css)`,
   `dispose()`. It puts its drawing into `host.container` and draws only from the state, so two bodies of one character
   can't drift apart.
6. **`rich`** (optional): `{ load: () => import('./my-3d.ts').then((m) => m.create), drag, minSize, maxLive }`. Call
   `host.lost()` if it can no longer draw.

Wrap it in `defineCharacter({ … })` (it checks that the rig and the bodies agree on the state), put it in
`src/characters/<id>/`, list it in `CHARACTERS`, and give it a name in the hosts' catalogs.

## Deevi

The logo mark as a character: a lime rounded body with two eyes.

| File | What it holds |
| --- | --- |
| `characters/deevi/index.ts` | The character: looks, tunes, rig, bodies. |
| `characters/deevi/looks.ts` | Colours per mood, the cheeks, the chirps. |
| `characters/deevi/rig.ts` | The motion for both bodies: poses per mood, hop and squash springs, head tilt, breathing, blinking, the wink number, the waves. |
| `characters/deevi/flat.ts` | Light body: SVG; with the engine, under 8 KB gzip. |
| `characters/deevi/scene3d.ts` | Rich body: three.js, its own chunk, ~142 KB gzip, fetched only when used. |
| `characters/deevi/mark.ts` | The logo mark with its «Hello» scene (`mountDivi`), used in the panel's header. |

| Mood | What you see |
| --- | --- |
| `idle` | The logo pose: two upright eyes, breathing, blinking. |
| `tender` | Arched eyes, blush, a tilt and a sway. |
| `wink` | A ~2.7 s loop: wind-up, a wink with a hop (and sparkles in 3D), back. A tap restarts it. |
| `sleep` | Eyes shut, slow breathing. |
| `listen` | Wider eyes, a tilt, waves coming in. |
| `think` | Looks up and aside, right eye squinted, three dots. |
| `speak` | Mouth moving, waves going out. |
| `success` | Arched eyes, a high hop on entry. |
| `surprise` | Bigger eyes, an «o» mouth. |
| `sad` | Eyes down, a frown. |

Open question: `think` is violet, and in the panel violet means «an agent answers» (DESIGN.md, the Who-Answers Rule).

**Where it came from.** The 3D body is the designer's handoff `duowave-mascot.js` (three.js r128, a global script),
ported on 23.11.2026 to the `three` package (0.187.0) as an ES module, and then split: the motion went to `rig.ts`,
shared with the flat body. The port keeps the picture: hex colours that r128 read as linear go through `raw()`, and
light intensities are multiplied by π (r155 dropped the old light units). Measured against the handoff demo in
Chromium: the eyes match to the pixel, the body comes out about 3 % darker (not visible by eye).

**Geometry.** The logo's: body 160 × 176 with corner radius 68 on the 256 canvas, eyes centred at x 98 and 158, y 124,
eyes 1.4× (brand kit, `DIVI.md`). One rig unit is 80 logo units. The flat body is framed like the 3D camera, so the swap
from flat to 3D doesn't jump: in a 192 × 211 box the two bodies differ by under 3 px.

## Where the mascot is in the panel

| Screen | File | Mood follows |
| --- | --- | --- |
| Sign-in | `screens/auth.tsx` | `idle`; `think` while the gateway checks; `sad` while an error is shown |
| Connecting a speaker | `screens/connect.tsx` | `think` while connecting, `success` when it came, `sad` on a timeout or failure |
| Speaker connected | `screens/connect.tsx` | enters `success` with its move, with the confetti |
| Orbit centre in «Микрофон» mode | `stations/divi.tsx` | the stage: `listen`, `think`, `speak`; `sleep` when offline. No pointer events: a click on the frame shows the question's path |

All of them show `CURRENT`. Small marks (the header, the wizard's step icons, onboarding badges) stay the logo mark.

## Demo

```bash
pnpm --filter @duowave/mascot demo
```

Opens on port 5180: Deevi and the sample character, all moods (an arrow marks a fallback), the three body choices, the
two bodies side by side, the sizes, English and Russian. URL parameters: `?character=sample&mood=listen&body=rich&lang=ru`.

## Checked

- Chromium (desktop): moods in both bodies, light ↔ rich size, no WebGL → light, lost context → light with the mood
  kept, `dispose` empties the container (also before the rich body finished loading), no frames while off screen, the
  sample character with fallbacks and no rich body.
- The still frame lies exactly on the live light body (192 × 211, 300 × 150, 80 × 120).
- Not yet: Safari and the desktop app (WebKit), Firefox, phones.

## The sites

The site (duowave.ai, repository `duowave-front`) lives apart and its CI can't reach this repository, so it keeps a copy
in `vendor/mascot/`: `node scripts/mascot/sync.mjs <this checkout>` there copies `src/` and this README from a commit
(uncommitted changes are refused) and writes `FROM.json` with the commit and a hash of every file; a test there fails if
the copy is edited by hand. Change the library here, commit, run the script in the site.

Two things exist for pages rendered on a server, before any script runs:

- `character.still()`: the light body standing still in `idle`, as SVG markup framed exactly like the live light body.
  The host shows it, then swaps it for the live mascot, which moves into its mood.
- The logo mark (`mountDivi`) brings to life an `<svg>` the server already drew (`DIVI_BODY`, `diviEyes('open')`,
  `data-divi` parts; see the top of `mark.ts`), even one inside a bigger drawing. Colours come from `--divi-body` and
  `--divi-eye`.
