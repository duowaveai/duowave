# get.duowave.app

Адрес установки одной командой на самой колонке:

```sh
wget -qO- http://get.duowave.app/station.sh | sh
```

В busybox колонки нет TLS, а сайт каталога `packages.duowave.app` (GitHub Pages) отвечает только по HTTPS. Поэтому
перед ним стоит Cloudflare Worker [get-worker.mjs](get-worker.mjs). Он отдаёт по простому HTTP только `/station.sh` и
`/station/<версия>/<файл>`, берёт их с каталога по HTTPS и передаёт с точной длиной. На всё остальное отвечает 404.
Сайт каталога выкладывает эти файлы только после проверки подписи сборки (`duowave-packages`, `tools/station.sh`), а
колонка сверяет сборку с контрольной суммой из скрипта.

Сам скрипт идёт без шифрования. Адрес за Cloudflare: если с колонки в России он не откроется, это видно сразу, по
первой строке `wget`.

Проверка без Cloudflare: `node scripts/release/cloudflare/get-worker-check.mjs` (её же гоняет CI).

## Поставить (один раз)

Названия кнопок взяты из справки Cloudflare на 22.11.2026. Где справка их не описывает, это сказано.

1. **Worker.** dash.cloudflare.com → **Workers & Pages** → **Create application**. Справка дальше описывает шаблоны и
   Git. Простой путь: шаблон «Hello World», имя `duowave-get`, **Deploy**, потом **Edit code**, заменить весь код
   содержимым `get-worker.mjs` и снова **Deploy**. Названий «Hello World» и «Edit code» в справке нет: если кнопки
   называются иначе, нужен любой пустой Worker, в который вставляется этот код.
2. **Адрес.** **Workers & Pages** → **Overview** → выбрать `duowave-get` → **Settings** → **Domains & Routes** →
   **Add** → **Custom Domain** → `get.duowave.app` → **Add Custom Domain**. Записи CNAME для `get.duowave.app` быть не
   должно: на имя с такой записью Custom Domain не ставится.
3. **Простой HTTP для `get`.** **SSL/TLS** → **Edge Certificates** → **Always Use HTTPS**.
   - Выключено: ничего делать не надо.
   - Включено: Cloudflare отправит `http://get.duowave.app` на `https://`, и `wget` колонки не скачает. Исключить один
     адрес правилом настройки (Configuration Rule) нельзя: в списке их настроек «Always Use HTTPS» нет. Справка
     советует выключить его и переводить на HTTPS переадресацией (Single Redirect) там, где нужно. Порядок, чтобы не
     было минуты без переадресации:
     1. **Rules** → **Overview** → **Create rule** → **Redirect Rule**. Имя: `HTTPS, кроме get`. Условие: **Custom
        filter expression** → **Edit expression**:
        `(not ssl and http.host ne "get.duowave.app")`.
        Куда: **Dynamic**, выражение `concat("https://", http.host, http.request.uri.path)`, код **301**, **Preserve
        query string** включить. **Deploy**. Справка не описывает эти клики пошагово, а выражение написано на языке
        правил Cloudflare (`ssl`, `http.host`, `concat`): редактор правила сам скажет, если в нём ошибка.
     2. Только потом **SSL/TLS** → **Edge Certificates** → **Always Use HTTPS** выключить.
4. **Проверка** (с любого компьютера):

   ```sh
   curl -sI http://get.duowave.app/
   ```

   отвечает `404`, а не `301`. После выпуска и сборки сайта каталога (Actions → pages → Run workflow):

   ```sh
   curl -s http://get.duowave.app/station.sh | grep DUOWAVE_VERSION
   ```

   показывает новую версию.
