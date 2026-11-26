# Changelog

What a gateway user can see, release by release. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versions follow [SemVer](https://semver.org/).

## 0.19.2 — 27.11.2026

- **After DuoWave asks you something, music and weather that Alice answers work again.** The answer no longer ends
  with "Yandex did not answer" (#2, thanks @salekseev).

## 0.19.1 — 24.11.2026

- **Deevi is a clear bubble now**, with soft light inside in the colour of Deevi's mood, in the app and on the sites.
- **In the app, Deevi's sound waves and thinking dots are coloured** instead of grey.
- **Deevi's 3D waves are no longer cut at the edges of a small box.**
- **After a tab sat in the background, Deevi no longer comes back squashed out of shape.**

## 0.19.0 — 24.11.2026

- **A phrase said into the computer's microphone gets a spoken answer from the computer.** In the "Speaker" mode the
  computer now says its answer aloud, as in the "Microphone" mode, when the home's voice is its own: a model on the
  computer or in the cloud. With Alice's voice the answer is still text, because Alice speaks only through the
  speaker.
- **First steps, "Whose AI answers": a large "Open the DuoWave page" button.** It opens the sign-in page after "Get your
  gift", instead of a small link above the code field. The same goes for OpenRouter.
- **First steps, "Whose AI answers": "Next" waits until DuoWave is connected and says why.** The line under the choice
  says "Next opens once DuoWave is connected. Or choose another AI." OpenRouter, an own key and a model on the
  computer still go on without signing in.
- **First steps, "Your home": "Search again".** A hub switched on later is found without restarting DuoWave.
- **"Connect the home" no longer offers an address the hub cannot reach.** The connection key lists the home network's
  address first, and never an address on the loopback adapter. Tunnels and virtual adapters come after the home ones.
- **Without macOS "Local Network", DuoWave says so.** The journal has a line with where to allow it, and the panel
  shows "macOS keeps DuoWave off the home network". Once allowed, the home is announced again without a restart.
- **A Turbo can be removed from the home on its own page.** "Remove from home" is next to "Check now" on the Turbo's
  "Device" tab.
- **A Mac is named by its family by default**, such as "MacBook Pro computer" or "Mac mini computer", so two Macs in
  one home are told apart. The name the owner gave the Mac in macOS is not used: other homes can see the gateway's
  name.
- **Settings → Home has a "Folders" line.** It opens the "Folders" window: the home folder, what is attached, who sees
  what.
- **On a hub on the speaker, "Music" and "Stories" are in place.** On a new speaker they no longer said the folder
  was missing. The hub's "Folders" says that the home's folders are on the Turbo too, and a Turbo's file shows its
  computer's name.
- **The workshop's MIDI teacher also works on a hub on the speaker with a Turbo.** The teacher runs on the speaker,
  and the Turbo passes on the keyboard's notes. On a speaker without a Turbo it still cannot be installed.
- **The "Where DuoWave runs" slider's English label says "self-hosted DuoWave".**
- **For developers: the gateway is built from crates.** The workspace has `crates/duowave-{base,rights,voice,brain,
  ears,apps,home,pc}` and the program `gateway`. Everything only a computer needs is in `duowave-pc`; the speaker's
  build leaves it out. Build commands are the same. The speaker's program is about 650 KB larger.
- **The gateway's Docker image keeps the program in one layer**, with `setcap` in a build stage of its own.

## 0.18.6 — 24.11.2026

- **The store and added sources have a backup host.** When the host of the store or of a source you added does not
  answer or stalls, the apps and characters come from a copy of the same files elsewhere, checked the same way. The
  copy that worked is tried first for a week.

## 0.18.5 — 24.11.2026

- **First steps wait for the hub on the speaker.** When DuoWave moves onto the speaker and this computer becomes its
  Turbo, the speaker restarts for about 40 seconds. "Who answers" and "Character" now wait for it and say "The speaker
  is restarting — please wait, the step will carry on by itself", instead of "This step didn't open". The invite code
  is applied the first time, and signing in to a model added in this step works.
- **Looking for a home in first steps no longer finds this computer itself.** On a computer with nothing set up,
  first steps said a home already exists, with its hub on this very computer. The search now knows its own gateway by
  its id. Another gateway on the same computer is still found.
- **A found speaker without a name is named by its model**, such as "Station Lite 2", not by the firmware's label
  `YandexIOReceiver-…` or its address.

## 0.18.4 — 23.11.2026

- **A model's limit is said plainly, with the time it renews.** When DuoWave's daily limit runs out, the speaker says
  "DuoWave's daily limit is used up. It renews at 7:00", in the speaker's time zone, instead of "Too many requests,
  try in 368 min". The weekly limit, a ChatGPT plan's limit, money running out on an OpenAI, OpenRouter or Claude
  account and a service asking to wait long are each said in their own words.
- **While a limit is out, the backup answers at once.** The speaker no longer asks the model whose limit ran out on
  every question. The backup model answers straight away, and the first time it says so once: "DuoWave's daily limit
  is used up. Until 7:00 OpenRouter answers." Every 15 minutes the speaker asks the model once more, and as soon as it
  answers, it is back.
- **Without a backup, Alice can take over.** In "Alice + your AI" the speaker asks once: "Hand questions to Alice for
  now?". "Yes" gives this and the next questions to Alice until the limit renews, "no" keeps saying when it renews.
  The answer is kept, and it can be changed in Home next to the backup.
- **DuoWave's card shows what is spent.** "Used today: 81%, renews at 7:00" and "This week: 61%, until 28 November",
  yellow from 80%, red at 100%. At 90% of the day the speaker says once after an answer that the daily limit is almost
  used up. The cards of OpenAI, Claude and OpenRouter link to their usage pages.

- **A speaker a Turbo took into the home as an ear stays an ear.** The panel no longer lists the hub and that ear under
  "Speakers not in the home yet".
- **An ear's voice reaches the hub at once.** The hub lets in the voice of its own ear without waiting for the admin's
  approval. A speaker that becomes an ear, or a hub again, reconnects to the hub right away, not at its next restart.
- **A Turbo joined by the code knows the hub's and the ears' devices.** The panel no longer offers "Join the home" for
  them.
- **Deevi, the DuoWave mascot, shows how DuoWave is doing.** On the sign-in screen and in the speaker connection wizard
  Deevi thinks while you wait, jumps for joy when the speaker is connected and is sad when something fails. In the
  centre of the orbit, when you talk through the computer's microphone, Deevi shows whether DuoWave is listening,
  thinking or speaking, and sleeps when there is no network.
- **Deevi is 3D where there is room and flat where there is not.** The 3D part loads only when it is shown.
- **The DuoWave mark in the panel's header no longer stretches its eyes into lines when a frame comes late.**
- **The MIDI keyboard teacher speaks English.** The workshop template (`--midi`) now has two languages: the lesson
  window, the widget, the tune names ("Ode to Joy", "Für Elise"…), the speaker's answers and English phrases ("let's
  learn Jingle Bells"). The language is the panel's and the home's, as in the other apps. The "My own tune…" button
  hides where nothing can read `.mid` files.
- **Web host: DuoWave apps on a web page.** An app's window and its `backend.js` run in an ordinary page without the
  gateway: storage is in the browser, settings are the defaults, and the keyboard comes through the browser's Web MIDI.
  The script `panel/scripts/web-tools.ts` builds the host and copies the packages that do not ask for models, the
  speaker's voice, gateway services or links to other apps. The others do not go on the site (`docs/plans/web-host.md`).
- **Pretend stations for checking a home of several speakers on one Mac.** With `DUOWAVE_PRETEND=station` (or `turbo`)
  the gateway takes the speaker's role on its own address on the Mac, several side by side. `duowave pretend` stands in
  for the speaker's firmware and Yandex: you type the phrases and read the answers (`scripts/pretend`,
  `docs/plans/pretend-stations.md`).
- **`scripts/pretend/check.sh` runs the multi-station home check on pretend stations in one command:** search, the hub
  password, a Turbo by code, an ear by code, the ear's voice through the hub, the Turbo's records and the home status,
  with PASS or FAIL for each step.

## 0.18.3 — 23.11.2026

- **DuoWave installs with one command right on the speaker, with no computer.** Log in to the speaker over SSH and type
  `wget -qO- http://get.duowave.app/station.sh | sh`. The speaker downloads the latest release build, checks the
  checksum, shows what it will change and installs DuoWave after your "yes". At the end it asks for the panel password
  and whether to send the speaker's voice to DuoWave. This way is shown in Where DuoWave runs and in First steps, next
  to installing from a computer.
- **The panel's first-setup code shows at the end of the install.** You no longer need to look for it in the speaker's
  log. Installing on the speaker (`duowave station install`, which the YAM package and the one-command install run)
  prints the code while the panel has no password. Installing from a computer (`duowave station setup`) waits until
  the speaker lets it in over SSH again and writes the code. If there is still no code, both show the last lines of
  the log. `duowave station code` on the speaker shows the code again.
- **A computer's devices are available to the whole home.** A MIDI keyboard, the screen, flash drives and the DuoWave
  folder on a Turbo computer are now seen by the hub. Apps, the workshop and Thinking use them like their own, and you
  can ask "what is plugged in?" by voice. With several Turbos, looking and listening work on all of them at once, and
  an action runs on one: the one used last. The speaker says which one the first time and when it changes, and saying
  "at the PC" changes it.
- **Devices has a "Plugged into computers" section.** Things are grouped by kind, and each one shows its computer:
  "Keystation 49 · Mac". A sleeping computer is shown in grey, with a "Wake up" button next to it. You also choose here
  where to run things.
- **"What this computer gives the home".** On the This computer page, each kind of device has its own checkbox. The
  home gets a camera, a microphone and keyboard-like devices only if you turn them on. Another home from "Who I help"
  gets no devices or folders by default.
- **A sleeping Turbo can be woken up.** If the keyboard is at the Mac and the Mac is asleep, the speaker says so and
  asks "Wake it up?". The answer is understood by its meaning. The hub wakes the computer over the home network, and if
  it does not wake up, asks you to wake it by hand.
- **The workshop makes a MIDI keyboard teacher even when the keyboard is not at hand now.** The speaker says where the
  keyboard is and when the teacher will work.
- **A Turbo away from home: folders are visible, devices are not.** If the computer is online from outside the home
  network, its folders are available, but its keyboard, screen and flash drives are not.
- **A Turbo's flash drives are visible from the hub in the home's folders, under "Connected",** read-only.
- **The speaker takes through the computer the feeds it cannot open itself.** If the speaker cannot reach DuoWave
  Store, the demo feed or your own source, but the computer at home can read them, the speaker takes a copy of the feed
  and the characters' archives from the computer. Before, only the main catalog came through the computer, and a
  tester with a Station Mini 2 could not install the demo characters.
- **Instead of an endless "preparing on the speaker", the panel says why a character does not install.** Tap the pill
  to see the cause and choose "Don't install" or "Wait". It retries by itself every two minutes.
- **The journal shows the real cause of a network error in a feed or a download:** "could not connect (Connection
  reset by peer)", "no answer in time", not just "error sending request".
- **DuoWave asks for the invitation code before the sign-in on the site.** This is so in every place where DuoWave is
  offered: "Who thinks", "Who answers on the speaker?", the desk showcase, "Thinking" → "Add" and a DuoWave card you
  have not signed in to yet. The invitation gift comes only with the first sign-in, so you enter the code before it.
  The code is accepted in any form: without `s_`, with `S_` or as the whole link.
- **After signing in to DuoWave, it says "Trial week", with no date.** The gateway keeps the access type that the
  account page sends.
- **First steps without "Have an invitation?":** the demo characters appear by themselves, with no link to paste.
- **The desk showcase always keeps one look:** three tiles before and after the AI is connected, with no "Alice is
  smarter now" card. If the "Alice + your AI" mode turned on by itself, "Talk" shows "DuoWave connected" and "Undo".
- **"Home status" on the desk no longer covers the speaker's ring.**
- **"Who answers on the speaker?" fits a window 1100–1300 points wide:** the button moves to its own line.
- **If the sign-in does not come back to the panel by itself, the hint says that the page's address and the code from
  it both work.**

## 0.18.2 — 22.11.2026

- **Windows: the app starts again after the update.** In 0.18.1 the gateway on Windows refused to start with the error
  "migration 1 (init) was changed after it was applied": Windows builds before 0.18.1 kept the checksum of the
  migrations' text with other line endings. Now line endings do not count; the database is not touched and opens as it
  is.
- **The "Make Alice smarter" step takes a key of an unknown service.** If the service is not recognised by its key, you
  can enter its address right there. Any working model counts for the step, including one added in Settings.
- **The mode changes right from the "Thinks" window.** In the "Alice" mode, choosing a model switches the speaker to
  "Alice + your AI" with one button.
- **"Brain" is now called "Thinking"** everywhere it is seen and heard.
- **A computer that joins a home already set up does not go through First steps again.** "Your home: what works"
  opens at once.
- **First steps and the desk say what is not ready yet:** what is installing now, that your own word will work after
  the "Thinking" step, and that the character's voice will be on the Turbo computer, while Alice answers for now.
- **Joining by a code explains errors.** An empty or short code: what to enter and where to get it. A wrong one: how
  many tries are left and what to do after the third.
- **"Home status" on the desk folds into one line.** A line that waits for a choice can be left as it is: it turns grey
  and no longer keeps the card on the desk.
- **Cloud DuoWave is shown to everyone:** first in First steps, on the desk showcase and in "Add".
- **The demo characters are available at once,** without adding a source by hand.
- **The desk has no lock by default.** Shift + wheel moves the desk sideways, and the background drags on a locked desk
  too.
- **Close buttons in a narrow window.** The Turbo card under the speaker and "Request flow" fold with a close button in
  a window of any size.
- **The desk showcase does not shrink** next to the "Connect your speaker" card, and folds into a strip when space is
  tight.
- **The workshop does not make everything neon.** If no look is given, it picks one itself, a different one each time.
  A game starts from a clean sheet, and examples are offered to choose from instead of one template for everyone.
- **The character's backup voice says whose it is and why it is installed:** "Rick: installing a backup voice", with
  the reason.
- **Pause in the panel stops the apps' sound**, such as YouTube, in the same way as the radio.
- **The Turbo's voice failed: the speaker speaks in Alice's voice instead of staying silent.** If the Turbo computer
  could not voice a phrase, the speaker says it in Alice's voice. For a couple of minutes after a failure it does not
  wait for the Turbo and answers in Alice's voice at once, then tries the Turbo's voice again. Before, every answer was
  lost in silence.
- **Windows does not let the character's voice run, and DuoWave says so.** On a new Windows 11, Smart App Control does
  not let DuoWave's voice files run. "Home status" says this plainly: you can keep Alice's voice or turn the protection
  off, and it warns that Windows protection will be weaker. The computer itself has an "Open Windows settings" button.
- **A home can have several Turbos.** Every computer that joins the home by a code gets its own entry on the speaker.
  Before, a second computer took the first one's place: both wrote their address to the speaker in turn, and the Turbo
  kept appearing and disappearing. Each job (hearing, voice, model, messages) goes to the first Turbo in the list that
  is online. When the first one sleeps, the next one takes the job at once. The order changes with the arrows in the
  list of Turbos.
- **"Home status" about several Turbos.** All online: one green line with their names. One offline while another
  helps: a warning that says who helps. The "Allow in the firewall" button appears only when a Turbo itself reported
  that its firewall closes it, and only on that computer. Before, it stood by any "Turbo offline" line, even on a Mac.
- **The "Allow in the firewall" button in the home window on Windows works again.** Its request went to the speaker
  instead of the computer itself, and nothing happened.
- **Plain words when a Turbo does not recognise the speaker's key.** Instead of "make a new key in "Access" → "Keys"",
  the message says where on the computer to turn helping the speaker on again. The speaker's entry carries the name of
  the computer that is in it now, not the previous one.
- **A speaker without internet says "My internet is gone" again.** The phrase is recorded in the voice that answered
  while there was a network. When the Wi-Fi went, the Turbo went with it, and the speaker moved to Alice's voice. It had
  no recording in that voice, so it stayed silent. Now it says the phrase in the voice it was recorded in.
- **The speaker notices that its Wi-Fi is gone.** A Lite 2 keeps a service Wi-Fi Direct route without a router, and the
  gateway took it for a way out to the network: lost Wi-Fi did not reach "Home status", and the speaker waited for an
  answer from Yandex. Now only a route through the router counts as a way out to the network.
- **The main device of the home is now called the "hub"**; the old word "centre" is gone from the panel and from the
  gateway's messages.
- **Simpler First steps texts.** The descriptions in the "Your home", "Who thinks", "Character" and "MagicPlace" steps
  and in connecting the speaker are rewritten as plain sentences. The root hint called a button "Only the computer",
  but it is called "Hub: this computer"; "Found 5 more speakers" agrees with the number.
- **The speech recognition and voice model cards are fixed.** The GigaAM model was called "GigaAM v3, end to end", but
  its real name is GigaAM v3 e2e RNNT. Its author was listed as "Sberezhenie (Sber)", now just "Sber". The descriptions
  are rewritten in plain words. In the English panel the cards used to stay in Russian; now they are translated.
- **Windows: a work in a folder makes its own copy.** The folder's path was kept in the form `\\?\C:\…`, and
  `git clone` did not accept such a path: the work ended at once with a git error. Now the path is kept in the plain
  form `C:\…`.
- **For developers:** the MCP and voice engine install tests compare paths the way Windows writes them.

## 0.18.1 — 22.11.2026

- **The connect wizard recognises luna and the AliceBridge agent.** luna is an environment for mods and does not take
  Alice's voice: the wizard gently warns that its mods can cut answers short, DuoWave's answers too. The AliceBridge
  agent (root from a phone) is also only a warning; if it is stuck switching Wi-Fi (the speaker drops off the network
  every 40–50 seconds), the wizard says plainly what to do. The wizard also says if AliceBridge sets the speaker's DNS
  and if root lets SSH in with the default password "1". The network name and Wi-Fi password from the agent's settings
  never leave the speaker.
- **Icons and voice samples in the store that did not load the first time load by themselves.** Before, a picture that
  was not downloaded on the first read of the feed (a network error, a slow answer) did not appear until the feed
  changed: only removing and adding your own source again helped. Now the gateway asks again for what is missing on
  later feed checks, but not more often than every 15 minutes, and for a file the site does not have or that does not
  fit, only after a new version of the app.
- **Windows: "the way in to the computer is closed" is no longer a false alarm.** Each of the computer's networks is
  checked against its own firewall profile: a "Public" network where the firewall for public networks is off or
  DuoWave is allowed does not count as closed, even if the domain profile is on. Before, any profile that was on and
  any "Public" network were enough: the gateway advised "make the network private" and showed the button, although the
  computer was open.
- **The speaker dropped off the network: the computer does not wait for it on every request.** After the first failure
  the panel on the computer marks the speaker "offline since 14:05" and shows its copy at once, read-only; the computer
  itself checks the speaker every 5 seconds, and the note goes as soon as the speaker answers. The log has one line
  when the link goes, with the reason ("Connection refused", "Network unreachable", no answer), and one when it is
  back, with how long it was down. Before, every panel request waited 3 seconds for the speaker and wrote "error
  sending request" to the log with no reason.
- **A speaker without internet says so at once.** While there is a network, the speaker records, in the voice it
  answers with, the phrase "My internet is gone. I'll be back as soon as it returns." When there is no internet (the
  Wi-Fi is gone or the router lost its connection, and the link to Yandex does not open) and a request was not
  recognised or did not reach Yandex, the speaker plays this recording instead of silence after 5–10 seconds of
  waiting. "Stop" while a timer or an alarm rings works as before. In the "Alice" mode Yandex itself answers, and
  everything is as before.
- **"Home status": the speaker's Wi-Fi.** The speaker watches its own network, sending nothing anywhere, and remembers
  the drops for three days. If the Wi-Fi went at least three times in a day or for longer than 5 minutes, "Home status"
  shows a line: how many times and how many minutes without a network, and the likely cause if the speaker can see it:
  an AliceBridge agent stuck switching Wi-Fi, a weak signal or little free RAM.
- **A faster first word: a shorter request to the model.** The model's tool descriptions are shorter and written in
  English, and the gateway's own weather and news give way to the tools of the "Weather" and "News" apps when those are
  installed: the request is smaller by about 700–800 tokens of 3600 (speakers waited 4–5 seconds for the first word).
- **The answer is in the language of the question,** even when the tools and the data are in another language.
- **Built-in apps with short English tool descriptions:** Calendar 2.1.2, Exchange rates 2.1.2, Folders 1.0.6, News
  2.0.2, Radio 2.1.2, Shopping list 2.1.2, Smart home 2.0.2, Weather 2.1.2. App authors get the same rule in
  `docs/plugins/AUTHORING.md` and in the workshop.
- **The chosen theme is in all of a person's windows.** A theme chosen in the browser now reaches the app and the phone
  of the same person by itself; different people have their own themes. Light, dark or "Auto" stays with each device.
