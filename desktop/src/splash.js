// The splash asks the app how the gateway is doing and goes to the panel once it answers.
// No framework, no bundler: the IPC is Tauri's own, local pages only (the panel's origin gets none).
const invoke = (command) => window.__TAURI_INTERNALS__.invoke(command);
const byId = (id) => document.getElementById(id);
// Russian for a Russian system, English for everything else.
const ru = (navigator.language || '').toLowerCase().startsWith('ru');
const t = (russian, english) => (ru ? russian : english);
let timer;

async function tick() {
    clearTimeout(timer);
    try {
        const status = await invoke('gateway_status');
        if (status.panelUrl) {
            location.replace(status.panelUrl);
            return;
        }
        render(status);
    } catch (error) {
        render({ phase: 'failed', title: t('Нет связи с приложением', 'No connection to the app'), advice: String(error) });
    }
    timer = setTimeout(tick, 500);
}

function render(status) {
    document.body.dataset.phase = status.phase;
    byId('title').textContent = status.title;
    const advice = byId('advice');
    advice.replaceChildren();
    for (const part of (status.advice ?? '').split('\n\n').filter(Boolean)) {
        // A command to copy («sudo setcap …») stands apart from the words.
        const element = document.createElement(/^sudo /.test(part) ? 'code' : 'p');
        element.textContent = part;
        advice.append(element);
    }
    byId('actions').hidden = !(status.phase === 'failed' || status.phase === 'stopped');
    // The port is taken: the first way out is another port, so it comes first and «Запустить снова» goes quiet.
    byId('port').hidden = !status.portBusy;
    byId('retry').classList.toggle('quiet', Boolean(status.portBusy));
}

byId('retry').addEventListener('click', async () => {
    render({ phase: 'starting', title: t('Шлюз запускается…', 'Gateway is starting…') });
    await invoke('gateway_restart');
    tick();
});
byId('port').addEventListener('click', async () => {
    render({ phase: 'starting', title: t('Шлюз переходит на свободный порт…', 'Gateway is moving to a free port…') });
    await invoke('gateway_free_port');
    tick();
});
byId('log').addEventListener('click', () => invoke('open_log'));
byId('data').addEventListener('click', () => invoke('open_data_dir'));

// The page's own words.
document.documentElement.lang = ru ? 'ru' : 'en';
byId('port').textContent = t('Взять свободный порт', 'Use a free port');
byId('retry').textContent = t('Запустить снова', 'Start again');
byId('log').textContent = t('Журнал', 'Log');
byId('data').textContent = t('Папка с данными', 'Data folder');
byId('title').textContent = t('Шлюз запускается…', 'Gateway is starting…');

tick();
