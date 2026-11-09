//! Our call page: a name, who is online, call and answer, the microphone through WebRTC to the hub.

const CALL_PAGE_TEMPLATE: &str = r##"<!doctype html>
<html lang="ru">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Звонок на колонку</title>
<style>
  :root { color-scheme: light dark; --gap: 12px; }
  body { font: 16px/1.4 system-ui, sans-serif; margin: 0 auto; padding: 16px; max-width: 520px; }
  h1 { font-size: 20px; }
  button { font: inherit; padding: 8px 14px; margin: 4px 4px 4px 0; cursor: pointer; }
  input { font: inherit; padding: 8px; width: 100%; box-sizing: border-box; }
  .row { display: flex; align-items: center; justify-content: space-between; gap: var(--gap); padding: 6px 0; border-bottom: 1px solid #8884; }
  #status { margin: 12px 0; min-height: 1.4em; }
  .notice { padding: 8px 12px; border: 1px solid #c908; border-radius: 6px; }
</style>
</head>
<body>
<h1>Звонок на колонку</h1>
__NOTICE__<p><label>Как вас назвать<br><input id="name" placeholder="Например: Мама" autocomplete="nickname"></label></p>
<button id="join">Подключиться</button>
<div id="status"></div>
<div id="list"></div>
<div id="incoming" hidden><p id="who"></p><button id="accept">Ответить</button><button id="decline">Сбросить</button></div>
<button id="hangup" hidden>Положить трубку</button>
<audio id="remote" autoplay></audio>
<script>
const $ = (id) => document.getElementById(id);
let socket, me, call, pc;
const status = (text) => ($('status').textContent = text);
$('name').value = localStorage.getItem('callName') || '';

$('join').onclick = () => {
  const name = $('name').value.trim();
  if (!name) return status('Введите имя.');
  localStorage.setItem('callName', name);
  socket = new WebSocket((location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/calls/ws');
  socket.onopen = () => socket.send(JSON.stringify({ type: 'hello', role: 'browser', name }));
  socket.onclose = () => status('Отключено.');
  socket.onmessage = (message) => onEvent(JSON.parse(message.data));
};

function render(endpoints) {
  $('list').replaceChildren(...endpoints.filter((e) => e.id !== me).map((e) => {
    const row = document.createElement('div');
    row.className = 'row';
    row.textContent = e.name + (e.kind === 'station' ? ' (колонка)' : '');
    const button = document.createElement('button');
    button.textContent = 'Позвонить';
    button.onclick = () => socket.send(JSON.stringify({ type: 'call', to: e.id }));
    row.append(button);
    return row;
  }));
}

async function startMedia() {
  pc = new RTCPeerConnection();
  const stream = await navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true } });
  stream.getTracks().forEach((track) => pc.addTrack(track, stream));
  pc.ontrack = (event) => ($('remote').srcObject = event.streams[0] || new MediaStream([event.track]));
  await pc.setLocalDescription(await pc.createOffer());
  await new Promise((resolve) => {
    if (pc.iceGatheringState === 'complete') return resolve();
    pc.onicegatheringstatechange = () => pc.iceGatheringState === 'complete' && resolve();
    setTimeout(resolve, 2000);
  });
  socket.send(JSON.stringify({ type: 'offer', sdp: pc.localDescription.sdp }));
}

function stopMedia() {
  pc?.getSenders().forEach((sender) => sender.track?.stop());
  pc?.close();
  pc = undefined;
}

function onEvent(event) {
  switch (event.type) {
    case 'welcome': me = event.id; status('На связи как ' + event.name + '.'); render(event.endpoints); break;
    case 'endpoints': render(event.endpoints); break;
    case 'ringing': call = event.callId; status('Звоним: ' + event.peer.name + '…'); $('hangup').hidden = false; startMedia(); break;
    case 'incoming': call = event.callId; $('who').textContent = 'Звонит ' + event.peer.name; $('incoming').hidden = false; break;
    case 'connected': status('Разговор: ' + event.peer.name + '.'); $('incoming').hidden = true; $('hangup').hidden = false; break;
    case 'answer': pc?.setRemoteDescription({ type: 'answer', sdp: event.sdp }); break;
    case 'ended': status('Звонок завершён' + (event.reason ? ': ' + event.reason : '') + '.'); call = undefined; $('incoming').hidden = true; $('hangup').hidden = true; stopMedia(); break;
    case 'error': status('Ошибка: ' + event.message); break;
  }
}

$('accept').onclick = async () => { await startMedia(); socket.send(JSON.stringify({ type: 'accept', callId: call })); };
$('decline').onclick = () => socket.send(JSON.stringify({ type: 'hangup', callId: call }));
$('hangup').onclick = () => socket.send(JSON.stringify({ type: 'hangup', callId: call }));
</script>
</body>
</html>
"##;

/// Without WebRTC in this build a browser cannot carry a call's sound: the page says so above everything else.
const NO_WEBRTC: &str = "<p class=\"notice\">Звонок из браузера (WebRTC) не входит в эту сборку шлюза: страница \
                         покажет, кто на связи, но разговора не будет.</p>\n";

/// The page, with the notice when this build has no WebRTC.
pub fn call_page() -> String {
    let notice = if cfg!(feature = "webrtc") { "" } else { NO_WEBRTC };
    CALL_PAGE_TEMPLATE.replacen("__NOTICE__", notice, 1)
}

#[cfg(test)]
mod tests {
    #[test]
    fn is_the_call_page_with_its_peer_connection() {
        let page = super::call_page();
        assert!(page.contains("RTCPeerConnection"));
        assert!(page.contains("/calls/ws"));
        assert_eq!(page.contains("WebRTC) не входит"), !cfg!(feature = "webrtc"));
    }
}