- **Windows: a disconnected folder leaves "Connected".** Windows makes a link to a folder a directory, and before it was
  not removed: the folder stayed in "Connected", and a moved link was not set again.
- **Windows: LM Studio on this computer no longer looks switched off.** `localhost` on Windows tries IPv6 first, and the
  refusal came later than the check ended; now all addresses are checked at once.
- **Windows: apps see the computer's RAM** in `system/device` (an app that read it crashed), and so does the apps'
  budget; and the free disk storage too.
- **For developers:** the tests find the `duowave-test-stub` stand-in with cargo 1.100 too, which puts the tests in
  another folder; Windows CI runs the tests that used to fail on Windows.

## 0.18.0 — 22.11.2026

Not released on its own: everything below went into 0.18.1.

- **The network changed: the Turbo is found again by itself.** A computer whose address changed (another Wi-Fi, a new
  IP) tells it to the speaker that names this computer its Turbo, even if the speaker got its home key from someone
  else (before, the speaker kept calling the old address). A hub on a computer that lost the link to its Turbo looks
  for it every 2 minutes by its announcement on the network (`_duowave._tcp`, by its id), without scanning the subnet,
  and moves the link to the new address; the pinned certificate still proves it is the same computer.
- **A computer finds a home whose hub is another computer or a server.** DuoWave on a computer and on a server
  announces itself on the home network (mDNS `_duowave._tcp`: role, port, version, home), and First steps offer to join
  such a home by a code, as a speaker's home; a computer hub connects the Turbo with its link key by itself. First
  steps no longer take an ear speaker for the hub and offer its hub instead. Speakers do not announce themselves: they
  lack the RAM, and they are found as before.
- **A Turbo is the home's helper, not a second hub.** A computer that helps another home by a link key and leads no
  speakers of its own is shown in the device row as that home's Turbo: "Computer · Hub · Turbo" on one device no
  longer appears. On a Turbo, the computer's own desk is in the account menu: "This computer's desk", and back to "The
  home's panel". The DuoWave window on such a Turbo opens the home's panel, even when the hub is another computer or a
  server: the panel's requests go to the hub over the same link, on behalf of whoever signed in on the Turbo, with
  their role. The hub lets in this way only a Turbo of its home from its own list, and only to the panel's API; a hub
  with an old DuoWave leaves the Turbo its own desk.
- **More speakers become ears with one button.** A Turbo computer shows in "Devices" the speakers on the network that
  are not in the home. A speaker without DuoWave: "Make it an ear". The computer installs DuoWave over SSH right as an
  ear of the hub; the computer's settings do not go there, and no code is needed. A speaker with its own DuoWave: "Join
  the home". First comes a warning that its settings and characters will be replaced by the home's, and its memory
  merged; the code is said by that speaker itself (or "Allow" in its panel). Then we ask only for the room. In First
  steps the extra speakers are marked "will become an ear later".
- **"Your home": what works and what to fix, in one summary.** The last step of First steps on any path, and a card on
  the desk until everything is green: the speaker's voice goes through DuoWave, the brain answers (or why not, for
  example the key no longer works), the Turbo is online, answers sound in the character's voice, who answers. Each line
  has one button that leads to the screen that fixes it. A Turbo on Windows tells the hub itself when its firewall or a
  "Public" network closes the way in, and the line says so instead of just "offline". Speakers on the network that are
  not in the home are one more line.
- **A Turbo joins the home by a code, with no panel password or key.** A computer that finds a speaker with DuoWave on
  the network asks it to let it in. The speaker hub makes a 6-digit code (5 minutes, once, 3 tries) and says it aloud
  twice in a row, in pairs of digits, when its session can speak. In the "Alice" mode too: the hub asks Yandex for the
  phrase over the speaker's own connection and plays it on the speaker (if that fails, the code stays on the screens).
  An admin sees the same code with "Allow" and "That's not me" buttons in the home's panel on any screen, the phone
  too; on a computer hub the code comes as a notification, on a server as a line in the terminal. After joining, the
  help starts at once, and First steps show "What this computer gives the home": hearing, voice, model, messages, "Keep
  the computer awake", the link with the speaker and "Allow in the firewall" on Windows. A speaker with an old DuoWave
  does not know the code; then, as before, its panel password is used.
- **A move no longer overwrites the home on the speaker.** If the speaker has its own DuoWave home with people (for
  example, one installed from the terminal), a move from a computer stops at the check and changes nothing. Such a home
  can be replaced only on purpose: with the "Replace the speaker's home" button and a warning of what will be lost. A
  home that this computer itself moved to the speaker, and a speaker without people, are not affected. In First steps,
  "This is a new home" now means a home on this computer: the steps neither connect nor touch a speaker in another
  home.
- **The panel password at the end of the one-command install on the speaker.** After "Done" the installer asks you to
  make up a panel password (twice, the characters are hidden) and sets it on the speaker itself: you sign in to the
  panel with it at once, without the first-setup code from the log. An empty answer, `--yes` or a run outside a
  terminal works as before: the installer shows the code. At the end: the panel's address and a hint that to let the
  computer help the speaker, you open the DuoWave app.
- **`duowave panel setup`: the first panel password from the device itself.** The name and the password come as two
  lines on stdin (in a terminal the command asks for them itself, the password without echo); this device's gateway
  takes them from 127.0.0.1, without a code. The password never goes on the command line or into the log. If a
  password is already set, the command refuses.

## 0.17.3 — 21.11.2026

Not released on its own: everything below went into 0.18.1.

- **The "Version available" line on the home screen also covers this computer.** In a home whose hub is a speaker, the
  line looked only at the speaker: the computer, which updates first and then hands the version to the speaker, had no
  line of its own, and when the speaker was offline there was no line at all; the new version showed only in the tray
  and in "Updates". Now the panel also asks the computer itself (every 5 minutes, without going to the release feed)
  and says "for this computer, it updates first"; the dot on "Home" lights up for it too.
- **Windows: the gateway knows how much RAM the apps' programs use and how much disk storage is free.** Before, these
  numbers were missing on Windows: the apps' RAM limits did not work, and the apps' budget and the device state went
  without them.
- **Windows: "Stop" on a work in a folder ends Claude Code, the ACP agent or the folder's command together with
  everything they started.** Before, it sent nothing, and commands such as `npm test` kept running and holding the
  copy's files. The ACP agent's rights: a path without a drive (`/etc/hosts`) no longer counts as a path inside the
  copy.
- **Windows: Russian network names no longer show as "????"**, in the hint to make the network private and in the
  speaker search.
- **A gateway built on Windows hands the speaker its scripts with the right line endings** (LF): with Windows line
  endings the speaker would not run any of them.
- **For developers:** Windows CI runs the gateway's tests, not only builds them; the stand-ins for outside programs in
  the tests are one Rust program, `duowave-test-stub` (not part of the release).

## 0.17.2 — 21.11.2026

- **Apps see the device state.** A new service `system/device` (the right `see:system/device`): the temperature, free
  RAM and storage, processor load and frequency, the Wi-Fi signal and the uptime of the speaker and the computer, their
  names and roles in the home, and a few hours of history for a chart. "Make a chart of the speaker's temperature": the
  workshop builds a widget that updates itself. A computer sees the speaker, and a hub speaker sees its Turbo (both
  need this version). A Mac has no temperature, and the widget says so.
