# Check by hand: work systems (part 2)

How to walk the stages of «Системы работ» by hand: our works as before (step 1), Claude
Code run in a terminal (steps 2–7), bb and T3 Code (steps 8–14), the sets of models and a limit (steps 15–18), T3
Code's stop through its MCP (step 19), the ways «с проверкой» and «с планом» (steps 20–22), one's own words (step 23),
`/mcp/home` for an agent (steps 24–26), and the speaker as the hub (steps 27–29). About two and a half hours, on the
computer. Steps 27–29 need the speaker: they are done only after your «да» to reach it, and nothing on it is
changed. Steps of a system you do not have are skipped: say which, and they stay open.

Walked by the agent alone on 27.11.2026, on the branch gateway (debug build, port 7790, a fresh state; nothing paid,
the speaker not touched):

- 1 and 8: the systems as listed — Claude Code ready; bb «не найден на этом компьютере»; T3 Code not ready with its
  own words (its server was not running), and no «стоп» offered while its MCP is not connected; DuoWave «рабочих
  папок пока нет» on the fresh state.
- 2: the plugin's two commands to install and two to remove are shown; not run.
- 3: two Claude Code sessions busy at that moment show as works begun outside, quiet; the eight idle ones do not, as
  only going work is a work.
- 15: the three sets with Claude Code's haiku, sonnet, opus.
- 23: before «запомни» the phrase went to the model step (no model on that gateway: it said so); after it, «глянь …»
  was heard with no model and asked «Какую: …?», as two works were in play.
- 24–26 by an agent's key over MCP: five tools; `say` «not said: no station is on the line»; `desk_card` a work begun
  outside; `ask_yes_no` its question («… спрашивает: Купить за 3000?»), «silence» with no answer in 5 s; `task_step`
  ends it. A summary without a stop read «…за 2900 Подробности в пульте.»: fixed (68c3108b) and seen right after.
- Not walked: everything that runs an agent (money, limits), the plugin in a terminal, answers by voice, bb, T3 Code,
  the speaker.

Walked with the owner on 27.11.2026 (the branch gateway after main was merged, port 7790; the plugin installed for
the trial folder only, `--scope local`):

