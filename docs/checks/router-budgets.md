# Check by hand: the router's door, the budgets of works, a work in a folder by voice

How to walk router stages 2 and 5 of «Роутер и работы» by hand: the seats, the door
and the budgets (steps 1–6), a work in a folder by voice (steps 7–14). Step 0 is the first work with a real Claude
Code: stage 3 has never been run with one. About forty minutes, on the computer, in the DuoWave app. The speaker is
not needed and is not touched.

Before the check:

- the app is built from the branch `router-s2-crates` (router stages 2 and 5 on the crates) and restarted once; you
  do this yourself;
- Claude Code is installed, and you are signed in to it in a terminal;
- the regulator «Что может уходить из дома» is on «Любые облака»;
- you are signed in to the panel as the admin.

Every run of Claude Code below spends money or the subscription's limit: a few cents a run.

## 0. The first work with a real Claude Code

1. «Работы» → «Рабочие папки» → «Агенты»: Claude Code is «найден», shows its version, «вошёл», and the sign-in is
   «Как в терминале».
2. Add a trial folder with git (a copy of a small project with a `README.md`). Below it is called «Проба». A folder
   without git is not added, and the panel says why.
3. «Новая работа»: «Проба», «поправь опечатку в README», «Запустить».

It should be: the tab «Журнал» shows live what the agent reads, edits and runs. At the end the folder has a branch
`duowave/…`, and the branch you had chosen stays chosen. «Готово» shows the checks and the dollars, and «Расход» has
the deal of this work.

4. «Добавить вдогонку»: a new run goes on in the same session. «Где» shows a command; run it in a terminal and see the
   same session.
5. Ask for a work where the agent needs to push its branch. The journal shows `git push` refused, and the branch stays
   on the computer.

## 1. The conversation as before

1. Say «который час» to the panel, then «почему небо синее».
2. «Персонаж» → «Мышление» → «Расход» → today's deals.

It should be: two deals on the same models as before: «Быстрая» with the reason «правило: короткая фраза» and «Умная»
with «правило: трудная фраза». The answers come as fast as before the update.

## 2. The folder's old ceiling became its budget

1. «Рабочие папки» → the trial folder → the field «Бюджет одной работы, $».

It should be: if a «потолок запуска» was set there before, the number is in place, and the label says «Бюджет одной
работы».

## 3. A cheaper model at 80%

1. «Работы» → «Новая работа» → the trial folder, the task «добавь в README строку „проверка бюджета“», «Запустить».
2. Wait for «готово». Open the work → «Сделки» and note what the run cost, for example $0.30.
3. «Рабочие папки» → the trial folder → «Бюджет одной работы, $»: write a tenth more than that, for example `0.33`,
   and save. More than 80% of the budget is now spent.
4. Back in the work → «Добавить вдогонку»: «и ещё строку „вторая“».

It should be: in «Сделки» the second run is on the model `haiku` with the reason «бюджет: модель дешевле». In the
second run's journal the agent begins with a retelling of the task: it is a new session on another model.

## 4. The question at 100%

The second run has a few cents of the budget left, so it reaches the end quickly.

It should be: the next run does not begin. In «Работы» the work is under «Спрашивает» with the line «…: бюджет $0.33
кончился, потрачено $0.3… Добавить ещё $0.33 или остановить?» and the buttons «Добавить ещё» and «Остановить». Until
you answer, nothing is spent: «Расход» has no new deals.

If the second run ended before the budget did, press «Добавить вдогонку» once more: the question comes before that run.

1. Press «Добавить ещё».

It should be: under the buttons, «Добавил $0.33, продолжаю.», and the work goes on with your usual model (the share
spent is below 80% again).

2. When the question comes again (or in a new work the same way), answer by voice: «Как там …?». The panel says the
   work's step and the budget's question. Say «стоп». (The button «Остановить» does the same.)

It should be: the work is stopped, and the branch with what was done is in the folder. «Добавь» said to the same
question works as the button «Добавить ещё».

## 5. The regulator holds the agent

1. Set the regulator «Что может уходить из дома» to «Дом и моё облако».
2. «Новая работа» in the trial folder.

It should be: the work ends at once with «не получилось — регулятор «Что может уходить из дома» не выпускает Claude
Code в облако». Set the regulator back to «Любые облака».

## 6. Give the folder back

Clear the field «Бюджет одной работы, $» of the trial folder: the general budget holds again, 5 dollars a work.

## 7. A work in a folder by voice

1. Press «Сказать»: «В папке Проба поправь опечатку в README».

It should be: «Взяла в работу в папке «Проба»: «Поправь опечатку в README». Скажу, когда будет готово.» «Работы» →
«Сейчас» shows the work in the folder.

## 8. «Как там …»

While it goes on: «Как там опечатка?»

It should be: «В работе …: правит README.md.» (or «читает …», «запускает …»).

## 9. «Готово» and «подробнее»

Wait for the end.

It should be: «Поправь опечатку в README: готово. Ветка duowave/…, проверки прошли, ≈ $… по подписке. Рассказать,
что написал агент?» Say «да» (or «подробнее»): the panel reads the agent's last message, without markup or code.

## 10. No folder named, or no such folder

1. «Поправь ещё одну опечатку в README», without a folder.

It should be: with one folder it takes that one. With two it asks «В какой папке: «…» или «…»?», and you answer with
the name.

2. «В папке Блог добавь страницу».

It should be: «Папки «Блог» нет. Есть: «Проба».» No new work begins.

## 11. The workshop does not wait

While the work in the folder goes on: «Сделай виджет с часами».

It should be: «Делаю: …». The workshop begins at once.

## 12. A guest cannot

If there is a guest sign-in: sign in as a guest and say the phrase of step 7.

It should be: «Работу в папке может начать только администратор дома.» (or a plain answer with no work). No new work
begins.

## 13. A subscription limit

If a limit comes: «Как там …?»

It should be: «… ждёт лимита до 18:00. Подождать или остановить?». «Подожди» gives «Жду. Продолжу сам, когда лимит
сбросится.». «Стоп» stops the work. «Добавь» is not an answer to this question.

## 14. Background jobs as before

Start the tool «В фоне» or an app's long task as usual: everything works as before.

## If something is wrong

- Write the step's number and what you heard or saw.
- If the conversation's deals go to another model than before, the door is wrong: write the phrase and the model.
- If a work waits for an answer and there are no buttons, open the work and answer by voice: «добавь» or «стоп».
- A single work's budget and the rules table change only through the API for now (`PUT /api/v1/works/{id}/budget`,
  `PUT /api/v1/router/table`). Their page in the panel is stage 6в.