- **"Allow in the firewall" right in the speaker wizard on Windows.** When no speaker is found on the network, there is
  no route to the speaker, or it has not reached the hub within 3 minutes, both wizards ("Connect a speaker" and "First
  steps") show the button: one rule for DuoWave on private networks, and Windows asks for rights. After it the wizard
  looks for the speaker or checks it again by itself; if the Windows network is "Public", it says what else to do.

- **Your own root password for the speaker, right in the wizard.** "The speaker did not let SSH in" shows a password
  field; DuoWave keeps the password encrypted and signs in with it later (connecting, returning to Yandex, removing). A
  wrong one is not saved.
- **Windows: replacing a voice no longer fails with "os error 32".** A file the voice server holds goes as soon as the
  server lets it go, or at the next start; it no longer counts as installed.
- **The speaker could not reach the Turbo computer: now both panels show it.** On the computer, the speaker's card says
  what the speaker answered and what to do; on Windows there is an "Allow in the firewall" button (one rule for private
  networks, Windows asks for rights) and a hint if the Windows network is "Public". On the speaker, a computer that is
  "offline" says what to check.
- **Speaker search on Windows** says why it found nothing (the firewall, a public network), and the computer's address
  for the speaker is taken from the real home network, not from a WSL, Hyper-V or VPN adapter.
- **Yandex's voice "every other time":** the connection to Yandex tries IPv4 first, so where the speaker has no IPv6 it
  no longer fails with "Network unreachable".
- **The dock mic on a phone no longer stays silent.** A panel opened over the network (`http://<speaker>`) does not get
  the microphone from the browser: the button is dimmed, and a press says that on a phone you speak to the speaker
  directly, and the panel's mic works on a computer with DuoWave.
- **A report for the developer:** "This computer" → "Collect the report": the version, the system and the end of the
  log, without passwords, keys, the home folder, the name or the email; one button copies it.
- **A log for sorting out other people's problems:** the speaker search (adapters, what was found), each wizard step,
  the link with the Turbo both ways with the address and the error, voice server crashes with the exit code, operations
  on model files, the connection to Yandex with the IPv4/IPv6 addresses and the error. On the speaker, `voice.sh`
  records every voice switch: when, what, and who called it (`voice.sh history`; also in "What happened" on the
  computer).
- **An app's RAM budget follows what it uses.** A manifest can declare more memory for what its program names in its
  `/health` answer (`budget.memoryMbFor`, the program's `uses`), and the supervisor keeps the app in the budget of what
  it uses now. So an app that usually needs less also fits on a Station Mini 2. If the speaker has no room for more,
  the budget does not grow, and the app can say what to choose instead.
- **Widgets and apps on the speaker count in the budget what they hold.** An idle loaded context counts what its engine
  really holds, with a margin; during a call, its declared limit. Before, each one held its whole limit in the count,
  and on a Station Mini 2 widgets did not load: "needs 2 MB, 0 free".
- **The RAM reserve for the system and Alice follows the speaker:** a sixth of its memory, from 24 to 40 MB (on a
  Lite 2, the same 40 as before). `PLUGINS_RESERVE_MB` still sets it directly.
- **A refusal to load an app says what is missing:** "X of Y MB free for apps; in use now: Radio 11, Weather 2", or
  that apps have little room at all, and the rest is taken by the system and Alice.

## 0.17.1 — 21.11.2026

Not released on its own: everything below went into 0.17.2.

- **The speaker connect wizard says why there is no SSH.** Instead of one "SSH on the speaker does not answer": the
  speaker does not answer (is it on? on the same network?), SSH is not running on it (the port is closed), there is no
  route to the speaker from this computer (a VPN, a firewall, an antivirus), or the port is open but SSH did not answer
  or closed the connection at once. It waits up to 5 seconds for SSH to answer (was 1.5), and if the port is open, the
  wizard still tries to sign in. The reason is in the gateway's log too. If the speaker did not let root in without a
  password, the wizard says where to put your own: the line `STATION_SSH_PASSWORD` in the `.env` file of the app's
  data folder (for Windows and Mac).
- **Speech recognition on a Windows or Linux computer no longer crashes on processors without AVX-512:** the engine is
  built for a common level (AVX2), not for the build machine's processor. On a processor without the needed
  instructions, the panel says what is missing. If the recognition worker still crashes, the gateway's log has its exit
  code and what it managed to write.
- **No false alarm "The speaker has not been connected to the hub for 5 min" on a new install:** the alarm is only
  about a speaker that was connected before. Instead of a script from the repository, the advice is "Speakers" →
  "Return to Yandex".
- **The one-command install asks for the speaker's root password itself** if signing in without a password failed:
  what you type is hidden, three tries, an empty answer quits. It reads straight from the terminal, so it also works in
  `curl … | sh`. `STATION_SSH_PASSWORD` works as before.
- **"Devices" on a hub speaker:** the hub no longer offers to connect itself to the home, and its own voice is not a
  separate "speaker" but a note on its row: "voice offline".
- **The first-setup code also works without the dash:** `79188906` is the same as `7918-8906`.
- **"Add a Turbo"** leads to where the key really is: on the Turbo, "Connection key" → "Connect a home".
- **A window on the computer's page inside the speaker's window on Windows** ("Voice file needed" and others) no longer
  covers the middle of the screen with a black column: the window sits in the middle, and the page stays in place.
- **`install.ps1` in Windows PowerShell 5.1 prints Russian text without garbled characters;** the speaker's address can
  be given in advance with `$env:DUOWAVE_STATION`.
- **"Voice" → "Speaking now" without a speaker** no longer says "the speaker itself speaks": in a home with only a
  computer it shows the computer's voice or "No voice yet".
- **Stopping the gateway on the speaker no longer hangs:** a radio stream that goes through the gateway does not hold
  the exit; the gateway exits within 12 s, and `gateway.sh stop` waits for it and stops the apps' programs. The new
  `gateway.sh` reaches the speaker only with an install (from the panel, `duowave station install` or YAM), not with a
  self-update; with the old one the gateway still exits within 12 s.
- **A voice download is visible.** A normal download is a calm pill by the dock, "<Character>: installing <model> ·
  N %"; the dot and the window appear only for "Memory is tight" or "failed". The card under "Voice" names the model,
  and "Now: …" names the one that really installs on the Turbo. "First steps" keep the voice model choice until the
  character is installed (the Turbo no longer installs another one) and show the progress of the model that really
  downloads. The last choice wins: a choice per package and a choice per character replace each other, and a removed
  package takes its choice away both in the home and on the Turbo.
- **MagicPlace: an app for both the speaker and a computer is marked "on the speaker" on the speaker's gateway,** not
  "needs a Turbo computer".

## 0.17.0 — 20.11.2026

- **"Update the computer and the speaker" with one button.** When a release is newer than the computer, the speaker
  updates right after it: the computer installs the new version for itself and hands it to the speaker as soon as it
  starts. Instead of "The speaker has the latest version", the panel says what stands in the way of the update and what
  to do. This works once the computer itself runs 0.17.0. Until then it is as before: first the computer from the tray,
  then the speaker with its button.
- **The computer installs urgent fixes on the speaker by itself,** with no button and even without "Update
  automatically" (`UPDATE_AUTO_CRITICAL=false` turns this off).
- **"Install without the computer" in the speaker's panel,** for when no computer is around. The speaker's own update no
  longer ends with "An update is already being installed".
- **Apps on a Turbo:** an update does not load the speaker, the Turbo keeps up with the hub, and the app's card says
  when the Turbo could not install it. In "Updates and backups" of the home window there are two cards: the speaker and
  "This computer".
- **Apps speak with a recording.** An app can give the speaker not only words but also an Ogg/Opus recording from its
  own folder (up to 2 MB). It plays after the words, like a voice message.
- **"Do not disturb" by voice keeps apps quiet too.** While the mode is on by voice, apps say nothing by themselves; the
  speaker still answers questions. The gateway cannot see the mode when it is turned off with the speaker's button or in
  Yandex's app, so the remembered "on" lifts after 12 hours.
- **Sign-in to a model provider can set up the models by itself;** clearer messages when a sign-in or a plan limit
  fails.
- **Every new package gets a random id** such as `p-3f9a01c27b5e`: `duowave plugin new`, the workshop's drafts and the
  "New plugin" form (it no longer asks for an id). The package's name is in its title.
- **The built-in apps have new ids, with no twins.** The gateway removes the built-ins of earlier builds by itself at
  start; their settings and records (the shopping list, the calendar, the weather's city and others) begin anew. Your
  own copies ("Make your own copy") and apps from MagicPlace are left alone. Packages from MagicPlace installed before
  0.17.0 get no more updates: remove them and install them again.
- **Installers also under names that stay.** On the release page, next to the versioned files, there are
  `DuoWave-macos-apple-silicon.dmg`, `DuoWave-macos-intel.dmg`, `DuoWave-windows.exe`, `DuoWave-linux-x86_64.AppImage`,
  `DuoWave-linux-arm.AppImage`: the link `…/releases/latest/download/<name>` always gives the newest installer.

## 0.16.3 — 20.11.2026

- **The microphone in the middle of the dock everywhere.** The green "Say" button sits between "Character" and "Apps" in
  the panel of a computer as Turbo too: since 0.16 its microphone talks to the hub, so there is no reason to hide it
  there any more.
- **"Waiting for you" and the install's progress at the top, by the search.** The badges no longer sit next to the dock:
  at the bottom there are only the sections and the microphone. On a phone, "Waiting for you" is a green circle with a
  number.
- **"No, that's fine" does not stop the work.** After "Doing it: … Anything else to add?", any no ("no", "no need",
  "nothing", "leave it as is", "go on") means "go on": the speaker answers "OK, carrying on.", and the window on the
  computer closes quietly. Only "stop", "cancel" and "don't do it" stop the work. A lone "yes" asks "What should I
  add?", and "and also …" adds to the work; other phrases are handled as usual.
- **"Say": type a text and hear it.** In "Settings" → "Voice" and in search ("Say a text…"): a text and whose voice, the
  character's or Alice's. It plays on the speaker; without one, on this computer in the home's voice (Alice speaks only
  through the speaker). The choice of voice is remembered on the device.
- **One theme for all your windows.** Set a theme in the browser, and it is the same in the app and on the phone. Each
  person has their own theme. Light or dark stays with each device, "Auto" by default, as the system.

## 0.16.2 — 19.11.2026

- **First steps without needless scrolling.** The middle of a step scrolls only when it does not fit in the window, and
  only the part that does not fit; the step's header and buttons always stay in view. The "Try it" scene is never taller
  than the window, and in a low window the steps are tighter.
- **Where an app runs, as a label by its name.** "On the computer" or "on the speaker" in the MagicPlace step of first
  steps and in the store, from the catalog's data.
- **The speaker's installer says why it could not reach the speaker.** `duowave station setup` tells apart a speaker
  that is off or on another network, a speaker without SSH running, and a macOS block: the program was not given access
  to the local network. In that case it says where to turn it on ("Privacy & Security" → "Local Network").
- **The panel's first-setup code at the end of the install.** The speaker's installer prints the code that the speaker's
  panel asks for together with the password. The speaker's panel itself says that the installer showed the code (and how
  to read it over SSH if it is lost), not a Mac's log.

## 0.16.1 — 19.11.2026

- **First steps continue from where they stopped.** The step, the choices, the steps done and what finishes in the
  background (the invitation link and the delivery of the set to the speaker, a speaker connection that was started) are
  written to the panel's memory. After a restart of the app, the gateway or the speaker, first steps open on the same
  step with everything chosen and finish what was started by themselves. "Go through first steps" in "Settings" now runs
  over a home that is already set up: the place stays as it is, what is done is ticked ✓, and nothing is reset.
- **The workshop does not drop a game silently.** If the model's answer hits its length limit before it writes even one
  file (it thought for long or wrote code as words), the agent hears "write the files at once, in parts" and goes on. If
  that does not help, the speaker says "not finished. Continue?" instead of "nothing changed". "It did not work" names a
  plain reason: the "Brain" key ran out of money, the request limit was hit, the key did not fit. "Redo" in the panel
  also carries the first request for the thing to the agent, so it no longer forgets what it was doing.
- **Every widget has its own place on the desk.** When you add or remove a widget, data arrives or music starts, the
  neighbours no longer jump to another column, and a new widget never lands under another one: it takes the first free
  place and keeps it, and a widget that grows pushes down only the one it lands on. "New widget" stands where the next
  one will go. The Turbo card floats over the desk at the right edge, closes with a cross and comes back with the
  lightning icon by the speaker; the labels under the speaker fold into "Request flow" when the desk has your own
  widgets.
- **A new widget comes with data and lands on the desk.** A just-installed app fills its data once, quietly, right away;
  it does not wait for the morning or for its 08:00–22:00 window. A widget put on the desk by voice ("yes" or "on the
  desk" in answer to "Done. On the desk or try it on?") lands on the desk instead of staying as the words "On the
  desk.".
- **The header and "Works" tell the truth.** "Waiting for you" counts only what really waits for an answer: a new
  version, a started "Finish" and a thing put on the desk clear the question from old records too. A tap always opens
  something: "Done" on the bar, everything else the work itself with the answer buttons. A work whose run has ended no
  longer hangs as "in progress · starting": the end of the run reaches all records of that thing, and "Stop" on a work
  that the agent no longer works on closes it. A theme you tried on stays on the panel after "Turn on" or "Apply", and
  "Done · …" at the top right goes away as soon as the thing is installed. The light capsule in the top row is the
  device whose page is open.
- **"Voice" and "Hearing" are their own items in "Settings".** The choice of voice and models is no longer hidden in
  "Devices" → Mac → "This computer". "Voice": a waiting voice (a question to the owner), who speaks now (where and on
  which model), whose voice answers, the model of the character's voice (to choose another: "Move to …"), and below that
  the computer's voices, with "Own voice" first and large. "Hearing": who listens now and on which model, the wake word,
  the computer's speech recognition models. The computer's page says little about voice and hearing and leads to these
  items; "Character" → "Voice" is the same item.
- **A waiting voice cannot be missed.** A voice that lacks memory, or one that did not start, shows from any screen: a
  pill by the dock "… is waiting for your decision", a dot on "Settings" and on "Voice". The window with the question
  and the buttons "Turn on anyway" / "Cancel" ("Try again") opens by itself, once for each wait. You can answer from any
  panel: the hub passes the answer to the computer where the voice waits.
- **The reason "RAM is tight" in the panel's language.** The gateway keeps numbers for a waiting voice (free, needed,
  how much it holds for the speaker) and writes the sentence in the reader's language; "RAM is tight" is no longer
  repeated twice.
- **Your own voice in first steps.** On the "Character" step, when the standard Alice is chosen, there are "Or your
  voice" and "Create your voice": the same recording wizard. The recorded voice turns on, and the speaker switches to
  the computer's voice.
- **Apps on the Turbo.** An app's code from a feed that only the speaker reads installs on the Turbo when the Turbo's
  catalog has checked that archive. The assistant says plainly whether the Turbo sleeps or the app is not there. The
  assistant reads the data of the app's card aloud and does not say "on the desk" about a widget that is not visible.
- **First steps: "speaker + Turbo" by default.** On the "Your home" step, the option "hub on the speaker, computer as
  Turbo" is chosen and marked "Recommended", even when the search did not find the speaker: on the "Speaker" step you
  can type its address by hand.
- **Characters from an invitation, at once.** The set of demo characters shows and can be chosen right away from this
  computer, while the speaker catches up in the background: the choice reaches it by itself, with no need for "Try
  again". While the speaker installs or answers slowly, "The set did not load" no longer shows for nothing; open panels
  and the speaker reread an added source of your own by themselves.
- **The "Where to start" showcase:** the snake shows "made by voice" and the phrase "Alice, make a game".
- **An answer clears the question.** Answering an agent's question in any way, by voice or with a button, removes it
  from the "Done" bar. "All works" opens the list of works; a game put "on the desk" by voice lands on the desk. A theme
  has its own question, and "on the desk" applies it.
- **Music without "Please repeat".** The tool that starts a track answers at once, and the track goes separately, so
  Alice no longer asks to repeat on top of music that is already playing.
- **Back to the character's voice.** "Voice" shows the characters' voices and "Back to the character's voice"; your own
  voice on top no longer falls off by itself. "Re-record for <model> from your recordings" is always there when the
  computer has another model.
- **Wake words in one place.** "Hearing" has a list of words: one listens, "Only Alice", "Remove all words". The word's
  button is by "Hears" on the desk; in "Character" there is a "Word" row with a switch, and with no word, "Find a word";
  in MagicPlace, a "Words" filter.
- **Sheet windows scroll.** A long list (for example "Add a provider") no longer shrinks to the window's height;
  descriptions in the speaker's window are in the panel's language.
- **"Own key" in first steps, with the provider's fields.** For "Own, OpenAI-compatible" and "Own,
  Anthropic-compatible": the service address and the model (if the service does not name its models itself); for
  YandexGPT, the catalog ID, as in "Character" → "Brain" → "Add a provider". Without them "Save" cannot be pressed. An
  address of your own makes its own provider, so an earlier one with another address is not overwritten.

## 0.16.0 — 18.11.2026

- **Install wizard.** "Install" in MagicPlace opens one window: what will be installed (the package itself, its pairs
  such as the character's voice on the computer, and anything else that comes with it) and which rights, with one
  button, "Install all". Then the same window shows the steps with checkmarks: packages, the character as the home's
  voice, the voice on the computer, apps starting; at the end, what to say to the character, and for an app, its phrase
  or how to use it. Closed halfway, the app's card offers "Continue the install"; "Collapse" hides the wizard while the
  steps go on, and a pill by the dock shows them. The voice step waits a minute and a half and then "catches up" instead
  of failing; the speaker switches to the computer's voice, and the step is closed only when the speaker confirms it.
  Rights approved at install go to the installed version, even while it is still off.
- **The character's voice on a Turbo, with a choice of model.** The voice model is chosen in the wizard up front, and
  its download is a separate step. A model that is already on the computer is offered first, the others behind "Choose
  another"; one that does not fit on the disk is not chosen and does not start, and the card names its size. The
  download never hangs silently, and "Cancel" stops it. When the voice is up on the Turbo, the hub sets "Computer voice"
  for the speaker by itself.
- **Own voice, step 3.** The main button takes the imprint, shows the progress and turns the voice on, then "Done". The
  computer's windows open over the whole panel (a dimmed background and a window; the page behind stays in place), and
  the wizard returns to where it was; the imprint hears the words of a take through the gateway when speech recognition
  runs in it.
- **VPN by voice:** "turn on VPN", "turn off VPN", "is VPN on?"; anyone at home can switch it. A VPN app that is turned
  off says "VPN is off" and turns on, instead of "No subscription".
- **First steps: an invitation with demo characters.** On the "Character" step there is "Have an invitation?": a link
  adds demo characters, and the wizard installs them. The choice holds through "Back/Next", and "Next" is busy while the
  set loads. "Character" waits until the chosen speaker becomes the hub, is never empty and offers "Move now"; Alice's
  card has her sign.
- **The move to the speaker reaches its end,** after a panel reload, a gateway restart and a lost task, and carries the
  computer's own sources and the installed characters, with the chosen one first. After the move, the computer as helper
  takes everything from the hub; the speaker gets the home's language.
- **"Yes" to rights.** An answer to a standing risky right is for good and survives a restart; the microphone is
  described as "listens or watches".
- **The assistant does not say "done" or "on"** unless it called a tool in that answer.
- **Wake word:** a misheard word is removed from the text, and after a request that went to Yandex there is no second
  press. In MagicPlace such words are a pair of a character, not separate apps, and have no "Recommended".
- **Desk and windows:** "On the desk" by voice puts the same tile that the desk draws; a Turbo card without a voice
  leads to where a voice is installed; the home window in the computer's panel knows the computer as "this one" ("Voice:
  not installed · Install"); the "Character" window installs through the wizard and hides characters that the feed keeps
  out of the lists; "Say: …" in the examples uses the word that really calls.
- **The desk after first steps has no overlaps.** A locked desk scrolls with the wheel and the touchpad: the lock holds
  the layout, not the view. The "Where to start" showcase is fully visible above the dock (the desk gets a little
  smaller by itself in a low window). Widgets on the sides of the speaker do not cover the showcase and first go where
  they can be seen, not past the right edge; "+ New widget" and a widget being made are above the showcase too.
- **The "Where to start" showcase goes away after your first own widget,** made by voice or a snake left on the desk.
  You can bring it back from "+ New widget".
- **"Installed": icons of apps from the catalog** show in the speaker's window opened from the computer too (before, a
  broken image). **"Folders" on a speaker hub** says "on the computer", not a red "error": it is an app of the computer.
- **MagicPlace: what is installed stays in "Recommended" and "New",** with the "Open" button in the same place.
- **The "yes" question on a new app's rights** is reread by the panel itself in the first seconds after install or
  turning on, so the sheet shows at once, even when the live channel did not tell about the question.
- **One panel for the home** (docs/plans/home-and-devices.md, stages 0 and 1). At the top of the panel there is the home
  and its devices: speakers, the computer, a server, with "Hub" and "Turbo" labels and a dot for the link. There are no
  more device tabs and no dark Turbo panel frame: from wherever you open it, the panel is one, and a tap on a device
  opens its page.
- **"Settings → Home → Devices"** instead of "Speakers" and the old "Home": all devices of the home, and each one's page
  shows only its own things. A speaker hub has "Overview · Room · Device", a speaker that hears for the hub has
  "Overview · Room", a computer has "Overview · This computer · Other homes · Device". "What leaves the home" is a page
  of its own.
- **Words.** People no longer see "gateway": the role is called "Hub", the program is DuoWave, and a device goes by its
  name. "Gateway" stays in the code, the logs and the documents for developers.
- **The whole home on the desk:** speakers that hear for the hub, the Alice and cloud badges, "Apps" and a colour legend
  (lime for the hub, violet for Turbo, amber for the clouds and Yandex, grey for asleep).
- **The microphone of a computer as Turbo talks to the hub,** and not only for an administrator: saying a phrase is not
  a change of settings.
- **MagicPlace says where a package will run** ("runs: Kitchen", "needs a computer as Turbo"), and "Apps → Installed"
  also shows what is installed on the home's Turbo, by device (stage 5a). For now they are installed on the devices
  themselves.
- **One desk for the home on every device** (stage 2). The home window on the computer writes the desk and the panel's
  memory to the hub, under the person themselves, not into the browser: the desk is the same on the computer and on the
  phone. A person can edit their own desk and panel memory through the computer too, not only an administrator.
- **The voice is part of the home's character.** Choose a character in any panel, and the hub names it to the Turbo; the
  Turbo installs the character's package from the signed feed by itself and speaks in its voice; the speaker sounds in
  the new voice. Another home does not change the computer's voice.
- **Telegram is a home connection.** The hub keeps the token and contacts (a speaker hub too): they are set in
  "Connections" from any panel, and the bot runs on the Turbo. The hub hands it the settings when they change and when
  the Turbo comes back.
- **Rooms.** On a speaker's "Room" tab there is "Who answers here" (Bublik in the kids' room, Arthur in the kitchen,
  with shared memory) and the mode of this room ("Alice" in the kids' room, "Your AI" in the kitchen); when the mode
  changes, only this speaker reconnects. The gateway already keeps the room's volume; the speaker will take it in stage
  4.
- **A copy of the home** (stage 3). The hub is the only one that writes the home; a computer as Turbo keeps a copy of
  it: settings, people, characters, connections, rooms and memory. A new change on the hub reaches the copy in seconds
  (the computer looks at the hub right away through its live channel) and applies without a restart; the speaker no
  longer restarts on every change. The computer has its own access keys: with them the hub reaches it as a Turbo.
  Sign-in to the computer is with the home's account. The copy does not touch what belongs to the computer: its models
  (LM Studio, Ollama, what runs on it itself) and MCP programs stay, and the Turbo's model is the same. A Telegram bot
  that the computer had before the home becomes the home's bot if the home has none of its own.
- **Without the hub the computer answers by itself,** from the copy: the same character and the same memory. What it
  remembered outside the home joins the home's memory when the hub comes back: each record has a date and who said it,
  and the newer change of a record wins. A change in the home's panel while the hub is away is saved on the computer and
  goes to the hub in order when it comes back ("saved, will reach the hub").
- **Removed:** "settings on the computer" and "Return the settings to the speaker", copies of a speaker's settings on
  the computer and their restore. The live copy of the home replaced them. A new install: there is no move from the old
  data.
- **The home's apps: the face at the hub, the code in its place** (stage 5b). On a speaker hub with a Turbo, an app for
  the computer is installed as a face (its phrases, tools, window and widget are at the speaker), and the hub installs
  its code on the Turbo by itself over the link, from the Turbo's signed feed, in the same version; if the Turbo is
  asleep, it installs it when it wakes. Every call of such an app goes to the Turbo; its code does not run on the
  speaker. Another home installs nothing on the computer.
- **Apps "for the speaker" install on a speaker with a Turbo again** (AirPlay, VPN): a speaker with a Turbo is still a
  speaker. Before, as soon as a computer started to help the speaker, they could not be installed, and those already
  installed stopped working ("does not work here"). Now they install, work and update as on a speaker without a Turbo,
  and RAM for them is still checked on the speaker.
- **The Turbo contract, version 2.** A Turbo older than the hub says "update both" on the device page: the new things
  (the character's voice, apps over the link) wait for its update.
- The catalog feed carries `prefer` and `place`: MagicPlace labels the card with the place before install.
- **Several speakers, one home** (stage 4). A second speaker becomes an ear of the hub: its voice goes to the hub (the
  hub decides who answers in that room and opens Yandex with that speaker's own account), it has a copy of the home, and
  without the hub it answers by itself. A change to the home is not accepted on an ear ("make changes in the hub's
  panel"), or it would be lost.
- **"Make the hub".** On the hub, on the page of a speaker that is an ear: the hub moves to it over the home network,
  without SSH, with the latest of the home, and the old hub becomes its ear. A computer as Turbo moves to the new hub by
  itself. The same path works when a firmware update closed the speaker: another device becomes the hub.
- **Removed:** the compatibility layer with the old 0.11 "Home server" (`helpers/legacy.rs`): its setting is no longer
  read or written; the computer's address and key from the install still make it the speaker's Turbo.
- **First steps: "Your home"** (stage 6) instead of "Where will DuoWave live": what the home has and what each device
  becomes: "the hub is already here", "will become the hub", "needs root", "this computer is a Turbo while it is on" or
  "the hub, while it is alone in the home". If there is a home already, the computer joins it as a Turbo: it has no home
  of its own.
- **One more speaker with one button.** On the computer's "Devices" there are speakers with DuoWave that are not in the
  home yet: "Join the home" makes the speaker an ear of the hub with a copy of the home, and then we ask only about its
  room.
- For developers: `GET /api/v1/home/devices` and the `home.devices.changed` event, `GET /api/v1/home/apps`, `GET|PUT
  /api/v1/rooms` and `home.room.changed`, `/home/v1/character`, `/home/v1/messenger/bot`, `GET|POST
  /api/v1/home/memory`, `POST /api/v1/home/held`, the `home.copied` event, migration 16 (`home_queue`), `GET|PUT|DELETE
  /api/v1/home/ear`, `POST /api/v1/home/centre`, `POST /api/v1/home/become-centre`, `POST /api/v1/home/join-ear`;
  `/home/v1/plugins` returns all of the Turbo's packages with their kind and state.

## 0.15.1 — 17.11.2026

- **The gateway on the speaker no longer swells while the panel looks at it.** The panel on the computer asks the
  speaker often, and every such look read all the apps again: the gateway's RAM fragmented and never came back, from 15
  to 57 MB in an hour with the panel open. Now the gateway rereads only the apps that changed and hands freed RAM back
  to the system at once: a night on the speaker stays at 9–13 MB instead of 70.
- **Updates where they are done.** The window of a speaker run by a computer has "Update to …": the speaker asks the
  computer, and the computer updates it. On the Turbo's page the speaker's card comes first, with a line on who updates
  it, and below it "This computer" with its own version. A new version for the speaker puts a dot on "Settings" too. The
  gateway's hint names this path.
- **The avatar menu in the Turbo's panel is readable:** it is dark like its bar; before, light text sat on a light
  background and the menu looked empty. A speaker from another home that the computer helps shows as a speaker, not a
  laptop.
- **The Turbo card reads "Thinks · Hears · Speaks";** the filter in "Apps" is "Kind / Any kind" instead of "Face".
- **Desk: "+ Shortcut" opens its menu again** on a wide desk. Since 16.11 the scrolling of the floating column cut the
  menu off, and the clear backdrop ate the taps, in every panel. "Remove from desk" works on a locked desk too: the lock
  holds only places and widths.
- **"YouTube Music" with its own icon.** The YouTube app is now called "YouTube Music" (0.1.1) and has its own icon, a
  "play" button with sound waves; the voice phrases are the same. The source is `apps/youtube-music/`, id
  `youtube-music`. The agent cards Codex CLI, Factory Droid and OpenCode in MagicPlace have their own icons (1.0.1).

## 0.15.0 — 17.11.2026

- **Updating from 0.14.0.** The first start adds the tables of work folders and works to the database (migration 15);
  the existing data does not change, and before that the gateway puts a copy of the database into `backups/`. Going back
  to 0.14.0 is just the old version: it only warns in the log about the migration it does not know; you do not need to
  restore the copy from `backups/`, it is older and would lose what changed after it. A Turbo model without its own
  address (a Turbo by key, or one that left while its model was chosen) is stored in "Brain" with a stand-in, a closed
  port on the device itself, so the old version does not take it for OpenAI and does not send a question there. Three
  things change right after the update. The panel from the internet through your own proxy on this computer opens only
  with `PANEL_REMOTE=true` in `.env`. If the main model is on a computer (Ollama or LM Studio here, "Model on the
  computer" for a speaker) and there is no "Backup", the gateway sets a cloud "Backup" with your key by itself:
  questions to it cost money, and folders without the "The model can read names and files" checkbox close to the model
  (more below). The gateway does not remember "not needed" chosen earlier as a refusal: if you do not want a cloud
  "Backup", choose "not needed" again after the update. A cloud "Backup" set by the gateway stays there as yours after a
  rollback, so remove it before you roll back. The address and key of the speaker's computer move into the helpers list
  by themselves ("Home computer"); the old setting is kept along with the list for one more version, so a speaker that
  its safety net rolled back to the old version finds the same computer. A speaker on the old version with a new
  computer, and a new speaker with a computer on the old version, both work; a link through a router works when both are
  new. The computer also listens on a link port, the next one after the panel's port: 7781 next to 7780 (`LINK_PORT` in
  `.env`; `off` means no port of its own, and then the link goes only through your own proxy). If you update from 0.13.0
  or older, first read "Updating from 0.13.0" in 0.14.0 below.
- **You can see where you are and who does what (owner, 16.11).** The heavy part is now called "Turbo": a computer at
  home or your own server that takes the model, hearing and voice. At the top of the panel there are tabs for the home's
  devices: `Speaker "Kitchen" · gateway | This computer · Turbo`; the pressed one is whose panel this is, and the other
  opens with one tap, without `?local`. The desk shows two bodies: the light gateway and the dark Turbo block next to it
  with its skills and whether it is online. The scene's nodes are "Hears · Thinks · Speaks", coloured by who does the
  work (lime for the gateway itself, violet for Turbo, amber for the cloud and Yandex); under the scene, what happens if
  Turbo falls asleep. The bar "This computer helps the speaker" and the "Where now" widget are no longer on the desk.
  Everything about roles is on one page, "Settings" → "Home": the gateway and who answers on the speaker, "I hand work
  to Turbo" with switches for skills, "If the Turbo sleeps" with "Backup", and the dial; "Brain" keeps the choice of
  models. The speaker's modes are named by who answers: "Alice · Alice + your AI · Your AI" (it was "Yandex · Together ·
  Own"). A computer that only helps shows its panel in a dark frame: its strengths, whom it helps and "No talking here".
- **Panel windows follow one layout (owner, 17.11).** Windows of apps, connections, the catalog, characters, MCP, Turbo
  and settings open in the middle, and the section's window under them hides. The header has an icon, a name and chips
  (running, from where, version); rare things sit behind "⋯". A window closes with the cross, Esc or a tap outside;
  there is no "Done" at the bottom any more. There is one main button, lime, at the bottom right; "Save" appears only
  when something changed; turning off, removing and unlinking are under "Danger zone" at the end of the window. An app
  has the tabs "Overview · Access · Settings · Versions": phrases as chips, addresses and rights as code. The question
  "Remove?" is a small window with buttons on the right. While data loads, grey bars hold its place, so nothing jumps; a
  connection's window no longer opens twice. "Hearing" and "Voice" appear on the computer's page once each. Buttons
  across the panel look the same, with no white ones. The shortcuts bar floats over the desk, transparent, and does not
  push the desk.
- **Helpers.** "Settings" → "Helpers": in order, who gives hearing, voice, the model, messages, apps and app history.
  Arrows change the order; the row's menu has "Check now", "Do not take from it…", "Rename", "Connect again", "Open its
  panel ↗", "Remove". For each skill the gateway takes the first helper that is online and can do it; with none, the
  clouds by keys, then Yandex if the mode lets it. On a computer "This computer" comes first: the computer takes its own
  hearing, voice and model by the same choice. This gateway's name is there too, under "Change".
- **"Where now"** is there too, and as a widget on the desk: who gives hearing, the brain and voice right now (the page
  also shows messages, apps and history) and why not the first one. When this computer's recognition or voice server
  does not answer, "Where now" says so, and the speaker and other gateways take hearing and voice from the next one.
- **"What may leave home"**: "Home only" · "Home and my cloud" · "Any clouds". The default is "Any clouds", as before. A
  helper outside the home (your own server, a cloud) that the dial does not let through gives nothing. The dial holds
  the cloud models of "Brain" together with "Backup", cloud hearing and voice, and apps' cloud voices; it does not touch
  Yandex, which the mode lets through. It does not stop "pass on…" messages through the computer's Telegram bot. Nor
  does it hold apps' own requests to the internet, MCP programs and the catalog download.
- **"Brain" is not silent without the computer.** Every helper with a model has its own row "Model: <name>"; the old
  "Model on the computer" became such a row, and the choice was kept. When a helper is offline, "Backup" answers at
  once, without a 10 second wait; with no backup, another helper with a model; with none of them, Alice answers in
  "Together", and in "Own" and in "Ask the brain" there are honest words in under a second. A model whose VPN is not
  answering right now does not take the computer's place: it would only say "VPN is not answering". In "Spending" a
  model of a helper at home is "at home", free; outside the home it is "own cloud": at the price entered in "Spending",
  otherwise "no price". Honest words do not count as a deal.
- **"Backup" may cost money by default.** If the main model is on a helper (Ollama or LM Studio on this computer
  included) and there is no "Backup", the gateway sets a fast cloud model with your key by itself: only one it knows by
  exact name, only when the dial lets clouds through, and not one that the VPN does not let through right now. Questions
  to it go over the internet and spend your key; "Brain" says so in a line. In "Backup", instead of a single "not
  needed" there are two items without a model: "let the gateway choose" (the default; when the gateway has chosen, the
  item names the model) and "not needed", and then the gateway does not set it until you bring back "let the gateway
  choose". Choose "let the gateway choose" again, and the gateway chooses anew; remove the provider of a backup the
  gateway chose, and it goes with it, and the gateway takes another cloud. With the main model in the cloud the gateway
  does not set a backup by itself, and "Brain" says so. `PUT /api/v1/brain/choice` takes `backup: "auto"`; the answer
  has `choice.autoBackup`.
- **A cloud "Backup" behind a home model is cloud too.** While a cloud "Backup" (the one set by the gateway too) or a
  smart model in the cloud stands behind the home model, folders without the "The model can read names and files"
  checkbox are closed to the model, and apps' questions in words go only when "Questions in words via the cloud" is on:
  a cloud model could pick up the answer along with what was read.
- **Connect a gateway by key.** On the computer, "Connect a gateway": a one-time connection key as text and QR, valid
  for 15 minutes; on the gateway, "Add a helper": the text or a photo of the QR (on iPhone too), "Check", "Add". The
  gateway opens the link itself; it is encrypted and goes through its own port 7781, the only one to open in the router.
  In Docker the "Connect a gateway" sheet shows only the key: from inside the container the gateway sees neither the
  machine's address nor its port, so on the gateway type `https://<machine address>:7781` and the key, or enter that
  address in "Outside address". Behind your own proxy with a domain, the domain goes into "Outside address"; the proxy
  runs on this same computer (or on the Docker host: the proxy container's address or a small compose network, no wider
  than `/24`, in `LINK_PROXY_FROM`), has a real certificate for the domain, writes `X-Forwarded-For` and passes only
  `/home/v1/link`. The permanent link key never travels the network in the clear: at connection it arrives encrypted by
  the connection key, and after that the gateway names only its number and answers the Turbo's challenge, so anything
  seen on the stretch behind the proxy opens nothing. Changing the "Outside address" after connecting does not break the
  link. If the gateway and the Turbo have different link versions, the gateway says so: "Turbo runs another version of
  DuoWave: update both". Over `http` a helper connects only in the home network. The computer gives everything to the
  gateways of its own home; to the gateways of another family it gives hearing, voice (only this computer's Silero or
  Piper), the model and the catalog, and the rest is under "I help" → "What to give…".
- **A proxy on this same computer no longer passes internet requests off as requests from the computer itself.** The
  gateway judges a request with forwarding headers (`X-Forwarded-For`, `Forwarded`, `X-Real-IP`, `Via`) by the address
  of the one the proxy asks for, on every route. Home devices through a home proxy stay at home; the panel from the
  internet through your own proxy works as through a tunnel, only with `PANEL_REMOTE=true`. Wrong passwords through a
  tunnel or proxy are counted by the visitor's address and do not lock sign-in on the computer itself. The gateway
  counts as a Cloudflare tunnel visitor only one named both by `X-Forwarded-For` and by the other forwarding headers:
  behind nginx that writes only `X-Real-IP`, stray `cf-*` headers no longer let anyone guess the panel password without
  being counted.
- **The home number does not get mixed up after a rollback.** A speaker taken back to 0.13.0 or 0.14.0 took the
  computer's home number on "Move to the speaker", while its home key stayed its own: two keys for one home. Now the
  gateway remembers which key its number belongs to and puts its own number back by itself after the update.
- **"This computer"** instead of "Helper computer" (in the computer's window; on a computer without a speaker, at the
  bottom of "Helpers"): "I give", "I help", the connection key and, while the computer helps someone, "Keep the computer
  awake". On the speaker's desk "Where now" replaces the "Helper computer" widget; the computer's widget "Help the
  speaker" became "This computer is a helper".
- **The speaker's program is lighter**, although it has all the new code: 22.50 MB instead of 23.12 MB in 0.13.0. The
  new code added 0.56 MB, and the compressed panel saved 1.18 MB. The panel is stored in it compressed (2.18 MB of its
  files take 0.85 MB in the program) and goes to the browser compressed the same way; the panel's fonts are woff2 only.
- **The computer recognises a speaker with its own DuoWave.** The speaker check read its gateway's answer past the
  wrapper and recognised none: the "Help the speaker" step in "First steps" and the card on the desk did not appear.
- **A speaker "offline" by name.** The speaker's window called it by an internal number (`Speaker "h:…" is offline`);
  now it uses the name from the list of speakers, or the address when there is no name.
- **"Ask the brain" without a raw error in English.** With no connection to Anthropic it says "Claude is unavailable
  right now: no connection to Anthropic."; the details are in the gateway's log.
- **The bar "This computer helps the speaker" on the desk** shows only while the computer really helps (with "Help" off
  it is not there), and without the speaker's name it says "Speaker", not the character's name.
- **"Character" in "Computer only"** no longer says "Voice/Brain: Yandex · Alice": the rows are as on the desk.
- **"No voice" on a fresh "Computer only"** shows only when there is really nothing to speak with: when Alice's voice is
  chosen but the gateway speaks with its own, the desk shows that.
- **"First steps" do not come back** after × and a panel reload right after it: the panel's memory goes to the gateway
  at once.
- **"Where will DuoWave live?" points to the real place:** "Settings" → "Speaker" → "Where DuoWave runs", and for
  "Computer only", "Settings" → "Microphone" → "How to talk to DuoWave".
- **"First steps" on a computer linked to a speaker** choose "Speaker + computer" up front, even when the network search
  did not find it: "Next" no longer moves the conversation from the speaker to the microphone.
- **The gateway on the speaker no longer swells from apps.** Every load and unload of an app fragmented its RAM, and
  what was freed never came back: by morning the gateway held up to 72 MB, and 0 was left for apps. Now every app has
  its own piece of RAM, and on unload it goes back to the system whole: over 330 loads the growth is 12 MB instead of
  49.
- **An app is counted with what it started.** Supervision counts the RAM of an app's program with all its processes:
  each one's own RAM, and shared file pages once; for a program without its own processes the figure is the same as
  before. "VPN" now shows its engine too; the "VPN" program 0.1.1 itself holds no more than 1 MB of unread XHTTP, and
  the "VPN" budget in the catalog is 18 MB instead of 32.
- **Panel and gateway texts:** in Russian, RAM is now called "ОЗУ" (not "оперативка") and storage on the speaker
  "память"; the two are no longer mixed up.
- **A wake word for an app on the speaker.** An app that hears its own word wakes the speaker like the button (message
  `system/activator` → `wake`), and the request gets the sound before the word: the phrase comes in one breath, the
  volume is set by the word, and the word leaves the command text. The word makes its character the home's character,
  from this answer on, until another one is chosen (for now the home has one character). The microphone sound comes by
  the message `system/mic` → `open`: the microphone listens to the room all the time, so the owner gives the "yes" at
  install. Only the gateway on the speaker itself wakes it. An app with builds for platforms from its own feed installs
  when the DuoWave catalog pinned the archive for this platform by hash.
- **A character and its word are paired packages** (`companions`): install one, and the panel offers the other at once,
  saying why, how much storage and RAM it takes and which rights it needs. If you declined or it did not fit, the card
  has a "Goes with" row with an "install" button; remove one, and the panel asks about the other.
- **One question at install.** An app that asks for rights is installed by the panel after one question: how much
  storage and RAM it takes and what it will be able to do, in words. "Install" is also your "yes" to these rights: the
  app does not ask later. For the speaker's microphone, which hears the room all the time, such a "yes" means "always"
  for this app.
- **Low RAM on the speaker no longer turns an app off for good.** The last one turned on falls asleep (it stays on,
  "asleep" in the panel) and wakes by itself when RAM frees up with room to spare. The gateway sets `oom_score_adj` for
  app processes: if the kernel has to kill something, an app goes first, not Alice and not the gateway.
- **YouTube on the speaker, an app on the computer.** "Alice, play Kino, Gruppa Krovi from YouTube" gives what was
  found, and after it similar songs from the YouTube mix; "add … to the queue" puts a song after the one that plays;
  "next", "pause", "continue" work as always; "Now playing" shows the title, channel and cover. The sound comes through
  yt-dlp: the app downloads it from GitHub itself, checks its SHA-256 and updates it when YouTube changes its cipher;
  ffmpeg on the computer makes MP3. The next song gets ready while the current one plays. Without a VPN where YouTube is
  blocked, or when YouTube refuses a provider's shared address (429), the speaker says so in plain words. It is not in
  the shared catalog: it is in its own feed and installs from there because the DuoWave catalog pinned this very archive
  by hash.
- **Code from your own feed only when we checked it.** A plugin or a one-archive app from your own source or an extra
  feed installs when the signed DuoWave catalog pinned exactly this archive by hash (`recommends`); a new archive waits
  for a new hash, and other code still does not install.
- **"What's playing" under your own song** is answered by the gateway itself, with the title and artist, without
  listening to the music; under radio Yandex still listens. **"Add to the queue"** for plugins is the `add` field of the
  answer. An app's stream goes to the speaker with its length and `Range`: "continue" after a pause resumes in place.
- **Work in work folders.** The "Works" page has "New work": a folder, a task, "Run", and the way into "Work folders". A
  work folder is a folder with git on this computer, added in the panel (a folder without git is refused, with words
  why). Every work has its own working copy and its own `duowave/…` branch: your branch and your files do not change,
  and the gateway brings the work's branch into the folder itself. The work is done by an agent: the built-in one runs
  Claude Code that you installed yourself, "As in the terminal" (the sign-in you made there, a subscription too; DuoWave
  does not see it) or "By API key". A folder has an agent and a model, a cap per run, preparation commands, checks after
  the agent and allowed commands: what is not allowed, the agent cannot do, `git push` included. The "Journal" tab shows
  live what the agent writes and thinks, which files it reads and edits (as a "before / after" diff), which commands it
  runs and what they printed, the steps of its helpers, and errors; "Only errors", search, "Download as is". "Add on the
  way" and "Continue" start a new run in the same agent session; "Where" has the command that opens this session in the
  terminal. A subscription limit is a question, "wait or stop?", and the work goes on by itself after the reset. Every
  run is a deal in "Spending" in the "Worker" seat. One work in folders at a time, the next waits in the queue. Computer
  only, administrator only.
- **Other agents as cards from MagicPlace.** MagicPlace has an "Agents" tab: a card says which program to install (you
  install it yourself; installing the card downloads and runs nothing), how to sign in to it, and leads to its terms.
  The first cards are Factory Droid and OpenCode. An installed agent appears in "Work folders" next to Claude Code:
  whether it was found, which version, whether you signed in; you can choose it for a folder or for one work. DuoWave
  talks to it over the Agent Client Protocol: the journal, "on the way", "Stop" and deals work as with Claude Code. When
  the agent asks for permission, the folder's rules answer: allowed commands, files only inside the working copy;
  DuoWave never gives "allow always". The agent reports the price itself ("agent's estimate"); if it does not, "price
  unknown". An agent that does everything without asking is marked. Remove a card, and the agent leaves the folders, and
  the panel says which ones. Cards install only from the signed catalog.
- **"First steps" with a speaker reach their end.** In "Speaker + computer" and "Speaker only", in the middle of moving
  DuoWave to the speaker the panel jumped to the speaker's window, and "Who answers", the character and the rest were
  lost. Now the computer's window waits until the steps end or are closed. After the move, the mode, the brain sign-in
  and the key are saved on the speaker, where DuoWave takes them; the model on the computer stays with the computer. The
  character is on the speaker too, and its voice is on the computer: the speaker speaks in the computer's voice.
- **Changing the language on the first screen of "First steps"** no longer throws you out to the desk.
- **"Settings" → "Appearance" is whole again:** language, the panel's look, the shortcuts bar and "Go through first
  steps"; from the list of pages, only the panel's look was there. In the speaker's window, "Go through first steps"
  opens them in the computer's window.
- **"Put on the desk?"** keeps its buttons together: when space runs short, the text wraps, not "No thanks".
## 0.14.0 — 16.11.2026

- **Updating from 0.13.0.** No database migrations. The "VPN" 0.1.2 app from the catalog needs this gateway. The first
  update of the speaker's gateway to this version still goes the old way: it needs room for two programs (about 30 MB
  free).

- **"First steps".** After the password the whole panel becomes one "First steps" screen: where DuoWave runs (only the
  computer, the speaker and the computer, or only the speaker), connecting the speaker right there (the check, what
  will change, the connection and the move of the gateway), who answers (the speaker's modes and the model, or signing
  in to a provider for a computer), the character, six "Try saying" phrases and MagicPlace. If DuoWave already runs on
  the speaker, it offers "Help the speaker". To go through it again: Settings → "Appearance" → "Go through first
  steps".

- **"VPN" is one place in the panel.** Tabs "Home", "Locations", "Settings" and "Subscriptions"; an "Off/On" switch,
  locations with signal bars, "Only the chosen" and one "If the VPN does not answer" setting for everything.
  `PUT /api/v1/egress` takes `{on, pick, homeDirect, onDownAll}`; the old `{mode}` still works.

- **Desk shortcuts are off by default.** A fresh desk has no shortcuts, and AirPlay no longer adds its own. Turn the
  shortcuts bar on in Settings → "Appearance" → "Desk" → "Shortcuts bar" (or say "show shortcuts" while this switch is
  on the screen). Saved shortcuts are kept while the bar is off.

- **What stands in the way on the speaker.** The check in "Connect a speaker" names other mods (YaJavis, Alisa
  Voice, Pulsar, Quasar Bridge, Alice LED) and traces of unknown ones: maind not running, Alice's files replaced, little
  storage. The rows open up: how it gets in the way and how to turn it off; the ones in the way are highlighted. Mods
  with a switch of their own get "Turn off". It works after a "Yes" and removes nothing; the event shows in "Who changed
  what".
- **DuoWave onto the speaker in one command.** `curl -fsSL https://github.com/duowaveai/duowave/releases/latest/download/install.sh | sh`
  (Windows: `irm …/install.ps1 | iex`) downloads the gateway for the computer, checks it against the release's
  `SHA256SUMS` and runs `duowave station setup`. It finds the speaker, looks at what stands in the way, shows the plan
  and, after one "yes", installs the gateway on the speaker and points the voice at it.

- **Own icons for the built-in apps.** In "My apps", Calendar, Currency rates, Folders, Games, Morning show, Own
  music, Neon snake, News, Radio, Shopping list, Smart home, Fairy tales and Weather now have their own colored logos in
  one style instead of letters and plain lines. The card with tiles got even margins on all sides.

- **The start desk: less, but alive.** A fresh desk has the scene and a showcase of three cards under it. First, the
  honest part: while there is no AI, "Make Alice smarter" (sign in through OpenRouter, a ChatGPT plan, your own key or
  a model on the computer). Once a key works, the speaker turns to "Together" by itself (with "Undo") and shows "Alice
  is smarter now" with three phrases: talk, play snake, make your own widget. You can say a phrase or tap it. The snake
  plays from a button even before the key, and after a game it offers "Keep on the desk". Nothing else puts itself on
  the desk: "Music" shows while something plays, "Timers" while a timer runs, "Not supported yet" when there are
  unanswered questions, "Helper computer" when something is wrong with it. "Who answers" opens with a tap on "Brain" in
  the scene; modes, counters, apps and the snake are in "+ New widget". A desk you already arranged stays as it was;
  you can hide the showcase and bring it back from "+ New widget".

- **"Put it on the desk?"** Ask a second time about something an app with a card answers ("what is the dollar rate"),
  and under the speaker you see "You asked twice about "Currency rates". Put it on the desk?". "Add" puts the card
  there with a "new" mark; "No thanks" means it never asks about this app again. A card you removed is never offered.
  The question journal now remembers which app answered, both by its phrase and when the model called its tool, so
  questions in your own words count too.

- **A theme from three colors, and a drawn backdrop.** A theme needs only three colors per mode: background, text and
  accent. The panel builds the cards, lines, underlays, selection and shadows itself; the "who answers" colors stay
  the same. So a theme is easy to ask for by voice: "Alice, make a theme: cream, with green". Behind the desk there can
  now be a backdrop: up to three of the panel's living scenes in layers: stars, aurora, snow, waves…; "make the theme a
  starry sky" gives stars with an aurora in dark mode. The panel draws the scenes; the theme only names them. The
  workshop refuses a theme whose text does not read on its background.

- **The "VPN" window asks the home country, not the panel.** "Where is your home?" is in the app's window: the
  guessed country (Russia for Russian), a couple of others to choose from, "Another country…" with search, and "I
  won't say". The VPN app keeps the answer itself; other apps do not see it. "Internet access" decides by it. The
  "Home country" card in Settings → "Appearance", `GET|PUT /api/v1/home/country` and the `system/home` → `country`
  letter are removed; an answer given earlier moves to the VPN app by itself.

- **The speaker's gateway updates without a 30 MB reserve.** A new version first runs for 10 minutes from the speaker's
  memory, while the old one stays on disk and comes back by itself if the new one fails, hangs or the speaker restarts.
  Then the new one takes the old one's place, and in a quiet minute the gateway restarts for a couple of seconds. On
  disk it now needs room only for the program's growth plus 5 MB spare. During the trial it needs RAM the size of the
  program; if there is not enough, the speaker's apps turn off for those minutes and turn back on by themselves.
  Leftovers of past updates are cleaned up (after a rollback, 20 MB used to stay on the speaker as a safety copy). If
  the power goes off in exactly the seconds of writing, the speaker becomes plain Alice, and "Try again" on the
  computer puts the gateway back. The first update to this version still goes the old way: it needs room for two
  programs.
- **Apps on the speaker keep one version.** The old one is deleted as soon as the new one starts; "History" on the
  speaker does not keep large version files from the catalog. To bring the old one back: "History" → "Restore"; the
  files come from the computer or again from the catalog. The history of a removed app leaves the speaker at once, not
  after 30 days, when it also exists elsewhere: the app is from the catalog, or its history is already on the computer
  (your own app that exists nowhere else is kept for 30 days, as before). The room apps leave for a gateway update is
  about 8 MB instead of 15.
- The computer does not put on the speaker by itself a version that failed its trial there; "Update" by hand still
  works.

- **CosyVoice no longer changes the voice on short phrases.** CosyVoice says a very short sentence ("Yes, sir.", "Of
  course.") together with the next one, not as a separate piece. Before, it sounded in someone else's timbre.

- **Characters' catchphrases.** Now and then a character says its well-known phrase: "As you wish, sir." after an
  answer or "Listen up." before it. On average once in several answers, never in two answers in a row and never the
  same phrase again. The phrases come from the feed the character was installed from (`phrases` in its
  `catalog.json`); the feed changes and removes them without a new package version. A phrase is said word for word; the
  model does not get it, and it does not play when the answer fails.

- **The speaker on its own, without a computer and without Yandex.** Cloud hearing and voice are set in the speaker's
  panel: Settings → "Speaker" → "Hearing" (OpenAI, Groq or your own address) and "Character" → "Voice" (OpenAI or your
  own address). The OpenAI key from "Brain" is enough for everything; the key is kept on the speaker. With cloud
  hearing and voice, "Own" turns on on the speaker. When the computer sleeps, "Own" keeps going on the cloud; when the
  cloud is off, the speaker turns to "Together" by itself. In the cloud, a phrase is recognized with one request after
  the speech ends, not every half second. The cloud voice reads numbers and Latin letters by itself. "Whose voice
  answers" on a speaker without a computer: "Cloud voice" or "Alice".
- **The "Music" widget on the desk is alive.** What Yandex plays shows between phrases too: a track change, pause,
  title, artist and cover, from Yandex's answers. The widget's buttons work for Yandex music: the panel tells Alice the
  same words ("pause", "next", "I like it"), and under the widget you see what was said. A song without a length is a
  song, not radio: it gets the heart and "playing 1:23". Apps that play by themselves (AirPlay) can give the title,
  artist and cover.
- **An empty "Own music" no longer blocks Yandex.** "Play music" with an empty folder plays Yandex Music, not "No music
  yet".
- **Moving onto the speaker over an earlier gateway.** The room of the program being replaced counts as free; the old
  gateway stops before the new program is written; the install no longer copies the program onto itself (on the
  speaker `/data` is a link to `/bkupgrade/data`).

## 0.13.0 — 16.11.2026

- **Internet access.** The gateway decides what from DuoWave goes online directly and what goes through your own
  server from the "VPN" app (MagicPlace). Settings → "Internet access": three buttons:
  "Off", "Where blocked" (the default: through the server only what does not work from the home country, by the
  catalog's data) and "Everything via server" (Alice too); a default server; a summary line; below, fine settings for
  each one who goes online: Alice, the "Brain" models, cloud hearing and voice, MCP tools, apps, your own addresses.
  The table builds itself: install an app and its row appears. If the server does not answer, models, hearing and
  voice do not go directly (a key from a country a service does not serve puts the account at risk): they say "VPN
  does not answer" at once; Alice goes directly. Without the "VPN" app and without a "yes" to "VPN → Gateway network"
  everything works as before. Programs of apps get `HTTPS_PROXY` pointing at the gateway's entry.
- **Shared app settings.** Any gateway setting can now have its own value for a single app (the `app_settings` table,
  migration 14); this is how "Internet access" keeps the rule of each row. The rules survive removing the app (install
  it again and the rule comes back) and leave together with its history. They move with a settings package, except
  what the home learned about its own network. Before the migration the gateway puts a copy of the database into
  `backups/`.
- **Home country.** The panel asks once where home is, guessing by the computer's time zone and language; it never
  looks at the address. "I won't say" is an answer too. To change it: Settings → "Appearance", next to the language.
  The country is kept only on the gateway (`GET|PUT /api/v1/home/country`); apps learn it from the `system/home` →
  `country` letter. "Internet access" will decide by it.
- **Two ways back from the speaker.** `restore.sh` still only gives the voice back to Yandex: DuoWave stays on the
  speaker, and the panel connects it again; now it says so plainly. The new `uninstall.sh` removes DuoWave completely:
  the voice goes back to Yandex, the speaker's gateway and call client stop, their scripts leave `/etc/init.d`, the
  `/data/opt/duowave` folder is deleted, and the speaker restarts (`--keep-data` keeps the gateway's data on the
  speaker). It goes onto the speaker together with `restore.sh`; speakers connected earlier get it from the gateway at
  `/station/uninstall.sh`. The YAM package has
  `duowave uninstall`.
- **The "Remove DuoWave from the speaker" button** in the speaker's window, under "Return to Yandex". The gateway goes
  into the speaker over SSH, runs `uninstall.sh`, checks that nothing of DuoWave is left and restarts it. The speaker's
  SSH key stays, so "Connect a speaker" finds it again. In the "Minimal" look, the red button under the cursor no
  longer turns white without its caption.
- **The panel menu: five buttons, one home for each place.** The dock is always "Home · Character · Say · Apps ·
  Settings": the microphone in the middle, full captions, nothing slides off the edge on a phone, and the developer
  layer adds no buttons. MagicPlace is a tab of "Apps" with a pink mark; its update count is on the "Apps" button.
  "Workshop" is the "Make your own" button there. "Folders" is an ordinary app. Old addresses still work.
- **"Apps" in tabs.** "My apps" shows installed apps as tiles (icon, name, one status line), without characters and
  themes: those are in "Character" and "Appearance". An app's page has "Open", "Update" and "Shortcut on the desk".
  "Connections" holds outside services and programs (MCP); the model, hearing and voice have their own places in
  "Brain", "Hearing" and "Voice".
- **Memory is in "Character"**: what the character remembers about the home, people and things, pause. "Own voice" is
  inside "Voice".
- **Settings in groups**: "Device", "Access and look" and "System"; on a wide screen, no cut captions.
  "For developers" has the layer switch and the hidden options, without a list of shortcuts.
- **Shortcuts on the desk.** MagicPlace and "Folders" in one tap; more with "+ Shortcut", "Shortcut on the desk" on an
  app's page, or by voice: "add the calendar shortcut", "remove the radio shortcut". The MagicPlace widget (updates,
  new) no longer puts itself on the desk; whoever wants it: "+ New widget" → MagicPlace.
- **The panel remembers the person, not the browser.** Shortcuts, the folder open in "Folders" and the developer layer
  are kept on the gateway (the new `/api/v1/panel/state`, the `state` table, no migration): a phone shows the same as
  a computer. Theme and language stay with the device.
- **Updating from 0.12.0.** The first start adds three tables for counting model spending (migration 13) and the
  `app_settings` table for app settings (migration 14) to the database; existing data does not change. Before that the
  gateway puts a copy of the database into `backups/`.
- **Counting model spending.** Every model answer is written down as a deal: the seat, the model, why it was chosen,
  tokens and dollars at the day's price. Prices come from your own entry, the provider's list of models or the
  built-in models.dev snapshot. How the model is chosen did not change.
- **"Spending" in "Brain"** (Character → Brain → "Spending"). For today, the week or the month: dollars and tokens side
  by side, by day, seat and model, how much went at home (free) and by subscription (what it would cost by key);
  below, the deals with their reason, and you can keep just one seat. Tap a model to see its price: enter your own or
  bring back the snapshot's price (the new `GET /api/v1/router/prices/{provider}/{model}`). In the request journal, a
  request shows its seat, model and dollars. Dollars are an estimate, not a bill. Administrators only.
- **Long tasks in the background.** MCP tools marked "In the background" (Apify robots) no longer hold the
  conversation: the call becomes a background task. A paid one first asks "Run it?"; "how is it going?" and "cancel"
  work, and "Done" comes with a short retelling and "Tell you more?"; the result is a button on the panel's strip. A
  long app task started in a conversation reports itself the same way. Simple JS apps get `host.tasks`.
- **The "Works" page.** What is running, what waits and why, what asks a question (with answer buttons), and what
  finished lately; inside a work, its steps, deals and where it runs. The "Works" widget on the desk leads there too.
  Administrators only.
- **AirPlay on the speaker** is an app of the main MagicPlace catalog and installs only on the speaker: an iPhone sees
  it as an AirPlay receiver with the speaker's DuoWave name. When the phone plays, Alice pauses; "Alice, pause / stop
  / resume / next / previous" control the phone while AirPlay plays; ask Alice to play music and the phone pauses.
  DuoWave asks once for "AirPlay → Speaker player". It installs only from MagicPlace, never by itself; once installed,
  its shortcut appears on the desk by itself (once; a removed one does not come back).
- **"On the speaker" instead of "on the computer".** An app that works only on the speaker (AirPlay) is labelled so in
  MagicPlace and "Apps", and the MagicPlace "Where it runs" filter has a new "On the speaker". It has no "Built by the
  computer" line.
- **Apps that run all the time** (`"resident": true` in the manifest): they start at power-on and when the gateway
  starts, and are not unloaded when idle. For receivers a phone finds by itself.
- **New on the bus: "Speaker player"** (`system/player`): an app says `playing` (playing / paused / stopped) and hears
  player commands as the `control` news.
- **App supervision**: a Go program starts under a memory limit (`RLIMIT_AS` at least 4 GB, `GOMEMLIMIT` is the
  budget); a stop sends SIGTERM first and gives 3 s to exit; on the speaker the program knows the speaker's name
  (`DUOWAVE_STATION_NAME`).

## 0.12.0 — 15.11.2026

- **Updating from 0.11.1.** No migrations; the database does not change. If you update from 0.11.0 or older, first
  read "Updating from 0.11.0" in 0.11.1 below.
- **MCP: ready-made "Exa search" and "Apify".** Web search and page reading: Exa by an Exa key, Apify by its token;
  robots from the Apify store can be switched on (a run of one is paid).
- **Hidden options for developers.** Eight taps in a row on "For developers" in Settings (the list row or the page's
  title) open the "Hidden options" section; from the fourth tap before the end the panel says how many are left. The
  first option is "Show hidden packages": packages that feeds keep hidden (the new feed field `unlisted`) show in
  MagicPlace with a "hidden" mark, in the characters catalog and by voice, from the main catalog, stores and your own
  sources. An administrator changes it; a button hides the section. An installed hidden package stays in sight.
- **Feed and "Mr. Freeman (demo)" are hidden** from MagicPlace and the catalog: "Show hidden packages" shows them.
  Feed is no longer under "Recommended". Gateways before this release skip the field and show them as before.
- **Quitting the app turns off "My voice" too.** The server of your own voice (OmniVoice and others) no longer keeps
  running and holding memory after "Quit", Cmd+Q or logging out. A gateway restart and an app update still leave it
  alone, as before; the next start of the app brings it up again.

## 0.11.1 — 15.11.2026

- **Updating from 0.11.0.** No migrations; the database does not change. If you update from 0.10.0 or older, first
  read "Updating from 0.10.0" in 0.11.0 below.
- **Installing means turning on.** "Install" in MagicPlace, the first start and "install …" by voice install an app
  and turn it on at once: its widget, window and phrases work without a separate "Turn on". If it does not start, it
  stays installed, and you are told why.
  An app that is installed but turned off shows "Turn on" instead of "Open" in MagicPlace; in "Apps", "Turn on" comes
  first for a turned-off app, above the settings.
- **The catalog changed while the copy was fresh.** If the archive the catalog copy names is not on the site (404),
  the gateway reads the catalog again at once and installs what it names now; if that fails too, it says so in words.
- **"May stay empty".** An app setting with `"optional": true` is marked in the form, and the gateway does not wait
  for such a key (the card does not say "waits for a key").
- **"Recommended" for a package of a feed whose address stays private.** The catalog can recommend a package of your
  own source by the hash of its archive alone, without naming the feed's address (a `recommends` line without `feed`):
  where that source is added, the package is under "Recommended" and counts as checked, as long as the archive is the
  same.

## 0.11.0 — 15.11.2026

- **Updating from 0.10.0.** No migrations; the database does not change. With a speaker, the "On it: …" line now
  ends with the question "Anything else to add?", and the speaker listens for the answer after it. If you update from
  0.9.0 or older, first read "Updating from 0.9.0" in 0.10.0 below.
- **You can finish your thought** ("Microphone" mode). A phrase cut off on "and…", "so that…", "which…" waits 3.5 s
  for the rest; after "wait", up to 20 s, and the word "wait" itself does not go into the request; "yes" and "stop"
  work at once. A long wish of up to a minute is not cut off: the panel recognizes it in pieces and joins them. An
  unfinished thought does not leave as a scrap after silence: the panel says "Waiting.", and for two minutes the next
  "Say" continues it. The words follow the home's language, in the `gateway/locales/talk/<language>.json` files:
  Russian and English; other languages are added as a file.
- **You can add without the button.** After "On it…", "I'll add…" and after a question, the microphone listens by
  itself for 6 s: "and also…", an answer, "enough". Anything else said without "Say" (talk in the room, a new request)
  goes nowhere and is not written to the journal; the strip shows a hint. Before, any such phrase went to "Brain".
- **A question is not lost.** If you did not answer "To the desk or try it on?", nothing is done: the thing waits in
  "Waiting for you" next to "Say", and after the next conversation the computer asks once more.
- **Two microphone sounds**: "listening" and "heard". The switch is in Settings → "Microphone".
- **"On it: … Anything else to add?"** After this question the microphone listens by itself; "no" closes it silently.
- **Choosing the microphone and the sound output**: Settings → "Microphone": where to listen from and where to speak
  to; by default, automatic, as in the system.
- **"Done" does not interrupt**: it waits until the panel finishes saying an answer or hearing a phrase.

## 0.10.0 — 15.11.2026

- **Updating from 0.9.0.** No migrations; the database does not change. The "Catalog" tab under "Apps" is gone: the
  catalog now lives in MagicPlace, and the old addresses lead there. If you update from 0.8.x, first read "Updating
  from 0.8.1" in 0.9.0 below.
- **MagicPlace, the app store.** Its own icon in the dock (with the number of updates), a small widget on the desk and
  its own window: search by name and description in every language of an app; the "Updates", "Recommended" and "New"
  shelves; filters by kind, by the face of an app (widget, skill, window), by where it runs and by its state;
  "Install", "Update" and "Open" right there. Your own sources and "Do not load the catalog" are in its settings.
  "Apps" → "Catalog", the characters' "Catalog" and "New widget" lead here. The first start offers the recommended
  apps in one tap and says how much they will download.
- **MagicPlace by voice.** "Open the store", or any store name you know from a phone, opens MagicPlace in the panel;
  "install Feed", "install the character Arthur" get a question and a "yes"; "what was updated" and "update
  everything"; "what's new in the store". With the speaker and with the computer's microphone, in Russian and in
  English. The "To the desk by voice" switch became "Install by voice" and decides this too.
- **"Recommended" for a package from another feed.** Our catalog can recommend a package of your own source or of a
  partner's feed by the hash of the archive we looked at (the main feed's `recommends` field): where that source is
  added, the package goes under "Recommended" and counts as checked, as long as the archive is the same.
- Catalog previews update at once with a new package version: the panel no longer shows the old picture and phrases
  for up to an hour.

## 0.9.0 — 15.11.2026

- **Updating from 0.8.1.** The first start adds a table of conversation works and two fields of the request journal to
  the database (migration 12); before that the gateway puts a copy of the database into `backups/`. Node apps (Feed)
  download the gateway's own Node on the first install, about 50 MB, once. A spoken "yes" now puts a finished thing on
  the desk; if you don't want that, use the `talk.desk_by_voice` switch. If you update from 0.8.0, first read "Updating
  from 0.8" below: the home folders got English names.
- **Making things in your own words.** "I'm learning Spanish, give me a widget with words of the day": "Brain" calls
  the workshop itself and passes on the whole wish; the "make a widget…" template is not needed (but it still works
  without a model). The model no longer dictates a phrase you have to repeat; instead of "I can't do that yet" it
  offers to make it. Answers come in the language of the person talking.
- **"Brain" knows what is being made.** Every request carries a short "Now" block: what is in the works, what waits
  for an answer, what was done lately, what is on the desk. You can see it in "Brain" → "What Brain knows now".
- **Add-ons instead of cancelling.** "And a button too…" during a build is a note to the same work: the agent reads it
  between steps, and the tile shows a check mark. A new request about the same draft no longer cancels the running
  work.
- **Editing a finished thing** makes its new version, not a second draft. A thing already on the desk updates at once
  if it needs no new rights; "put it back as it was" within 15 minutes rolls it back.
- **An honest "Done".** The gateway checks the draft itself: "…: done. To the desk or try it on?" or "not finished.
  Go on?". The line is short; the agent's words come with "tell me more" and on the tile. A "Done" that was cut off
  plays once more after the next conversation.
- **A spoken "yes" puts it on the desk** (the `talk.desk_by_voice` switch): the rights and hosts are named in the
  question; dangerous rights are approved only in the panel; without a key, the app goes on the desk marked "waits for
  a key" and opens a window with the fields.
- **Without a model:** "how is the widget?", "and also…", "try it on", "to the desk", "tell me more", "cancel the
  widget", and the "workshop" door, where every phrase is a request or an addition until you say "leave the workshop".
- **Panel:** an unrecognized phrase goes into the conversation in "Speaker" mode too (the answer is text on the strip);
  in "Microphone" mode the panel says the "Done" of its own work aloud and listens for the answer. The "Journal" shows
  what opened a phrase, the time to the first word and the model's tokens.
- **Faster and cheaper:** a spoken answer is at most 600 tokens, the workshop agent caches its history (Claude), and
  ready phrases in your own voice are stored in advance, so "On it." sounds at once. The conversation is remembered
  while the work runs and for 30 minutes more.
- **Things are done only by deeds.** A phrase about things ("make the words bigger in Spanish", "and a button too",
  "leave workshop mode") is first read by the model into an action, and the gateway does it and says it. "Brain" can
  no longer say "Done" or "On it" without a deed: results and refusals are the gateway's phrases, and the conversation
  history holds the state of the work, not our phrase.
- **The workshop does not make things from scraps:** for "Wo", "making it?" it says "Couldn't make that out. What
  should I make?"; leaving the workshop is understood in any words. A thing's name follows its meaning ("Dollar
  rate"); after the build it comes from the manifest.
- **The gateway asks "Fix … or a new one?"** and understands the answer itself: "a new one", "fix it", "yes", "no".
  The same for "Make your own copy?" for a built-in app.
- **"Done" only if the agent changed something:** otherwise "…: nothing changed. What should I do?", and the same on
  the tile. "To the desk" while a thing is being made puts it on the desk when it is ready.
- **A limit is a queue, not a refusal:** "Something else is being made, I'll start right after: weather", "I'll start
  in a minute: dollar rate"; the work starts by itself, and "how is…" about it gets "in the queue".
- **An edit shows:** "Fixing "Clock": version 2. I'll tell you when it's ready."; the tile of a thing from the desk
  says "Edit: …". "You can add details as it goes: just say what else to improve or add" comes the first time in a
  day.
- **Apps keep their own words.** "Stop the music", "play the radio", "show the news" go to the apps themselves again:
  the layer takes for itself only things made by voice and "in the works" right now: being made, waiting for an
  answer, or talked about in the last half hour.
- **A removed thing is removed for the conversation too.** Take an app off the desk and its work is "in drafts";
  delete the draft too and it is "removed": "Brain" knows it, it can no longer be found by name, and "how is…" about
  it gets "…is gone, it was removed. Make it again?". Everything goes by the thing's ID; what was deleted while the
  gateway was off it checks at start. "Brain" sees the apps on the desk with their IDs. Of two things with one name,
  the talk is about the one being made or the one talked about last; "Which one: … or …?" understands "the first",
  "the second", "the last".
- **An app on the home screen right under the speaker.** An app's widget can stand right under the speaker (or under
  Deevi), across its full width, until you move it; on a phone it comes first after the speaker, and without a speaker
  under the first steps. Feed stands like this. Everything an app needs to be allowed (model, voice, messages, memory)
  it asks for in one "Allow all" sheet, not letter by letter. An app can write with its own model, any of those
  connected in "Brain", not only "Fast" or "Smart", and save its settings (a service key) right from its window.
  For authors: `under`, `rights()`, `askRights()`, `saveSettings()` and the `system/brain` → `models` letter in
  `docs/plugins/AUTHORING.md`.
- **Apps on Node.** An app can be a Node script (`"runtime": "node"`) instead of a build for every system. The gateway
  keeps its own Node, one pinned version of Node 24: it downloads it the first time such an app runs (about 50 MB,
  once) and checks it by SHA-256; a Node already on the computer is left alone. The gateway installs the app's
  dependencies from its `package-lock.json` without install scripts. Computer only; such apps install from the signed
  catalog or from the terminal, not from the workshop. For authors: "An app on Node" in
  `docs/plugins/AUTHORING.md`, example: `docs/plugins/apps/hello-node/`.
- **Video and sound in an app's window.** An app's window plays clips and sound from the app's folder with seeking; a
  70 MB episode is not loaded into memory whole (`useMedia` in the window SDK). The app's program knows its folder
  (`DUOWAVE_APP_FILES`); the window gets an address from the viewer engine, with a key for one file and six hours
  (below, "One viewer engine").
- **A model, a voice and hearing for apps.** An app can write text with a "Brain" model (`system/brain` → `write`,
  "Fast" or "Smart", the answer as text or JSON), run an agent with the files of its folder and its own tools
  (`agent`, with a cap on steps and minutes), voice a line into a file in its folder (`system/voice` → `synth`) and
  recognize speech in its file: the text, the language and the time of every word (`system/hearing` → `recognize`).
  The app does not see the keys of models and voices; the app's journal shows which model wrote and how many tokens,
  with a cloud model in amber. Every voice has a license and says whether a clip with it can be published.
- **Cloud voices: ElevenLabs with your own key.** "Voice" → "Cloud voices": you paste the ElevenLabs key in the panel
  and it is kept in the gateway's safe; "check" shows how many voices and plan characters are left. Then apps can
  speak with ElevenLabs voices too, with the time of every word; the gateway decides by the voice who voices a line.
  The text goes to ElevenLabs and spends the plan, so the app first asks for your "yes"; in the "Allow all" sheet this
  row is amber. For authors: "The gateway's capabilities layer" in `docs/plugins/AUTHORING.md`.
- **The catalog: what an app does, before you install it.** An app with code from a signed feed gets a "Signed by
  DuoWave" mark, its rights in words ("Brain model: writes texts", "network: api.example.com") and a "Built by the
  computer" line if its program runs on the computer.
- **Publishing from an app without leaving DuoWave.** The `system/publish` letter: `telegram` sends a clip from the
  app's folder with the gateway's Telegram bot to a channel, a contact or the "Notifications" chat; `watch` makes a
  "watch and choose" page on the gateway (`/watch/<key>`): a clip with seeking and a choice of ①/②, one vote per
  browser, and the votes go back to the app. The page runs nothing and lives 30 days.
- **The home's interests for an app, after a "yes".** The `system/memory` → `interests` letter gives an app only what
  memory knows about the home's interests (what people like to watch and read), and only when memory is on; the first
  time, the owner answers "yes" in the panel. Each read is a line in the app's journal.
- **An app as an MCP server for your own Claude Code.** `/mcp/apps/<id>` serves the app's tools and the files of its
  folder (with the administrator key): this way an app can work with a person's Claude plan through their Claude Code.
- **"Share" and recording in your own voice, from any screen.** An app's window or a panel screen ("Folders") opens one
  "Share" window: the phone's system menu, "Save to 'Videos'", "Download", "Copy text"; the gateway's senders, "To
  Telegram with the gateway's bot" and "A 'watch and choose' link"; and the upload pages of TikTok, Reels, Shorts,
  Telegram and VK. The list comes from the gateway; an app cannot slip in its own link. The journal remembers what went
  where: for the app, in its journal; for the person, in the "Links" journal under their name. Recording in your own
  voice uses the panel's recording window: into the app's folder or into a home folder you can put files in; the app's
  window never gets the microphone.
- **Node apps speak the home's language.** An app's program learns the home's language from the gateway
  (`GET /api/v1/apps/host/locale`), the way a backend uses `host.i18n`.
- **One viewer engine: photos, video and sound.** "Folders" open photos, video and sound in the panel's viewer: photos
  full screen with paging (arrows, swipe), video with seeking and full screen, sound; photos and videos in the list
  have a thumbnail and a cover frame, video and sound a duration. The same engine shows app windows (`useMedia`, and
  the new `openMedia`, which opens the app's file in the panel's viewer: Feed opens an episode full screen from the
  same second this way), card pictures, the computer's screen windows and what the speaker plays. A file's address is
  short, with a key for one file and six hours, and the bus checks the rights; the type is judged by the content, and
  nothing the browser would run is served. The cover frame is made by the computer's `ffmpeg`, or by Quick Look on a
  Mac without it. In the speaker's panel, video and sound in the helper window can now be seeked.
  For authors: "Photos, video and sound: one viewer engine" in `docs/plugins/AUTHORING.md`.
## 0.8.1 — 14.11.2026

- **Updating from 0.8.** The gateway does not change the database. Home folders now have English names, and the Russian
  folders of 0.8.0 stay beside them and are not moved. What this means is below, in "Home folders get English names".
  The line `ASR_LANGUAGE=ru` in `.env` keeps English away from "Hearing": remove it. A speaker installed from YAM
  updates with `yam install duowave/gateway@0.8.1`.
- **"Hearing" recognizes speech in the home's language: Russian and English.** The recognition language follows the
  home's language ("Settings" → "Appearance" → "Language"). Before, the hint to the model was always `ru`. GigaAM v3
  knows only Russian, Parakeet TDT v3 knows 25 European languages, Whisper knows 99. If the home is English and GigaAM
  is chosen, an already downloaded model that knows English listens. If there is no such model, "Hearing" and the
  "Microphone" readiness say which one is needed and download it with one button (Parakeet, 740 MB). The recognition
  server and the cloud get the home's language as a hint. A speaker that recognizes speech through the computer also
  hears the home's language. A Russian home works as before. The line `ASR_LANGUAGE` in `.env` wins over the home's
  language: if it says `ru` (as in `.env.example`), English does not switch on, so remove it.
- **The "Say" key is yours to choose.** In "Microphone" mode another program often uses ⌥Space. Now "Settings" →
  "Microphone" has the card "The Say key". Press "Change" and then your own combination (with ⌥, ⌃ or ⌘, or one of
  F1…F24). This computer remembers the combination; "Back to ⌥Space" restores the old one.
- **In "Microphone" mode, widgets address Deevi.** App hints are written for the speaker: 'Say: "Alice, turn on the
  radio"'. Without a speaker the panel writes "Deevi, turn on the radio" (Deevi is the English spelling of the name). A
  song or a folder named "Alice" stays as it is.
- **Without a speaker, the gateway does not remind you about it.** In "Microphone" mode, every 5 minutes a message came:
  "The speaker is not connected to the gateway… return to Yandex". It went to Mac notifications, to Telegram and to the
  webhook. Now, while the computer's microphone is chosen, the gateway does not wait for a speaker.
- **Home folders get English names.** At the first start DuoWave makes `Photos`, `Documents`, `Videos`, `Downloads`,
  `Pictures`, `Projects`, `Connected` and the links `Music` and `Stories` in the home folder. It does this in any
  language, as operating systems do. In the panel, in yes/no questions and in the voice of "Folders" they still have
  their interface labels ("Фото", "Документы", "Подключено"… in Russian); in English the labels are the same words. App
  addresses are English too: `files/home/Photos`, `files/home/Connected/<name>`. The "Photo album" starting point and
  "Folders" (1.0.5) already use them. **If you had 0.8.0:** the gateway does not move or rename the Russian folders. The
  English ones appear beside them, and you decide what goes where: you can move the files yourself in Finder. Until
  then:
  - "Фото" (Photos), "Документы" (Documents), "Видео" (Videos), "Загрузки" (Downloads) and "Картинки" (Pictures) are
    plain home folders. They are visible and work as before, beside the empty English ones. The panel and the voice will
    show, for example, two photo folders.
  - Apps do not see the "Музыка" (Music) and "Сказки" (Stories) links: music and stories are in `Music` and `Stories`.
    You can delete the old links; the music does not go away.
  - Projects in "Проекты" (Projects) are no longer projects: the gateway looks for them in `Projects`, and "Проекты" is
    now a plain home folder.
  - Connected USB drives and folders work and appear in `Connected`. But their ticks in the "Folders" section (who sees
    it, the model, the voice) go back to the defaults: set them again. You can delete the old links in "Подключено"
    (Connected).
  - App links to the old addresses (`files/home/Фото/*`) lead to the old folders.
  - The same happens on the speaker, inside its data.
- **The "…" menu of a painted card opens above it.** On a widget with a look (a gradient, a scene, a photo), the menu
  with "Wider", "Narrower" and "Remove from desk" went under the card's content: numbers and rate badges were drawn over
  the menu items. Now the menu is always on top. If it is longer than the widget, it also goes over the next widget, as
  on plain widgets.
- **The workshop survives a dropped connection.** The connection to the model can drop in the middle of an agent step
  ("Anthropic stream broke … Operation timed out"), or the service can be overloaded or out of reach. Then the step
  repeats by itself after 3, 10 and 30 seconds, instead of ending the whole run with "Failed". The drop happens before
  the agent does anything: its calls run only when the step has arrived whole, so nothing is done twice. What was said
  in the dropped step stays in the run's progress. Normal voice answers are still not repeated.
- **Painted cards read in any theme.** On a card with a look, buttons, their text, fields and switches now have their
  own colors and do not take the colors of the panel theme. Before, in the light theme such a card had a white button
  with white text. The panel picks the text color by the brightness of the background. If the gradient has a light edge,
  the panel dims that edge so the text stays visible. "Check" in the workshop names a look on which some text will not
  read.
- **A beautiful card only when asked.** The workshop agent dresses a card (a live scene, colors, a photo) when the
  person asks for it: "make it beautiful", "bright", "with animation". Without such a request, or with words like
  "simple" or "strict", it makes a plain card. A plain card follows the panel theme: light, dark or "Auto". A window's
  own styling is either all from the theme's colors or all its own, never mixed. The built-in apps stay dressed.
- **Workshop: slow network goes to the background.** An app's functions for a card, a phrase and a tool have 2 seconds,
  network included. An app that waited longer for a slow service failed with "did not finish in 2000 ms" on every call.
  "Check" now warns when a card or a phrase waits for the network longer than 2 seconds and there is no background
  schedule. The agent moves slow services to a schedule and keeps a stock of data in kv.
- **The speaker's panel does not lead to a computer that is off.** In the "Helper" widget, the "Open the computer's
  panel" button shows only while the computer is online. When it is not, a line shows instead: the computer's panel
  opens when DuoWave runs on it, and its address.
- **`duowave station status` starts with the speaker's own panel:** `panel: http://<speaker>:<port> (this station)`. The
  line about the computer is now `computer (helper): <address>`, and it says at once whether the computer answers
  (`answers` / `not answering: start DuoWave on that computer`).
- **The YAM package installs from the registry:** `yam install duowave/gateway`, and to update, `yam install
  duowave/gateway@<version>` (YAM 0.2 has no `upgrade` command; the README, the panel and the gateway now say so). The
  install prints the panel's address once, the real one, and where the speaker's voice goes now. Before, it always said
  "at Yandex".
- **`duowave station remove` removes everything:** the service marks (`gateway.started`, pid files) and the empty folder
  `/data/opt/duowave` too.
- **Yandex's ear: Yandex hears, your brain answers.** In "Together" mode without hearing of your own (no recognition on
  the computer and none in the cloud), the speaker's voice still goes to Yandex. But now Yandex only recognizes the
  question and waits. The router decides. Talk, your own skills and "can't do that" are ours, and the answer sounds in
  the chosen voice (Alice's voice by default). What the routes give to Yandex, Yandex does itself, as before. The
  "Yandex hearing" switch is on the mode's card and is on by default. When it is off, Yandex both hears and answers. If
  the firmware does not wait and Yandex answers anyway, its answer does not reach the speaker, and the gateway's log
  gets a `yandexEarIgnored` note (`docs/plans/yandex-ear.md`). Not checked on a Station Lite 2 yet.

## 0.8.0 — 14.11.2026

- **Updating from 0.7.** The first start adds two tables to the database: the apps' long tasks and the folders of the
  home folder (migrations 10 and 11). Before that the gateway puts a copy of the database into `backups/`.
- **Languages: Russian and English.** The panel speaks the language you choose. At the first start it follows the
  system. The switch is right on the first setup and sign-in screen, and later in "Settings" → "Appearance" →
  "Language". The language changes at once, without a reload. This device's language and the home's language are
  different things. The home's language (apps, schedules and notifications speak it) is set at the first setup and
  changed on the same card. A new language is added as a dictionary folder (`docs/plans/i18n.md`, "How to add a
  language"). The speaker's voice is Russian for now. The gateway's own texts are in two languages too (below). If a
  language or a translation is missing, English is used.
- **The gateway answers in your language.** The gateway's errors and messages in the panel, about 1,800 texts, are in
  Russian or English, following the panel's language. For a gateway installed before languages, the admin's panel sets
  the home's language itself.
- **The Mac app in two languages.** The menu by the clock, statuses, tips on failures and the splash screen follow the
  home's language. While the gateway is not running, they follow the system's language.
- **The catalog in your language.** The catalog feed carries the names and descriptions of packages in their languages.
- The speaker's voice stays Russian: the speaker and speech recognition understand only Russian (`docs/plans/i18n.md`,
  "Deferred").
- **Apps in several languages.** The manifest says which languages the app speaks (`languages`) and carries translations
  (`locales`): the name, the description, the labels of settings, cards and schedules, and voice phrases. The panel
  shows the app in your language. The app's code knows the home's language (`host.i18n`), and its window knows the
  panel's language (`useT`, `useLocale`). The workshop agent makes apps in Russian and English at once, and "Check" says
  what is missing in the translations. The built-in apps have English names and settings.
- **Juicy widgets.** A card now has a look, `look`: a gradient of its own colors, its own photo as the background, or a
  live panel scene behind the content. The scenes are sun with rays, clouds, rain, a thunderstorm with lightning, snow,
  fog, stars with a falling one, northern lights, waves, bubbles, sparks and a neon grid. The look paints the whole
  widget together with its header. The scene stands still when the widget is off screen and with reduced motion. The
  built-in apps 2.1.0 are dressed. The weather shows the sky outside (by day and by night, in heat and in frost) and
  icons for the days. Currency rates show the dollar large on emerald with sparks. The radio shows waves while it plays,
  shopping shows bubbles, and the calendar follows the time of day.
- **"Neon snake", a built-in 3D mini-game.** A tile on the desk: the board spins and the snake plays by itself. "Play"
  opens the game: arrows, swipes or buttons on the screen, sparks, gold stars, sound and a high score.
- **The workshop agent makes things beautiful.** It gives every card a look that fits its meaning, and "Check" reminds
  you about a card without a look. The widget starting point is dressed already. The agent makes games and live graphics
  as a window, after the model of "Neon snake": the snake's whole code is in its reference (`docs`, topic `game`).
- **The app waits for the gateway.** Gateway calls from an app's code (`host.files.*`, `host.bus.*`, `host.http.fetch`)
  answer with a promise. Without `await`, the function answered before the call ended: a picture was never downloaded,
  and the card stayed empty. Now "Check" in the workshop warns about such a call and gives its line number. The gateway
  writes to the app's log when a function answered without waiting for a call, and the agent's reference writes `await`
  everywhere. A card without its picture says "Picture not found" instead of showing an empty space.
- **The workshop agent finishes big apps.** It has its own limit on answer length, apart from voice answers:
  `WORKSHOP_MAX_TOKENS`, 16,000 by default (it was 4096, shared with the voice's `LLM_MAX_TOKENS`). It writes a big
  file, such as a window's `panel.js` or a long `backend.js`, in parts: the first piece, then the next ones
  (`append_file`). If an answer is still cut off on a long file, the work does not fail: the agent hears that the file
  did not fit and goes on in parts. A model for which this limit is too big answers with the usual one. One request gets
  more time and steps: 30 minutes and 40 steps instead of 5 minutes and 16.
- **Long tasks for apps.** An app (`kind: app`) can run work that takes minutes, such as making a clip or unpacking an
  archive, without hitting the 2-second limit of a call. It starts a task, reports progress in percent and words, and
  ends with a result or an error. The app's window shows the progress and a "Cancel" button (`useTask` in the window
  SDK). One task per app at a time (`budget.tasks`, up to 4); a result is kept for a day. If the gateway restarted or
  the app crashed, the task is honestly marked "interrupted" and does not go on by itself. Tasks do not move to another
  computer in a move. For authors: "Long tasks" in `docs/plugins/AUTHORING.md`; the example is "Long count" in the
  reference app.
- **The DuoWave home folder.** In the DuoWave app it is a plain folder, `~/DuoWave`. You see it in Finder, and in the
  menu by the clock as "DuoWave folder". It holds Photos, Documents, Videos, Downloads, Pictures, your own folders and
  Projects; the home's music and stories show there as links. The voice, the agent and apps work only in it: the rest of
  the disk does not exist for them. What lay in the gateway's old folder moves there at the first start. To use another
  place, set `HOME_FILES` in `.env`.
- **"Connected": a USB drive or a folder from the disk, if you want.** Plug in a USB drive, and a tile on the desk asks
  "Look at what is on it?"; the speaker asks aloud if it is free. Say "yes", and the drive goes into "Connected" in the
  home folder as a read-only link. Unplug it, and it is gone; plug it in again, and it is recognized by itself. A folder
  from the disk is connected by dragging it onto the DuoWave window or with "Connect folder…" in the "Folders" section.
  Everything stays in its place. What is created or downloaded goes into the home folder as a real file.
- **The "Folders" section in the panel.** It shows what is in each folder, with thumbnails of photos, and who sees it
  (you, family, guests). It also shows whether a cloud model may read the folder and Alice's voice may name its files
  (in amber if this leaves the home), and whether the folder is open outside the home. Without the ticks, a cloud model
  gets neither names nor text from the folder, and a speaker with Alice's voice says only numbers.
- **The "Folders" app goes through the home folder by voice:** "what's in the Photos folder", "open the Recipes folder",
  "show photos from Holiday", "read the borscht file", "find a borscht recipe", "write pancakes… into the Recipes
  folder", "what's on the USB drive". Then, without "Alice": "next", "previous", "back", "number three", "enough". A big
  card on the desk shows the same place.
- **For apps:** `host.files` searches by name, reads the text of documents (txt, md, rtf, docx, pdf), shows a picture
  shrunk, plays a sound from a folder on the speaker and copies. Lists come in pages. A card shows a photo from the home
  folder (`picture: {address}`), and a window uses `useFolderFile`. A replaced file, like a deleted one, stays in the
  bin for a week. Each folder has its own fence: for apps, a link from it to the outside is not a file. The workshop
  starting point "Photo album" shows the photo of the day and a window with all photos.
- **Windows on the computer's screen.** Photos, videos and sound from the home folder, opened by voice or by the agent,
  appear in a separate window on top of all others. This happens even when DuoWave is minimized to the menu by the
  clock. The window is temporary: the next request takes its place, and a video closes when it ends. "Keep it" keeps the
  window until you close it with the cross or by voice. You can open several at once ("open these two photos and the
  video") and then say "keep this one, close that one". The window has "Keep", pause and a cross. The agent plays video
  and sound by address, pauses and stops them; apps reach the screen as `device/screen`. In conversation, "Folders" now
  also understands its usual phrases ("what's in the Photos folder", "open the photo").
- **"Links" are clearer:** address patterns are in words ("all home folders"), and the rights a built-in app got at
  installation are visible and can be closed.
- **Fixed in the guide:** the default voice is Alice's (`TTS_PROVIDER=yandex`) until you choose your own in the panel.
- **"Microphone" mode: DuoWave without a speaker.** You can talk to DuoWave with the computer's microphone; no speaker
  is needed. The choice "How to talk to DuoWave" ("With this computer's microphone" or "Through the speaker with Alice")
  is asked at the first start, after the password. It is changed in "Settings" → "Speaker" (in "Microphone" mode the
  page is called "Microphone"). Press "Say" or ⌥Space and ask: the answer appears on the bar and plays from the
  computer's speakers in your own voice, phrase by phrase. If the answer ends with a question, the microphone listens by
  itself for six seconds. A new "Say" or "×" on the bar cuts the answer off. The same "Brain" answers, with its
  personality, memory, weather, news, apps and the MCP servers open to voice. The conversation is shared with the "Ask
  the brain" chat. Music, radio, stories, timers, alarms, volume, lights and calls are not on the computer yet.
- **In "Microphone" mode the panel has no Alice, Yandex or speaker.** Deevi is in the middle of the home screen, and the
  built-in helper is called Deevi too. The mode is "Own" and is locked: the gateway does not switch to "Yandex" or
  "Together" until you choose "Through the speaker with Alice". Then the previous mode and everything else come back.
  For people who already use a speaker, nothing changes.
- **"Microphone" mode needs a voice of your own.** Alice's voice goes through the speaker and is silent without it. The
  readiness list (hearing, voice, brain) shows what is missing and leads to the place where you choose it. Without a
  voice, the answer stays as text on the bar.
- The right "Change Alice's personality" is now called "Change personality".

## 0.7.1 — 14.11.2026

- **The catalog is signed with two keys, like updates.** The package catalog keys are built into the gateway
  (`gateway/packages.pub`): the main key and a reserve key kept offline. Plugins and apps from the catalog install if
  either of them signed the feed. If the main key is lost, the catalog moves to the reserve key without a new gateway
  release, and the next release removes a leaked key from the list. `PACKAGES_PUBKEY` still replaces the built-in keys
  and can now be a list (one key per line). The build variable `DUOWAVE_PACKAGES_PUBKEY` is no longer read.
- **Fixed: the speaker swallowed the start of an answer** when the answer took longer than three seconds to prepare. Of
  "Great, everything's fine. And how are you?" only "you" was heard. While an answer is being prepared, the gateway
  keeps the speaker busy with silence. The speaker plays the stream by its own clock and skips sound that arrives later
  than its place. Now the gateway catches up with the speaker's clock with silence before each phrase, and the phrase
  plays whole. This works for the first phrase after a wait, and for the next one too if it took longer to prepare than
  the previous one took to play. The answer's log shows how much silence this needed.
- **Fixed: on macOS 15 your own voice (OmniVoice and the other voice lab models) did not start**, with "the voice server
  did not start: taskpolicy …". The gateway set a memory limit for the voice server with `taskpolicy` flags that macOS
  15 does not have. Now the gateway checks what `taskpolicy` understands. On macOS 15 the server starts with a lower
  priority but without a limit from the kernel; the server still watches its own memory. And on any system: if the voice
  server or a voice imprint under such a limiter (`taskpolicy` on a Mac, `systemd-run` on Linux) ends at once, in the
  first seconds, the gateway starts it once more without the limiter and writes this in the log.
- **The "Auto" theme.** From the first start the panel takes the system theme: light by day, dark in the evening and in
  the dark. It changes with the system without a reload. "Light", "Dark" or "Auto" is in the avatar menu and in
  "Settings" → "Appearance"; the choice is remembered.
- **Fixed: the panel address for a phone had the port twice** ("…:7780:7780/panel/") in the last step of connecting.
- **Fixed: the draft window in "Workshop" did not show the app.** It showed "Cross-origin script load denied…" instead.
  The app's window lives without an address of its own, so for a draft its panel part loaded as foreign content. The
  gateway did not allow this, as it already did for installed apps. Now a draft opens on the computer and through the
  helper on the speaker.

## 0.7.0 — 13.11.2026

- **Updating from 0.6.** The first start adds the links journal table to the database (migration 9). Before that the
  gateway puts a copy of the database into `backups/`. People, settings, speakers, the journal, characters and themes
  stay as they were. Apps with code and drafts for Host API 1 do not switch on: remake them in the workshop or delete
  them. Set up the speaker again after the update: the built-in apps on it update together with the gateway.
- **"Links": apps talk to each other by your rules.** An app, a folder, a device and a gateway service are participants
  with an address. One app can call another, tell it news or ask it in words, but only if you allowed it. What an app
  did not ask for does not exist for it. What it asked for in its manifest asks for "yes" at the first use, as on a
  phone: "Allow" and "Don't allow" are remembered, and you change them in the app's window, in "Links". Risky things
  (money, deleting, sending out, locks, cameras, access) that a person did not start are asked every time. A chain is
  never more than the one who started it: a guest cannot use an app to switch on what a guest may not.
- **"Links" in the app's window and in Settings.** They show whom the app talks to and at what level ("sees", "hears",
  "calls", "edits"), "Talks to others", and the mode "By rules", "Free" or "Isolated", for the home and for each app.
  There is a journal of who wrote to whom and how it ended, without the content.
- **Questions in words through the cloud, only on your word.** An app without an answer of its own answers with the
  "Brain" model and its own tools. If the model is in the cloud, this works only when "Settings" → "App links" →
  "Questions in words via the cloud" is on.
- **Folders for apps.** Each app has its own folder (with a `tmp/` that cleans itself). The shared home folders
  "Pictures", "Photos", "Videos", "Documents" and "Downloads" are available by permission; "Music" and "Stories" are
  read-only. What an app deletes from others goes to the bin for 7 days. On the speaker: up to 1 MB a file and 5 MB in
  all.
- **Cards.** A desk widget is now a card by default. The app gives data (a number with its change, a line, bars, tiles,
  a clock, rings with a countdown, shares, rows, buttons, a field, a switch), and the panel draws it itself. There is no
  app code in the panel; the workshop makes widgets as cards.
- **Windows.** Any app's own part of the panel runs in a separate window: without your sign-in, without the panel's
  storage and without the network, only through the panel. So showing a draft no longer asks for "yes", even a draft
  asked for from the speaker.
- **One way: Host API 2.** An app's rights are now of two kinds: where it goes on the internet, and what in the home it
  reaches over the bus. What it gives (phrases, tools, sources) no longer counts as rights. Its own storage and its own
  folder need no rights. The built-in apps are rewritten: their widgets (weather, rates, radio, shopping, calendar) are
  cards and have no code of their own in the panel.
- **The app's window remembers its size and place.** The first start opens it at 80 % of the screen width, in the
  middle, at the full height the screen gives windows. After that it opens where and as you left it, after an update
  too. If that place on the screen no longer exists, the window opens as at the first start. In a browser the panel
  works as before.
- **Without the mini-map, the lock and zoom sit at the bottom.** In a window 1280 points wide or narrower the desk has
  no mini-map. The lock and zoom row now goes down to the dock's line instead of hanging above it. It rises above the
  dock only where it would overlap it.
- **A new Deevi icon.** At rest Deevi has both eyes open; "Hi" is in motion. New app and menu icons.
- **Your own voice on macOS 15.** The gateway did not see the Mac's RAM on macOS 15 (it has no single swap counter) and
  counted only empty pages as free. On 48 GB with 90 % free, it said "The system is already short of memory" and did not
  take a voice imprint. Now the counters are read one by one, and swap is read from `vm_stat`.
- **Low RAM is only a warning.** A shortage of RAM (pressure, swap, video memory) no longer forbids taking an imprint or
  switching on a voice. The panel shows one warning and an "anyway" button. An imprint started "anyway" stops only at
  critical pressure or swap.

## 0.6.0 — 12.11.2026

- **Updating from 0.5.** The first start adds the memory and sleep tables to the gateway's database, and two package
  fields: which feed the package came from and whether it is hidden (migrations 6–8). Before that the gateway puts a
  copy of the database into `backups/`. Everything else (people, settings, apps, speakers, the journal) stays as it was.
  Apps' keys move from the settings into the safe. To a speaker still on 0.5, the computer passes them the old way until
  the speaker is updated. Characters' memory is on at once, and reflection is not. You switch them off in "Apps" →
  "Memory" → "Settings".
- **Own package sources.** "Catalog" → "Own sources": the admin adds a package feed by a link that someone gave them in
  person, after a warning that nobody has checked its packages. Its packages show in the catalog marked "own source ·
  *site*" and "not verified". Only characters and themes install from it, no code. You can have up to four sources and
  remove them in the same place.
  - A character with a voice of unknown origin (`voiceOrigin: unknown`) installs only from an own source and is marked
    "voice not checked". Such a character does not install from the catalog or the store.
  - A feed can hide a package (`"hidden": true`). Then the package is not in the catalog, and a copy installed from the
    same feed disappears from "Installed" and from the characters, and its code does not run. If that character was
    chosen, Alice speaks, and the character's voice changes to the backup one. The files stay: when the feed removes the
    flag, the package comes back. A feed cannot hide other feeds' packages.
  - An own source that answered 404 or 410 is closed: its packages leave the catalog, and the ones installed from it are
    hidden.
- **Characters' memory.** The speaker remembers what it was told about the home: "Remember: Masha is allergic to nuts",
  "Forget about the nuts", "What do you remember about Masha?". This works at once, without a model. In conversation the
  character also remembers things by itself, in passing (you can switch this off). The main things from memory are
  always in the character's prompt; the character finds the rest when asked. "Don't remember anything until morning"
  pauses the memory. You cannot switch memory off or erase it by voice, only in the panel.
  - "Apps" → "Memory". "Overview": what is remembered, by whom, where and when, with "Undo" for each record, and
    "Through the character's eyes": what the character knows for sure. "Home": people, pets, rooms and places, their
    other names, who is linked to whom, and the history of what was before. "Settings": "Memory", "Remember in passing",
    "Sleep and reflection".
  - **Sleep and reflection** (reflection is off until you switch it on). At night, at 4:00 on the speaker's clock, the
    memory thinks over the day's conversations. It glues repeats, closes what is out of date, links other names, puts
    dates in place of "tomorrow" and moves what has passed into history. Then it writes a summary of the day for the
    home and the character's diary in the character's voice. The character remembers the summary and the diary in the
    next days. In the morning there is the "Overnight" tab: "Undo" for each change, and "Roll back the night" for the
    whole night. "Run sleep now" is there too and in the memory "Settings"; when there is nothing to run, the button
    says why.
  - **The memory model** is separate from "Brain": its own model at home (the default), the same as "Brain", or any
    model from "Brain", at home or in the cloud. The panel asks again before a cloud model.
  - Under a speaker's answer: "Remembered: … · Undo". You can put the "Memory" widget on the desk with "New widget".
  - Viewing memory needs the "View memory" right, and changing it needs "Change memory". The admin has both and gives
    them to others.
  - Conversations are kept word for word for 7 days. Each record remembers who said it and who could hear it: later the
    memory will be split between the people of the home.
  - The "Speaker memory" card (RAM and app storage) is now called "Speaker resources".
- **A MIDI keyboard for apps.** The gateway hears a keyboard plugged into the computer with DuoWave (USB, or Bluetooth
  on a Mac): in the DuoWave app, in any browser and from a tablet at home. The app needs the right "hear and play the
  MIDI keyboard, at home, on this computer". The keyboard is open only while the app's window is open, and the notes go
  only to that page: not to the journal, not to the agent, not out of the home. A keyboard plugged in on the go shows at
  once. The app's card shows "Listening to the keyboard now". If you close the window in the middle of a note, the
  gateway releases the note, so nothing rings forever. If the gateway has no keyboard and the panel is open in Chrome on
  `localhost` or over https, the panel offers "Connect through the browser".
- **"MIDI keyboard teacher".** Say "Make an app: a teacher for my MIDI keyboard", and you get a tile, "Try-on" with a
  keyboard on the screen ("Expand" opens the lesson window) and "To the desk". Choose a melody, and the next note lights
  up. Press it, and it sounds and the next one lights up; press a wrong one, and it flashes briefly. There are eight
  melodies, folk tunes and old classics, and you can add your own as a `.mid` file (you choose the track). The first
  time, the lesson asks how many keys the keyboard has (25, 32, 37, 49, 61, 76 or 88; a number from the name, as in
  "KeyStep 37", is suggested). It also asks which key is the leftmost and whether the keyboard makes sound itself. If
  you answered wrong, use "Keyboard again". "Listen": the panel plays the melody, or the keyboard does if it has its own
  sound. With the speaker: "Alice, let's learn the Christmas tree song", "what melodies are there". The speaker has no
  keyboard, so it says where to ask.
- **For authors:** the `midi` right, Host API 1.3; in the panel SDK `useMidi`, `useSynth`, `PianoKeys`, `readMidiFile`,
  `melodyOf`, `noteName`; the `midi` template (`duowave plugin new --midi`); `MIDI=0` turns keyboards off, and
  `MIDI_REMOTE=1` lets them work outside the home.

- **Say it, and the thing appears on the desk.** "Say" in the dock is now the panel's microphone. Say "Make a widget: my
  tasks for today from Todoist", and the "New widget" tile becomes that thing. The agent's purple comet flies from
  "Brain" and draws a dashed outline, and lines appear inside it step by step as the agent works. The desk and the
  speaker stay free meanwhile. When it is ready, the tile stays dashed ("on trial") with sample data. The bar above the
  dock says: "Ready: the 'Today's tasks' widget. It needs a Todoist key. Try it on, or put it straight on the desk?" If
  you asked by voice, the panel listens for the answer for 6 seconds without a press. The same works from "+": write
  what to show, and the tile does the rest.
- **"Try-on".** A window with the same tile on a piece of the desk. It has phrases for the speaker (press one or say
  "try it", and the answer comes from the sample), "What it needs" and "What it may do". You can say ordinary needs
  aloud ("show three"), but personal ones only by hand; amber marks what leaves the home. "To the desk": you approve the
  first install and new rights, with the "Becomes: …" badge and "Approve and install · Enter", and not by voice. Without
  new rights the thing installs at once. The tile takes its place as a real widget with a "new" mark for a day, and only
  now does it show your data.
- **Before "yes", a sample, not your data.** The agent writes a sample of network answers for the thing
  (`examples.json`). The tile and "Try-on" show it, and the draft does not go to the network. "Check" warns when there
  is no sample.
- **Keys only by hand, kept in the safe.** While a key field is open, the microphone is off; this shows in the window
  and in the dock. Apps' keys no longer lie as plain text in the settings: the gateway's safe keeps them, like model
  keys. In a move they travel sealed, and in the agent's journal they show as "•••". The first start moves the keys you
  already entered.
- **The speaker too: "Alice, make a widget…".** The speaker answers "Making a widget. I'll tell you when it's ready" and
  is free: a new "Alice" does not cut the work off. The draft appears on the desk as a tile marked "asked from the
  speaker". At the end the speaker says "Ready…" by itself and listens for an answer for about 5 seconds without
  "Alice". "Try it on" opens "Try-on" in the panel, and "to the desk" opens the "yes" window (the speaker itself does
  not install). You can also say "to drafts", "no" or "redo: …". While the agent works: "Alice, how's the widget?" and
  "Alice, cancel the widget". A busy speaker (playing, ringing) says "Ready" when it is free, within 10 minutes.
- **The panel's hearing is our own.** The gateway's "Hearing" recognizes the panel's speech, the same as the speaker's;
  the sound does not go to Google. The bar shows where the speech was heard: at home or in the cloud (in amber). What
  the panel did not understand goes nowhere: it shows "Not understood" and hints. The old "Say through the speaker" is
  in search: "Type to the speaker…".
- **"Skills" became "Apps".** The panel calls everything you install an app with faces: a widget, a skill, a window. In
  place of "plugin", the card says where the app runs: "in the gateway", "on the computer", "on the network". "Memory",
  "App schedules", the catalog, "+" and the gateway's messages speak the same way. The workshop, the authors' place,
  speaks as before.
- **Apps' history: any version can come back.** Each step of a draft (the agent's work on one request, with the words of
  the request, or a save by hand) and each install is kept by hash, as in git. The same content is kept only once. The
  workshop and the app's window have the "History" feed. There you compare a version with the draft or with the running
  version, return the draft to a step, or bring back an installed version: "Code only", "Code and data" or "Data only".
  A return is a record too, and you can undo it. Before each change the gateway saves the app's data (storage, tasks,
  pictures; not settings and keys). Merging versions: simple ones merge by themselves, disputed ones go to the agent,
  and if the agent is not sure what to keep, it asks you and gives options. Your own copy learns about a new version of
  the original app and offers to merge. By voice: "Alice, bring the weather back to how it was yesterday", "undo the
  last change in the weather", "bring the shopping list back to yesterday", "bring it back", "what changed in the
  weather?". Storage: 2 MB on the speaker and 200 MB on a computer; old versions are cleaned by themselves. The speaker
  can keep its history on the computer ("Where to keep history"). The disk keeps the running version and the one before
  it; the others are in the history.