- 1: passed (the typo fixed on the work's branch, ≈ $0.23 by the subscription on opus).
- 3: passed: the terminal's session shows «начата не здесь»; «как там …» answers with its state. The owner's
  Claude Code runs in `auto` mode, which asks nothing for edits: steps 4–6 need `claude --permission-mode default`.
- 4: passed after two fixes found by it: the question reached the gateway but the panel showed nothing (bb0c226d),
  and «Разрешить» on the strip said «Работа ничего не спрашивает» (cffde2e4).
- 5: not passed, left for later by the owner. «Добавь ещё: …» was not heard (fixed, c622048a); then, as other
  Claude Code sessions always go on this computer, the gateway asks «Какую: …?», and only the model reads the
  answer: with no model, «первую» or words of the work's title are not understood. Claude Code also refuses a long
  `sleep` in its turn and runs it in the background, so a turn to add words to has to be long by its own work.
  Rechecked after the fixes: passed — «добавь ещё: в конце допиши в poems5.md слово конец» was heard, «Какую?»
  answered «первую», and the words reached Claude Code by its hook in the same turn («Дописал строку «конец»»).
- 6: the gateway's side passed («следи за haiku»: «Слежу…», the end kept watched, not quiet); the strip in the
  panel was not looked at.
  Rechecked: «следи за haiku2» was taken as «следи за работой 2» (a word ending in a number read as a label):
  fixed, 46fde231. With «haiku3»: «Слежу за …», the end kept watched, and the panel was told «ready» at once; its
  strip stays 10 seconds, as for any work, and was missed while the owner looked at the terminal. The owner keeps
  10 seconds.
- 7: passed (the plugin and its marketplace removed).
- 8: passed after a fix (7057169f): T3 Code «Готова» while its server answers; bb «не найден»; no «стоп» while
  its MCP is off. 9 does not apply to T3 Code (it lists no projects).
- 10, 11, 14 (T3 Code, with a model chosen in the test gateway — phrases need it): passed after fixes —
  T3 begins nothing without a model (82a2f159: the set's model, its provider Claude), its refusal said as it is
  (f7d4fb8a), the system named in the phrase taken whatever the model passed (50ccfdf6, 3d6c5ed8); the result is
  T3's last message. «Открой» does not apply (the card has no open).
- 12: passed: «Добавить в … или начать новую?», «добавь туда» reached T3 as its next turn.
- 13: «как там» passed (T3 tells nothing on the way: «Жду ответа сервиса»); «стоп» before step 19 «Остановить
  отсюда не могу. Откройте T3 Code.»
- 19: passed after a fix (3b9ce889: a refused stop was said «Остановлено»). T3's MCP signs in by OAuth with a
  pairing code (`t3 auth pairing create`); a thread begun by the command runs in full access, so the link needs
  «Full access» to stop it.
- 2: passed (the panel's commands; installed for the trial folder with `--scope local`).
- 16: passed after a fix (bfdf6340: the model dropped «подешевле» from the task, and the work began on opus): haiku,
  under a cent. 17: passed (opus).
- bb, 8–14 (bb's desktop app with its CLI, the folder «Проба» added as its project): 8 and 9 passed after a fix
  (4c0b8196); 10 «Взяла в работу в папке «Проба», у bb: …» with no word of bb; 12 «Добавить в … или начать новую?»,
  «добавь туда» reached the same work of bb; 13 «как там» says the time and «Жду ответа сервиса» (bb tells nothing
  on the way, as T3), «стоп» passed after a fix (f9e9cad7: bb stopped, DuoWave said it did not); 14 was not built:
  «открой» said «Плитка ждёт на столе» — built (cd7e2614: «Открыла в bb.», and «Открыть в bb» on the work's page), and
  after a restart nothing could be done with bb's works begun before (bbcad6e8). The phrases were typed (the browser
  pane gives no microphone).
- 18: passed with a stand-in `claude` (the panel's path to Claude Code): it answers a plan's limit as Claude Code does
  on any model but haiku, and runs the real one on haiku. «Лимит Claude Code кончился до 21:15. Подождать,
  остановить, перейти на «Экономно» или передать bb?»; «Перейти на «Экономно»» went on at once on
  `claude-haiku-5-5` and fixed the typo; «Передать bb» stopped the work, and bb did it in its project «Проба», its
  «готово» back as a work of bb. Found by it: bb «Готова» with its server down, and no work folder matched a project
  of bb (4c0b8196); the question's buttons only on the work's page (355c8330).
- 20: passed: a second run on opus, «ревью без замечаний».
- 21: passed: the plan run changed nothing, «План для «…»: … Делаю?». 22: passed by the panel's answer — the plan,
  then the review, «готово». A typed «да» did not answer: the question is never asked aloud on a gateway with no
  voice, so «да» went to the model, which began a twin work (stopped).
- 23–26: walked by the agent before (above); 24's line could not be heard on a gateway with no voice.
- 27–29: after the release, by the owner's word: the speaker is not touched now; it gets the program by its usual
  update, and its RAM is measured then.

Found on 27.11 and what became of them:

- «Какую?» without the model: «первую», «вторую», «последнюю» are read by the pack when said to the question (the
  listening window); the agent had typed them, which goes past the question. Words of a title («про стихи») still
  need the model.
- A typed «да» to a work's question never said aloud goes to the model: a phrase answers a question only in the
  listening window after it is said; the panel's buttons answer it otherwise. Not a fault.
- Words given to a work lost their dots («numbers txt»): fixed, a573ca29 (the words as said).
- A title cut at a dot inside a word («poems2»): fixed, a573ca29. «Потом» left in a planned task: fixed, a573ca29.
- DuoWave's own runs listed as Claude Code sessions begun outside («copy-bb», «proba-b7»), going after a restart,
  taking words, crowding «Какую?»: fixed, 79c47dbf.
- Claude Code's own turns (`<task-notification>`) became works: fixed, c622048a.
- T3 Code's «не готова» words with its Electron log line: fixed, 7057169f.
- An MCP server is marked «через интернет» while its answers may bring text from outside (a new server's safe
  default, `outside`), even on 127.0.0.1: the word misleads; changing the default is the owner's call. Left.
- T3 Code's threads begun by the command run in full access, so a «Supervised» link cannot stop them: begin them
  through its MCP at the link's level. Left for later.
- A permission question names the whole path of the file, and the strip cuts it («Write: /private/…/po…»): the
  file's name alone would read better. Left.
- A word ending in a number («haiku2») was read as a work's label: fixed, 46fde231.
- bb's own `status` answers with its server down, so bb showed «Готова»; bb lists its projects with the folder in
  `sources[0].path`, so «Работы в его проектах» and «передать bb» found no project: fixed, 4c0b8196.
- A limit's and a budget's question had its buttons only on the work's page, not on the strip: fixed, 355c8330.
- bb said «не остановил» a work it stopped (its plain words were taken for a failure): fixed, f9e9cad7.
- «Открой» on a system's work was never wired; after a restart a command system's works begun before took nothing:
  built and fixed, cd7e2614 and bbcad6e8.
- «В папке Проба напиши в файл poem.md поэму» went to the workshop as a widget; with «добавь» the same went to the
  folder. The model's choice, left.
- In the Mac app, «Войти» of an MCP server with OAuth showed only «нужно войти» (the owner, on the app's own
  version): by the code the sign-in opens within the app's window; checked with the next app. Left.

Before the check:

- the gateway is built from the branch `work-systems` and started apart from the DuoWave app: its own state folder
  and another port, so the app on port 7780 keeps running as it is (the commands are given when we begin);
- you are signed in to the panel of that gateway as the admin;
- Claude Code is installed, and you are signed in to it in a terminal;
- for steps 8–14: bb is installed, or T3 Code is installed and its server runs (`t3` answers `t3 thread list`);
- for step 19: T3 Code's MCP server is added in «Мозг» → «MCP» under the name «T3 Code», and it is «на связи».

Every run of an agent below spends money or the subscription's limit: a few cents a run.

## 1. Our works as before

1. «Работы» → «Новая работа»: a trial folder with git, «поправь опечатку в README», «Запустить».

It should be: the work runs and ends as before stage 1. `GET /api/v1/work-systems` lists `duowave`, `claude-code`,
`bb` and `t3code`; a system whose program is not on the computer is «не готов» with the words why.

## 2–7. Claude Code run in a terminal

2. «Работы» → «Рабочие папки» → «Claude Code в терминале» → «Показать команды». Run the two commands to install the
   plugin in a terminal.
3. In a terminal, in any project, start `claude` and give it a small task.

It should be: «Работы» shows the session with the mark «начата не здесь». «Как там …» answers with its state.

4. Give it a task where it asks for permission (a command not allowed in its settings).

It should be: the panel says the question aloud: «Claude Code спрашивает: …». «Да» lets the command run, «нет»
refuses it, and Claude Code goes on with the answer. With no answer in two minutes it asks in its terminal as usual.

5. While it works, say «добавь ещё …» to that work.

It should be: the words reach Claude Code after its turn, as the next message.

6. Let the turn end. Then say «следи за этой», give it another task, and let that one end.

It should be: the first end is quiet: nothing is said. After «следи за этой» the end is said aloud, with its last
message.

7. Run the two commands to remove the plugin.

It should be: Claude Code works as before the plugin; DuoWave only lists its sessions.

## 8–14. bb and T3 Code

8. «Работы» → «Рабочие папки» → «Системы работ».

It should be: bb (and T3 Code) are there, «Готова», with what each can. A system that is not installed is «Не
готова» with the words why.

9. «Что поручать bb?» → «Работы в его проектах».

It should be: the folders at the paths of bb's projects show «Кто делает: bb».

10. Say «в папке „Сайт“ сделай тёмную тему» (no word about bb).

It should be: «Взяла в работу в папке «Сайт», у bb: …». The work shows in «Работы» and in bb itself.

11. Say «а это дай T3 Code: проверь ссылки» for the same folder.

It should be: that one work goes to T3 Code; the next plain task goes to bb again.

12. While the bb work goes on, say «в папке „Сайт“ добавь футер».

It should be: «Добавить в «Сделай тёмную тему» или начать новую?». «Добавь туда» gives the words to that work;
«новую» starts another one in bb.

13. «Как там тёмная тема?», then «стоп».

It should be: the state as bb says it; «стоп» stops it in bb. For T3 Code «стоп» works only after step 19; before it
DuoWave says that T3 Code cannot stop from here.

14. Let a bb work end, then «открой».

It should be: «готово» comes with bb's result. «Открой» shows the command to open the work in bb.

## 15–18. Sets of models and a limit

15. «Рабочие папки» → «Наборы моделей»: see what «Экономно», «Обычно» and «Качество» give Claude Code. Leave the
   home's set «без набора».
16. Say «в папке „Проба“ сделай подешевле: поправь опечатку».

It should be: the work starts with the task «поправь опечатку»; its journal shows the model `haiku`.

17. «Новая работа» with «Набор моделей» → «Качество».

It should be: the journal shows the model `opus`.

18. When a subscription limit holds a work (it cannot be made on purpose; check it when it happens):

It should be: «Лимит Claude Code кончился. Подождать, остановить, перейти на «Экономно» или передать bb?» — only what
is possible now: no «перейти» when the work is on «Экономно» already, no «передать» without a ready system.
«Перейди на экономно» goes on at once on the cheaper model; «передай bb» starts the work in bb with what was done,
and this one stops.

## 19. T3 Code's stop through its MCP

19. With T3 Code's MCP server on, start a work in T3 Code (step 11) and say «стоп».

It should be: the work stops in T3 Code. The tool's arguments were not checked live: if T3 Code refuses, its words
are said, and the card is fixed after this check.

## 20–22. «С проверкой» and «с планом»

20. «Новая работа» → «Как делать» → «С проверкой»: «поправь опечатку в README».

It should be: after the work, the journal shows a second run, «review», on `opus`, that changed nothing. «Готово»
says «ревью без замечаний», or how many remarks were fixed after it.

21. Say «в папке „Проба“ сначала план, поправь опечатку».

It should be: the first run is «plan», nothing changed in the folder; then «План для «Поправь опечатку»: … Делаю?».

22. Say «да».

It should be: the work goes by the plan, then the review, then «Готово». «Нет» instead stops it.

## 23. One's own words

23. Say «запомни: глянь значит как там». Start a work, then say «глянь …» with the work's name.

It should be: «Запомнила: «глянь» значит «как там».», then the work's state, as for «как там».

## 24–26. `/mcp/home` for an agent

Before: «Доступ» → a key named after the agent with the rights «Агенту: …» (all four). Add DuoWave to the agent as an
MCP server at `http://127.0.0.1:<port>/mcp/home` with that key (the commands are given when we begin).

24. Ask the agent to say a line through DuoWave.

It should be: the line is said aloud. With «Не беспокоить» on, it is not, and the agent hears «not said».

25. Ask the agent to ask you «yes or no», and answer «да».

It should be: «<агент> спрашивает: …» aloud, and the agent hears «yes». With no answer in its time it hears «silence».

26. Ask the agent to show a task of its own and to end it.

It should be: «Работы» shows the task «начата не здесь», with its steps; it ends quietly.

## 30. Junie (JetBrains) by its card (when the catalog's card is published)

Before: Junie is installed (`junie` answers in a terminal) and can work: a JetBrains AI subscription, or a model
profile of your own (`~/.junie/models/*.json`, chosen with `/model` inside Junie). Without either, every task fails
with `NO_LICENSE`. The card `p-b0b62ae09a9c` is in the catalog: it is taken only from the signed catalog, so the
catalog's branch `junie-card` is signed and published first (your step).

30. Install the card in the store, then say in «Пробе»: «Junie, напиши в файл junie.md три строки про осень».

It should be: the work starts with Junie; its question of rights, if any, comes with «Да» and «Нет» only, and the
folder's rules choose; the file lands; the end shows the turn's tokens, with no price; «стоп» during a turn stops at
once. «Добавь ещё» during a turn is not possible (Junie refuses a second prompt): the words wait for the turn's end.
Junie runs «safe» commands by itself (`brave-auto`), so those do not reach the folder's rules.

## 27–29. The speaker as the hub (only after your «да» to reach the speaker)

Before: the computer gives the speaker «Работы» on «Этот компьютер» (a speaker paired before needs it turned on).
Nothing on the speaker is changed; the speaker's program is measured first.

27. Say to the speaker «в папке „Сайт“ сделай тёмную тему».

It should be: the work starts on the computer, in the system its setup gives the folder (bb, T3 Code or DuoWave).

28. Ask the speaker «как там тёмная тема».

It should be: the state of the work on the computer.

29. Let the work end.

It should be: the speaker says «готово».

