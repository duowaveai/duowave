//! The family room through LiveKit: a token (given at home, kept on the phone), the microphone, a call to the
//! station or from it. With an access (kept, or a call's link `#access=`) the page joins by itself, and «Ответить»
//! takes a call. The client library comes from the CDN, pinned.

const LIVEKIT_PAGE_TEMPLATE: &str = r##"<!doctype html>
<html lang="ru">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Звонок через LiveKit</title>
<style>
  :root { color-scheme: light dark; }
  body { font: 16px/1.4 system-ui, sans-serif; margin: 0 auto; padding: 16px; max-width: 520px; }
  h1 { font-size: 20px; }
  button { font: inherit; padding: 8px 14px; margin: 4px 4px 4px 0; cursor: pointer; }
  input, textarea { font: inherit; padding: 8px; width: 100%; box-sizing: border-box; }
  #status { margin: 12px 0; min-height: 1.4em; }
  .notice { padding: 8px 12px; border: 1px solid #c908; border-radius: 6px; }
</style>
<script src="https://cdn.jsdelivr.net/npm/livekit-client@2.23.0/dist/livekit-client.umd.min.js"></script>
</head>
<body>
<h1>Звонок через LiveKit</h1>
__NOTICE__<p><label>Как вас назвать<br><input id="name" placeholder="Например: Мама"></label></p>
<button id="token">Получить доступ (дома)</button>
<p><label>Или вставьте доступ<br><textarea id="saved" rows="3" placeholder='{"url": "...", "token": "..."}'></textarea></label></p>
<p id="me"></p>
<button id="join">Подключиться</button>
<div id="status"></div>
<button id="sound" hidden>Включить звук</button>
<button id="call" hidden>Позвонить на колонку</button>
<div id="incoming" hidden><p id="who"></p><button id="accept">Ответить</button><button id="decline">Сбросить</button></div>
<button id="hangup" hidden>Положить трубку</button>
<script>
const $ = (id) => document.getElementById(id);
const status = (text) => ($('status').textContent = text);
const LK = window.LivekitClient;
let room, callId;
$('name').value = localStorage.getItem('lkName') || '';
$('saved').value = localStorage.getItem('lkAccess') || '';
const LIVEKIT_SERVER = __LIVEKIT_SERVER__;
const fromBase64 = (text) => new TextDecoder().decode(Uint8Array.from(atob(text.replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0)));

// A call from the station: the link's access is for this tab, its reloads too, and kept in the browser only when it
// has none of its own (live 03.11 a reload of a link on a phone with its own access joined as the phone's owner).
// The fragment leaves the address bar, so the access is not left in history or bookmarks.
let linked = '';
try { linked = fromBase64(new URLSearchParams(location.hash.slice(1)).get('access') || ''); } catch {}
if (linked) {
  history.replaceState(null, '', location.pathname + location.search);
  sessionStorage.setItem('lkLinked', linked);
  if (!$('saved').value) localStorage.setItem('lkAccess', linked);
} else linked = sessionStorage.getItem('lkLinked') || '';
if (linked) $('saved').value = linked;
// Another access chosen here: the link's is done with.
const unlink = () => { linked = ''; sessionStorage.removeItem('lkLinked'); };

// The name is in the access itself: the station announces it, whatever the field above says.
const showMe = () => {
  let name = '';
  try {
    name = JSON.parse(fromBase64(JSON.parse($('saved').value).token.split('.')[1])).name || '';
  } catch {}
  $('me').textContent = name ? 'Вы: ' + name + '. Так вас объявит колонка.' : '';
};
$('saved').oninput = showMe;
showMe();

$('token').onclick = async () => {
  const name = $('name').value.trim();
  if (!name) return status('Введите имя.');
  localStorage.setItem('lkName', name);
  const response = await fetch('/calls/livekit/token?name=' + encodeURIComponent(name));
  const body = await response.json();
  if (!response.ok) return status('Ошибка: ' + body.error);
  $('saved').value = JSON.stringify(body);
  localStorage.setItem('lkAccess', $('saved').value);
  unlink();
  showMe();
  status('Доступ получен и сохранён в этом браузере.');
};

const send = (message) =>
  room.localParticipant.publishData(new TextEncoder().encode(JSON.stringify(message)), { reliable: true, topic: 'duowave' });

$('join').onclick = async () => {
  let access;
  try { access = JSON.parse($('saved').value); } catch { return status('Нет доступа.'); }
  if ($('saved').value !== linked) {
    localStorage.setItem('lkAccess', $('saved').value);
    unlink();
  }
  // «Подключиться» again: the old connection goes first, or LiveKit drops one of the two and its sound with it.
  if (room) { await room.disconnect(); document.querySelectorAll('audio').forEach((element) => element.remove()); }
  room = new LK.Room();
  room.on(LK.RoomEvent.TrackSubscribed, (track) => { if (track.kind === 'audio') document.body.appendChild(track.attach()); });
  room.on(LK.RoomEvent.AudioPlaybackStatusChanged, () => ($('sound').hidden = room.canPlaybackAudio));
  room.on(LK.RoomEvent.DataReceived, (payload, participant, kind, topic) => {
    if (topic !== 'duowave') return;
    const event = JSON.parse(new TextDecoder().decode(payload));
    if (event.type === 'incoming') { callId = event.callId; $('who').textContent = 'Звонит ' + event.peer; $('incoming').hidden = false; }
    if (event.type === 'ringing') { callId = event.callId; status('Звоним…'); $('hangup').hidden = false; }
    if (event.type === 'connected') { status('Разговор: ' + event.peer + '.'); $('incoming').hidden = true; $('hangup').hidden = false; }
    if (event.type === 'ended') { status('Звонок завершён' + (event.reason ? ': ' + event.reason : '') + '.'); $('incoming').hidden = true; $('hangup').hidden = true; room.localParticipant.setMicrophoneEnabled(false); }
    if (event.type === 'error') status('Ошибка: ' + event.message);
  });
  // The server is the gateway's own (LIVEKIT_PUBLIC_URL), never the one a link names: a crafted link must not put
  // the family into a stranger's room with their microphone on.
  await room.connect(LIVEKIT_SERVER || access.url, access.token);
  status('В комнате.');
  $('call').hidden = false;
};

// A phone's browser plays the station's voice only after a tap: the call's own tap allows it.
const talk = async () => { await room.startAudio(); await room.localParticipant.setMicrophoneEnabled(true); };
$('call').onclick = async () => { await talk(); send({ type: 'call' }); };
$('accept').onclick = async () => { await talk(); send({ type: 'accept', callId }); };
$('sound').onclick = () => room.startAudio();
$('decline').onclick = () => send({ type: 'hangup' });
$('hangup').onclick = () => send({ type: 'hangup' });
// With an access the page joins by itself: a phone with the page open can be called, after a reload too (live
// 02.11 a reload while the station's call rang left the page out of the room).
if ($('saved').value) $('join').onclick().catch((error) => status('Не подключились: ' + error.message));
</script>
</body>
</html>
"##;

/// Without the bridge in this build the station is not in the room: the page says so above everything else.
const NO_BRIDGE: &str =
    "<p class=\"notice\">Мост LiveKit не входит в эту сборку шлюза: колонки в комнате нет, звонок на \
                         неё не пройдёт.</p>\n";

/// The page with the gateway's own LiveKit server written in (JSON-escaped, `<` too, so it cannot end the script).
pub fn livekit_page(server_url: Option<&str>) -> String {
    let literal =
        serde_json::to_string(server_url.unwrap_or("")).unwrap_or_else(|_| "\"\"".into()).replace('<', "\\u003c");
    let notice = if cfg!(feature = "livekit") { "" } else { NO_BRIDGE };
    LIVEKIT_PAGE_TEMPLATE.replacen("__NOTICE__", notice, 1).replacen("__LIVEKIT_SERVER__", &literal, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_server_in_so_that_it_cannot_end_the_script() {
        let page = livekit_page(Some("wss://lk.example/</script><script>alert(1)//"));
        assert!(page.contains(r#"const LIVEKIT_SERVER = "wss://lk.example/\u003c/script>\u003cscript>alert(1)//";"#));
        assert_eq!(page.matches("</script>").count(), 2, "the CDN's and the page's own");
        assert!(livekit_page(None).contains(r#"const LIVEKIT_SERVER = "";"#));
        assert_eq!(livekit_page(None).contains("Мост LiveKit не входит"), !cfg!(feature = "livekit"));
    }
}