- **Hearing works by itself: on any computer, after a reboot and after a crash.** The app recognizes speech itself,
  without a Python server and without a terminal. At the first start it downloads GigaAM v3 (274 MB, once; the file is
  checked by SHA-256 and resumed after a break). Then the model runs on this computer without the internet. While the
  model downloads, the home screen shows "Getting ready to listen" with a bar; if it fails, "Hearing did not start" and
  "Retry". The model runs in a separate process of the app. If it crashes or hangs, it comes back and hears the phrase
  again. If the graphics card fails, the processor recognizes. Three failures in 5 minutes give a pause and the reason
  in the panel. When the app closes or crashes, the process ends with it. A Mac on Apple Silicon computes on the
  graphics card (Metal), other computers on the processor. On an M4 Max a phrase takes 15–30 ms against 60–90 ms with
  the old server, with the same text. Another model (GigaAM v3 variants, Parakeet v3, Whisper) switches on in
  "Recognition" without a restart: it downloads and starts while the old one listens. A speaker in "Speaker + PC"
  listens with the same model. A model file damaged on disk downloads again by itself. The old way, your own recognition
  server at `ASR_BASE_URL`, stays: the app uses it if the address is in `.env`.
- **Your own voice without a terminal.** Silero and Piper voices work in the app without a server of their own. At the
  first choice the panel offers to install "Own voice (Silero and Piper)" (~250 MB on a Mac, ~450 MB on Windows and
  Linux). The app installs Python and the libraries itself, downloads the voice and switches it on. The voice server
  runs while such a voice speaks. If it crashes, it comes back; on failures the panel shows the reason and "Retry". When
  the app closes, the server ends with it. uv is now inside the app, so the voice lab does not need it installed by hand
  either.
- **"Start at login" is on from the app's first start**: after a computer reboot, the gateway and hearing come back by
  themselves. Switch it off in the icon's menu; off stays off.
- **"DuoWave site" in the app's icon menu**, right under "Open panel": it opens duowave.ai in the browser.
- **"Turn off the light in 10 minutes".** Any command can be delayed: "in N minutes …", "at 18:30 …", "tomorrow at 9 am
  …", from a minute to a day, up to 20 per speaker. When the time comes, the speaker carries out the words as if they
  were said now. For messages and calls, the gateway first asks again ("— right?"). "Cancel the delayed command",
  "cancel all delayed commands", "what is delayed"; in "Apps" → "Installed" there is the "Delayed commands" list with
  "Remove". A command more than 10 minutes late does not run, and you get a message instead.
- **Pictures in apps.** An app with the `files` right downloads pictures (PNG, JPEG, WebP, GIF) and shows them in its
  widget (`useFile` in the panel SDK). The speaker has little storage: up to 1 MB a picture and up to 5 MB in all; the
  workshop warns about this there. A move takes pictures along with app data: to the speaker, as many as fit, newest
  first.
- **Timed games: the speaker says "Time's up!" by itself.** An app can give 3 to 120 seconds for an answer
  (`continue.timeout`, Host API 1.2). The countdown starts when the speaker stops talking. If nobody answers, the
  speaker says "time's up" by itself, and the game goes on. The gateway puts up with two such silences in a row; at the
  third it ends the game itself: "Looks like nobody's here — finishing". The workshop has the "Timed quiz" starting
  point and a "Stay silent" button in the sandbox.
- **Apps act on time by themselves.** A rhythm in the manifest ("on weekdays at 7:30", "every 2 hours from 9 to 21") or
  a time from the code (`host.schedule.set`: "in an hour", "tomorrow at 18:00", "every day at 9") sets when. When the
  time comes, the speaker says the app's answer or a ready phrase, and `notify` sends you a message in the
  "Notifications" chat. The app's card has "Schedule": when the next run is, how the last one ended, "Run now", "Pause".
  "Installed" has the "App schedules" switch. The gateway keeps the safety limits: at most 12 speeches and 20 messages
  an hour per app, at most three late runs in a row after a break, and a pause after five errors in a row. Schedules
  move together with app data.
- **"Alice, stop" stops at once again.** With background noise in the room, the gateway missed the end of a phrase. A
  request dragged on for up to 15 seconds, and the stop worked when there was nothing left to stop. Now:
  - a phrase also ends when the recognized text has not changed for 2 seconds;
  - a stop word ("стоп", "хватит", "замолчи": stop, enough, be quiet; with "Alice" or without) works as soon as the next
    recognition confirms it, in about a second;
  - "Стоп. Стоп." ("Stop. Stop.") is a stop too;
  - listening after a question takes the gain of the last "Alice", not the highest one, and the room's noise no longer
    counts as speech. The gateway remembers the gain of the last five "Alice" calls for each speaker, and a restart does
    not erase it. If there was no "Alice" in this connection yet (the speaker reconnected, or it is called by its own
    word), the middle value of those is taken. A speaker that has not heard "Alice" yet gets a moderate 30 dB.

  Settings: `ASR_UNCHANGED_END_MS`, `ASR_STOP_END_MS` (0 turns it off) and `AUDIO_GAIN_NO_WAKE_WORD_DB`.
- **Backup model.** In "Character" → "Brain", under "Fast" and "Smart", there is "Backup". If the chosen model did not
  answer (the plan's limit ran out, no connection, the service refused), the backup model answers the same question. It
  can be a model of any provider or the model from .env; the panel says where the question will go then. Without a
  backup model everything works as before.
- **Answers from a ChatGPT plan.** "Character" → "Brain" has a ChatGPT provider: "Continue with ChatGPT", and OpenAI
  models answer from your Plus or Pro plan. No API key is needed. The sign-in renews itself and is kept only on this
  computer. When the plan's limit runs out, the backup model answers if one is chosen; without it, the speaker says so.
  The provider's card shows the limit. How much DuoWave may spend is set in ChatGPT → Settings → Usage. From a phone,
  after sign-in the browser shows an address error for 127.0.0.1: paste that address into the panel. This is an OpenAI
  preview: the terms may change.
- **OpenRouter connects without a key.** In "Character" → "Brain", OpenRouter has "Sign in with OpenRouter". You sign in
  to your account, the key is created there and arrives in the gateway by itself, so you do not need to copy it. The
  panel on the computer with the gateway gets the key itself. On a phone, or with the gateway on a speaker, the
  OpenRouter page shows a code, and you paste it into the panel. The key shows in your OpenRouter account under the name
  DuoWave, and you delete it there.
- **A gateway installed on the speaker by hand finishes installing.** `duowave station install` run over SSH no longer
  hangs. The speaker's service starts apart from whoever started it, in its own session and without its output. So it
  does not hold the SSH session and does not stop with it. Before, the install waited forever, and the service lived
  only while the session was open.

## 0.5.1 — 11.11.2026

Fixes after 0.5.0: the computer updates the speaker again, the Lite 2 too. The "Helper computer" page no longer turns
light in the dark panel, and it opens what it promises.

- **The speaker updates from the computer, the Lite 2 too.** The computer sends the speaker the archive itself, not
  base64 in JSON, and the speaker writes the program from it straight to disk without holding it in memory. Before,
  the speaker's gateway ran out of memory during the update: the kernel stopped it, and the speaker went back to the
  old version. A speaker on 0.5.0 or older is updated by hand once, and the computer says so. After that it updates
  from the panel.
- **You can see how the speaker update goes.** "Watching over the speaker" says "The speaker is updating…", and if it
  fails, it says why: for example, "the link to the speaker's gateway broke halfway". Before, there was a short "The
  speaker is updating." and then silence.
- **"Open" on the "Helper computer" page works:** it shows "Where DuoWave runs" (copies, moving, what happened) right
  on the page. Before, the button did nothing.
- **The theme you pick in the app window is remembered,** and the "Helper computer" page uses the same colors. Before,
  the dark theme in the speaker's panel was not saved, and the computer's page stayed light.
- **The app no longer crashes on start** when the gateway answered before the window began to load.
- The new-version notice and the speaker's "who updates it" hint name the new places: "Settings" → "Updates and
  backups", "Settings" → "Helper computer".

## 0.5.0 — 11.11.2026

A rebuilt panel: four places instead of nine sections, one name for each thing, one panel on the computer that helps
the speaker, and a first run in four steps.

- **The panel is four places.** At the bottom: "Main · Character · Skills · Settings" and "Say". "More" and "+" at the
  dock are gone.
  - "Character": who answers, their voice and brain.
  - "Skills": what the speaker can do, connections, the catalog and "Who answers".
  - "Settings": a list, as on a Mac. The speaker (name and look, mode, hearing, where it runs), the helper computer,
    people and access, appearance, updates and backups, conversation history, "For developers".
  - At the top there is one line: the speaker, a link dot and its state ("online · computer helps"). Next to it are a
    ⌘K search across the whole panel and an avatar: theme in one tap, appearance, sign out.
  - Old links (`#/mind/…`, `#/house/…`, `#/journal`) lead to the new places.
- **"Character" is one page.** At the top is the one who answers, with "listen" and "Choose another". Below are the
  voice, the manner (answer length and other tones, right on the page), the brain ("think longer on hard ones" for
  Claude) and "Own voice". Behind "›" are all characters, all voices, models and providers. The "Voice" and "Brain"
  boxes on the main page lead here too.
- **"Skills" is one list.** What the speaker can do, and for each one who answers it: your own or Yandex. What it
  cannot do yet has "Teach": the workshop agent gets the phrase and a task. Then connections and programs (MCP).
  Installed items, the catalog and "Make your own skill" are behind "›".
- **One panel on the computer.** When the computer helps the speaker, the app window is the speaker's panel: main,
  character, skills and settings are the speaker's, through the computer, with no "speaker window inside a window".
  The computer's own things are on the "Settings" → "Helper computer" page: help the speaker, do not sleep, copies,
  its updates and models. "Computer brain", "Computer plugins" and the dark frame are gone. "Say" and the semicircle on
  the main page are now about the speaker, not the computer. If the speaker does not answer, the panel shows its copy
  and says so.
- **First run: four steps to "hello".** Find the speaker → check and connect → choose who will answer → "Say to the
  speaker: "Alice, what's the weather tomorrow?"" and the panel address for your phone. Name and look come later, as a
  card on the main page. If DuoWave is already on the speaker, there is no placement choice: the computer becomes its
  helper with one button.
- **Help without switches.** The computer has one switch, "Help the speaker". The separate parts are in "Fine-tuning"
  for developers. The settings copy shows its date: "today 14:02 ✓". There is one way back to Yandex: "Give the
  speaker back to Yandex" in "Settings" → "Speaker", and you can undo it. Moving messages name places that exist in
  the panel.
- **One name for each thing.** Modes are "Yandex · Together · Own". Personality is now "Manner", recognition is
  "Hearing", connectors are "Connections", routes are "Who answers", the journal is "Conversation history". "PC" and
  "helper" are now "computer" and "computer helps". The gateway's messages use the same words.
- **You can rename the speaker** at any time: "Settings" → "Speaker" → "Name and look". Before, you could set the name
  only during the first setup.
- **Yandex hearing no longer takes the speaker away silently.** Choosing it in "Hearing" asks first: Yandex then runs
  the speaker fully.
- **The new-version dot is visible** on "Settings" in the dock. In the "More" menu you could not see it.
- **The semicircle on the desk.** Around the speaker there are three nodes, as in the desk design: hearing on the left,
  brain at the top, voice on the right. The path of a question runs along this arc. Thinking opens from the "Brain"
  box, personality from the "Voice" box, and the character's ball flies into the speaker from the voice.
- **Widgets next to the speaker stay in place.** In the app, the widget columns on both sides of the speaker jumped by
  a fraction of a pixel while hearing and voice were lit (you could see it when the desk zoom was not 100 %). They no
  longer jump.

## 0.4.0 — 11.11.2026

A catalog from several feeds, and speaker settings that no longer change when you just look at the panel.

- **Extra catalog feeds.** The main catalog feed can name up to four more feeds, our store or a partner's feed, and
  the gateway reads them after it. A package from such a feed has a label with the feed's name in the catalog. You
  can install characters and themes from them. Plugins and apps not yet: there is no signature for such feeds yet. If
  a feed is not available, the catalog shows a line about it, and the main feed and the other feeds work as before.
  "Do not load the catalog" turns off all feeds at once.
- **Speaker settings do not change when you look at them.** The panel rereads the character list every 15 seconds,
  and each read wrote the characters from packages again, so the speaker settings revision grew once a minute. The
  computer made a copy for each one, and the ten kept copies covered only the last ten minutes. Now a character is
  written only when something in it changed, at gateway start too.

## 0.3.0 — 11.11.2026

A desk instead of the main screen, and a gateway on the speaker itself, helped by the computer.

- **Desk.** The panel is an endless desk. The speaker is in the center: you see who hears, thinks and speaks right
  now. Around it are widgets: timers, what is playing, weather, radio, exchange rates, calendar, shopping list, recent
  questions. You can move, zoom and hide them, and move around the desk with a map. Each person has their own desk,
  and the gateway keeps it. The dock is at the bottom, sections open as windows over the desk, and sheets and
  questions are always on top.
- **Widget by voice.** "Let's make a widget": the workshop agent draws it, the panel shows it live, and the "Put on the
  desk" button puts it there.
- **"Gateway" and "Helper".** The role shows in the panel frame. The gateway is the one that answers the speaker; its
  frame is white. The helper is the computer when the gateway runs on the speaker; its frame is dark. The helper edits
  the speaker settings right on the speaker, through the pairing.
- **"Speaker + PC".** Moving the gateway to the speaker and back carries the voice, plugins with their data, keys and
  the speaker name. A revoked key stays revoked. Before it applies the settings, the gateway makes a full copy of the
  database. In the speaker's "Voice" window you can choose the computer's voice.
- **Messages through the computer.** On a speaker with a helper, "Alice, send …" goes through the Telegram bot on the
  computer. Text and voice messages sent to the bot are read out and played by the speaker. One switch, "Messages" in
  "What I give the speaker", controls both directions.
- **The panel does not freeze with several tabs.** The panel's live channel is one WebSocket per tab, speaker events go
  through the computer, and polls do not overlap. Windows and sheets are opaque in every theme.
- **Plugins on the speaker.** A plugin takes the speaker's memory only while it is loaded, so turning one on no longer
  fails for lack of memory.
- **No false alarms.** While the gateway is on the speaker, the computer does not say "speaker not connected". The
  speaker's clock right after boot (year 1970) no longer counts anything down.
- **Weather 1.1.2.** The city is found by its dictionary form: "в Москве" ("in Moscow", a case form) is Moscow, not a
  random match of the start of a name.
- **Site: duowave.ai, mail: hello@duowave.ai.** The package catalog is built into every release:
  `packages.duowave.app/catalog.json`.
- **"Gateway" mode without its own recognition handles requests.** When the voice goes fully to Yandex, it is a request
  like the others: you see it in the panel and in the journal, with Yandex's words and answer. A speaker that
  reconnected and spoke at once no longer loses the phrase: the voice waits until the Yandex session opens (up to 5
  seconds).
- **Backup update key.** The gateway also trusts a second, backup key that is kept apart. Losing the main key will not
  leave gateways without updates.
- **Urgent updates install themselves.** A release that fixes something important is marked in the feed. A gateway
  below that version installs it by itself at the next check (once an hour), on its own and inside the app. The panel
  shows such an update at once: you cannot hide the line, and where the gateway does not update itself (Docker, Home
  Assistant, the package on the speaker), it says what to do. To turn it off: `UPDATE_AUTO_CRITICAL=false`. Normal
  updates install on a button press, as before.
- **A rollback to an old version does not install.** Each gateway archive holds `release.json` with the version and
  platform, under the same signature as the program. The gateway installs an archive only as that exact version,
  built for it and newer than the one installed: a forged feed can no longer take it back to an old release. The feed
  and archives come only over https.
- **A panic in one task no longer brings down the whole gateway.** The computer build unwinds the stack: the failed
  task ends alone, other speakers keep working, and the gateway can update. The speaker build is as before.
- **Docker builds from source.** The image did not get the speaker scripts that the gateway builds into itself, and
  `docker compose up --build` failed. Now `docker compose up -d` takes the ready release image. To build from source:
  `docker compose build gateway`, and CI checks that it has all the built-in files.

## 0.2.0 — 10.11.2026

The first open release: the code is under [Apache-2.0](LICENSE), the docs under CC BY 4.0
([docs/LICENSE](docs/LICENSE)). Contributions come under the same licenses, with no separate contributor agreement.
The DuoWave name and the Deevi mark are trademarks, and the code license gives no rights to them
([TRADEMARKS.md](TRADEMARKS.md)).

**New update signing key.** The update from 0.1.x to 0.2.0 will not go through by itself: you install it by hand, and
after that updates come as usual. The gateway now checks the signature against a list of keys (`gateway/updates.pub`,
one key per line) and also asks the feed at its own version's address, so the next key change will not need a
reinstall.

Everything below was gathered from the very start, in one list: the first release also carries what was not described
before.

- **The speaker is no longer edited where it cannot be rolled back.** The gateway does not change the firmware file
  `quasar.cfg`: a copy with the gateway address lives in `/data/opt/duowave` and is mounted over it at every boot
  (script `/etc/init.d/S95duowave`, before Alice). A speaker reset clears `/data`, and you get plain Alice, not a
  speaker that calls a gateway that is gone. A firmware update changes its file: the copy is not mounted, and the panel
  says the speaker was updated. The gateway will not rewrite or mount a copy where someone else wrote the address,
  unless you explicitly give it back. A speaker connected earlier is returned to Yandex once in the old way
  (`scripts/station-voice.sh legacy-yandex`).
- **Emergency phrases.** "Алиса, аварийный возврат к Яндексу" ("Alice, emergency return to Yandex") gives the speaker
  back to Yandex (copy removed, reboot). "Алиса, переведи шлюз в режим Яндекса" ("Alice, switch the gateway to Yandex
  mode") lets Yandex answer everything. The gateway hears them before routes, plugins and the model, and only as a
  whole phrase.
- **The gateway on the speaker is more reliable.**
  - It starts before Alice and waits up to 15 seconds for itself when the voice goes to it.
  - After a power cut it does not take another process for itself: the pid file holds the boot number.
  - A hung gateway (two minutes without an answer on `/health`) restarts. The third time in half an hour, the safety
    net steps in.
  - Autostart is written only after the started gateway answered.
  - A removed package or a stopped gateway does not leave the speaker mute.
  - After the safety net, "Try again" no longer takes its way back for someone else's edit of the copy, and "On the
    computer" rewrites the copy too: the next boot will not send the voice back to the speaker's gateway.
- The call client watches the loudspeaker after a call with an error. If it is stuck, Alice's media service restarts,
  then the speaker reboots, but not more than once an hour.
- The speaker firmware shows in the panel: whether the gateway was tested with it. A firmware change shows in "Change
  history". The update check on connect also sees a forged OTA key from the wiki.
- `/uni.ws` does not let browser pages in (the `Origin` header): the speaker does not send it.
- The speaker scripts are tested on a real kernel and busybox: in CI in a container, and on a button press in the
  KUYASE emulator on Station 2 and Mini firmware (`scripts/emulator`).
- **The panel is more honest about the speaker.** A speaker that rebooted or dropped off the network no longer hangs
  "online": the gateway pings its socket and lets it go after 45 seconds of silence. Before, such a session lived until
  the first Yandex push, sometimes 20–40 minutes, and the watchdog kept quiet. After a mode change, while the speaker
  opens a new socket (usually up to a couple of minutes), the panel says "reconnecting…", not "offline".
  `STATION_PING_MS` (15 s; 0 turns it off).

- **Confirmations work in the app.** "Update", "Delete", "Revoke key", "Sign out on all devices" and 14 more questions
  asked through `window.confirm`, and the app window does not show it, so the buttons silently did nothing. Now the
  question is a window of the panel itself, in any theme. Plugins get `ask()` from the panel SDK.
- "Minimal": the glow behind the speaker has no sharp corners in the app window (a gradient instead of a blur).
- **The gateway port is 7780, not 80**: port 80 is often busy on developers' computers. The speaker does not need port
  80: the gateway writes the address with the port to it on connect. The app, Docker (`GATEWAY_PORT`), the Linux
  service and the Home Assistant add-on use 7780. If your gateway is already installed, its `.env` and the speaker
  stay as they are. If the port is busy, the app offers "Take a free port". If you changed the port and the speaker
  still goes to the old one, the panel shows it on the main page and offers "Reconnect". A gateway address typed in
  the wizard without a port now means the gateway's own port.
- **New mark: Deevi**, a lime square with eyes that winks. It is in the panel (header, browser tab, "Powered by"), in
  the app icon and the menu bar icon, and on the app splash screen: while the gateway starts, Deevi listens and blinks.
- **New name: DuoWave** (site duowave.app): the program `duowave`, variables `DUOWAVE_*`, database `duowave.sqlite`,
  log `duowave.log`, on the speaker `/data/opt/duowave`, the app DuoWave (`app.duowave.desktop`), images and service
  `duowave`.
- First run: after the password, the panel opens "Connect the speaker" at once. While there are no speakers, the main
  page shows a "Connect your speaker" card (what you need, what the wizard will do, how to undo it) instead of a
  dashboard for a speaker that does not exist, and the header says "speaker not connected". The bare gateway address
  leads to the panel. The wizard names the real gateway port, not always 80.
- Characters: "Brain" → "Characters" instead of "Personality": personality, tone and voice in one choice. Characters
  install as packages from the catalog (the first ones are Arthur, Kovcheg, Palych, Bublik). Each has tone controls
  and "Fine-tuning" with the full text; your edits stay when the character updates. The characters' own voices are
  snapshots for OmniVoice, CosyVoice 3 and VoxCPM 2 (the third voice lab engine, Apache-2.0 for both code and weights;
  on a Mac it speaks on the graphics card). Until they are there, the character speaks with a fallback voice. The main
  page shows "Answering now".
- The built-in Alice is one card with "Irony", "Warmth" and "Detail" controls instead of "Standard", "Ironic", "Warm"
  and "Business". The old choice became her control positions by itself; at the default positions she speaks word for
  word like "Standard". "Butler" was replaced by the character Arthur from the catalog. For those who chose it, the
  gateway keeps the old text, and the panel offers to install Arthur.
- Package catalog: "Home" → "Packages" shows what is installed and what is in the catalog (`PACKAGES_FEED_URL`):
  install, update, remove, filter by kind. "Do not load the catalog" is a switch. While the catalog has no signature,
  only packages without code install from it: characters and themes.
- A voice installs its own model: a voice without a model or without a downloaded file offers "Download and turn on"
  and turns on by itself when the model is in place. You can re-record your own voice for another model from the
  saved recordings.
- Links to panel screens: `#/<section>/<tab>/<id>`. For example, `#/mind/characters/arthur?install` opens the character
  (or its catalog card) with an install button.
- Plugins: your own skills from packages in the gateway data folder (`DATA_DIR/packages`): phrases for the speaker,
  tools for "Brain", music and radio sources. Plugin code runs in QuickJS with memory and time limits and gets only
  what its manifest allows. The administrator turns plugins on (`/api/v1/packages`); `PLUGINS_DISABLED=1` starts
  without plugins. How to write your own: `docs/plugins/AUTHORING.md`.
- Built-in plugins: radio, weather, news, own music, fairy tales, games, the morning show and the smart home came into
  the gateway as plugins. You can turn them off, or make your own copy and edit it; the built-in skills stay as a
  fallback. New: "make up a fairy tale about …", "morning show" with music or radio, games by talking without "Alice",
  and the radio remembers stations where the speaker stays silent and plays similar ones.
- The "Plugins" screen ("Home" → "Packages"): system, built-in, your own and catalog plugins, each with its own
  buttons; memory against the budget, permissions and hosts, journal; "Roll back" to the previous version. A plugin
  can add a card, a tab or a theme to the panel; `?safe=1` opens the panel without them.
- Workshop (a panel section): the agent writes and fixes a plugin or theme from a description or from an API
  description (OpenAPI), tests it in a sandbox, runs the phrases and installs it as a new version. The administrator
  confirms the first install and new permissions. By voice: "Alice, let's make a skill". For the terminal:
  `duowave plugin new|test|install`; for Claude Code: the MCP server `/mcp/workshop`; plugin tools: the MCP server
  `/mcp/plugins`.
- If the speaker stays silent on a station for 5 seconds, the gateway opens the stream itself and passes it to the
  speaker through itself. It transcodes AAC to MP3 on the computer (on the speaker with
  `MEDIA_TRANSCODE_ON_STATION=1`). The request journal shows "started playing", "stopped", "silent for N s". Plugin
  tools play what they found at once.
- The gateway on the speaker keeps a memory budget for plugins: if a plugin does not fit, it does not turn on. Under
  memory pressure the gateway unloads idle plugins and turns off the last one turned on (`PLUGINS_RESERVE_MB`,
  `PLUGINS_PRESSURE_ACTION`). See "Home" → "Gateway" → "Speaker memory". A plugin with `"prefer": "home"` runs on the
  computer while the computer is online.
- The catalog is signed (minisign, `PACKAGES_PUBKEY`): from a signed catalog, plugins and apps install too. An update
  keeps the previous version for a rollback. A speaker without internet gets the catalog through the computer.
- Plugins on WebAssembly and from MCP servers. Apps are your own programs next to the gateway, under supervision
  (restart, memory, build hash), with a Host API over HTTP and their own sound on the speaker.
- Panel themes: a theme changes not only the colors but also the layout. "Glass" has tabs at the bottom, as before, and
  the new "Space" has a side bar. To choose: "Home" → "Access" → "Panel look".
- Where the gateway runs is your choice: on the computer, on the speaker itself, or on the speaker with the computer as
  a home server ("Home" → "Gateway" → "Where the gateway runs"). The move from the computer's panel is a task with
  steps over SSH, and the speaker build is built into the computer's gateway. You can also do it by hand:
  `duowave station install|point|status|remove` or the `yse` package. Both panels show what is available where (one
  gateway list), where the gateway runs, and the state of the speaker and the computer.
- Home server: the gateway on the speaker takes recognition, your own voice (and the voice lab voices) and the model
  from the computer while it is online. When the computer sleeps, the cloud or Yandex steps in. "Own" mode on the
  speaker works only with the computer: if the computer is gone, the gateway switches to "Gateway" mode by itself and
  comes back when the computer is online again, unless you chose otherwise.
- The main settings of the gateway on the speaker live on the speaker (the computer keeps the last 10 copies) or on
  the computer. The gateway on the speaker updates with a button in the computer's panel, without SSH, or
  automatically.
- Safety net for the service on the speaker: if the gateway does not come up, first the previous version is used,
  then the speaker's previous address. The computer's panel and alerts say so. "Give the speaker back to Yandex" is in
  the speaker's own panel too, without the computer.
- The app's tray has a line about the gateway on the speaker.
- Connections are set up in the panel, "Home" → "Connectors": Home Assistant, Telegram (token, contacts, "Wrote to the
  bot", alert chat), the weather city and news feeds. Each has its own sheet, secrets are masked, "Check" answers in
  words, changes apply at once, without a restart. What is missing says "not set up". Calls through LiveKit and SIP
  are "soon" for now: their sheet opens with an update. On the first start of the new version, the gateway moves the
  values from `.env` into the database by itself. After that it does not read `.env` for them and writes in the log
  which lines you can remove.
- News: a catalog of 37 checked Russian-language feeds on eight topics, your own feeds by address with a preview, OPML
  in and out. "Alice, sports news" reads the feeds of that topic.
- "Brain" opens on "Models": the tabs are "Models", "Personality", "Routes".
- The gateway is rewritten in Rust: one program, `duowave`, with the panel inside, instead of the Node gateway. Same
  `.env` and settings, same panel. Commands: `duowave serve`, `duowave health`, `duowave update [--check]`,
  `duowave version`. The macOS service writes its log to `logs/duowave.log`. The services, the Docker image, the HA
  add-on and the app start `duowave`. The Node gateway is removed and stays in git history (tag `node-gateway-last`).
- Own database from a clean slate: one file, `duowave.sqlite`, with numbered migrations and a copy before them. The
  Node gateway data is not moved: you set up the administrator, "Brain" and MCP servers in the panel again.
- Updates: the gateway finds a new version in the release feed, shows it in "Home" → "Updates" and installs it with a
  signature check when a person with the right to install updates agrees. The previous program stays for a rollback.
- The `duowave` package for `yse` also carries the gateway built for the speaker itself (`duowave gateway install`).
  It has not been run on the speaker yet.
- In the Docker image and the HA add-on, the end of a phrase is detected by loudness: Silero VAD is only in the PC
  builds that have it.
- Developer tools are `duowave` subcommands: `replay` (replays a voice recording through the pipeline and
  recognition), `journal` (live request flow), `animations` (light ring animations from the journal), `call-sim` (a
  speaker simulator for calls) and `livekit-token` (access to a LiveKit room). The last two are only in the PC build.
  For a phone, LiveKit access is also available through `GET /calls/livekit/token` from the home network.
- Connecting the speaker from the panel: network search, a read-only check, "What will change", connecting with a
  copy and rollback, "I'll do it myself" with one command, "Return to Yandex".
- Docker Compose: the gateway with the panel, the `full` profile (own recognition and voice on the CPU) and `gpu`.
  Calls work on Linux.
- A systemd service for Linux. `DATA_DIR` keeps all gateway data in one folder.
- An app for macOS, Windows and Linux: the gateway inside, the panel in a window, a tray icon, start at login. The
  computer does not sleep while the gateway runs.
- The `duowave` package for `yse` on the speaker, and an add-on for Home Assistant.
- Defaults for a new home: "Yandex" mode, request sound is not recorded, the journal keeps 7 days.
- Correct model and voice licenses on the cards in the panel.
