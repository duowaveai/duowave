//! Tells when the station has been away too long (STATION_OFFLINE_ALERT_MIN): in the log, as a macOS notification,
//! to a webhook and to the owner's Telegram, once per absence, and again when it is back. The station reaches only
//! this Mac: an absence means no voice at all until it is back or returned to Yandex (scripts/station-voice.sh).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::config::OperationsConfig;
use crate::core::events::{EventBus, GatewayEvent};
use crate::utils::time::{iso, now_ms};

/// How many station voice sessions are live now. The uniproxy session registry implements this when it is ported;
/// until then the gateway has no sessions to count.
pub trait ActiveSessions: Send + Sync {
    fn active_count(&self) -> usize;
}

/// Sends a message to the owner's Telegram: the calls' bot (src/links/calls.rs).
pub trait TelegramSender: Send + Sync {
    fn enabled(&self) -> bool;
    /// The chat the alerts go to, as the panel sets it up now (Telegram's «Оповещения»); none: no alerts there.
    fn chat(&self) -> Option<String>;
    fn send_text<'a>(&'a self, chat: &'a str, text: &'a str) -> BoxFuture<'a, Result<(), String>>;
}

/// For /health: whether a station is connected and when one was last seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchStatus {
    pub connected: bool,
    pub last_seen_at: String,
}

/// How long one alert may take to reach the webhook or Telegram: Node's fetch gave up on a server that never
/// answered after five minutes (undici's headers timeout). The watch's looks never wait for it.
const SEND_TIMEOUT: Duration = Duration::from_secs(300);

/// A clock before this (2020-01-01) is not set yet: the station boots in 1970 and takes the time from the network a
/// while later. An absence counted from such a moment is decades long (live, 10.11: «29905133 мин»), so the watch
/// starts counting once the clock is right.
const SANE_CLOCK_MS: u64 = 1_577_836_800_000;

/// Whether no station is to come to this gateway's /uni.ws at all: on the computer, while every station's gateway runs
/// on the station itself (src/placement/watch.rs, `none_here`).
pub type Elsewhere = Arc<dyn Fn() -> bool + Send + Sync>;

/// When a station was last there, and whether its absence is told.
struct Seen {
    last_seen: u64,
    alerted: bool,
}

pub struct StationWatch {
    registry: Arc<dyn ActiveSessions>,
    telegram: Option<Arc<dyn TelegramSender>>,
    config: OperationsConfig,
    events: EventBus,
    seen: Mutex<Seen>,
    send_timeout: Duration,
    cancel: CancellationToken,
    elsewhere: std::sync::OnceLock<Elsewhere>,
}

impl StationWatch {
    pub fn new(
        registry: Arc<dyn ActiveSessions>,
        telegram: Option<Arc<dyn TelegramSender>>,
        config: OperationsConfig,
        events: EventBus,
    ) -> StationWatch {
        StationWatch {
            registry,
            telegram,
            config,
            events,
            seen: Mutex::new(Seen { last_seen: now_ms(), alerted: false }),
            send_timeout: SEND_TIMEOUT,
            cancel: CancellationToken::new(),
            elsewhere: std::sync::OnceLock::new(),
        }
    }

    /// The computer: what says no station is to come here now (every station's gateway runs on it). The watch is
    /// silent about «не подключена» while it says so.
    pub fn set_elsewhere(&self, elsewhere: Elsewhere) {
        let _ = self.elsewhere.set(elsewhere);
    }

    fn none_expected(&self) -> bool {
        self.elsewhere.get().is_some_and(|elsewhere| elsewhere())
    }

    /// Listens for a stranger asking to connect and, when the alert is on, checks every 30 s. The TS
    /// `onApplicationBootstrap`. Neither keeps the watch alive: an engine started again leaves none behind.
    pub fn start(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        self.events.on("station-watch", move |event| {
            if let GatewayEvent::StationPending { address, .. } = &*event {
                let Some(me) = me.upgrade() else { return };
                let address = address.clone();
                tokio::spawn(async move {
                    me.alert(&crate::tr!(
                        "Новая колонка с адреса {address} просит подключиться к шлюзу. Разрешить — в пульте, раздел «Колонки».",
                        address = address
                    ))
                    .await;
                });
            }
        });
        if self.config.offline_alert_ms == 0 {
            return;
        }
        let me = Arc::downgrade(self);
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(30_000));
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ticker.tick() => {
                        let Some(watch) = me.upgrade() else { break };
                        // Each look on its own, as setInterval fired them: an alert still sending holds back none.
                        tokio::spawn(async move { watch.check(now_ms()).await });
                    }
                }
            }
        });
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// For /health: whether a station is connected and when one was last seen.
    pub fn status(&self) -> WatchStatus {
        let connected = self.registry.active_count() > 0;
        let now = now_ms();
        let mut seen = self.seen();
        if connected || (seen.last_seen < SANE_CLOCK_MS && now >= SANE_CLOCK_MS) {
            seen.last_seen = now;
        }
        WatchStatus { connected, last_seen_at: iso_at(seen.last_seen) }
    }

    pub async fn check(&self, now: u64) {
        if let Some(text) = self.news(now) {
            self.alert(&text).await;
        }
    }

    /// What a look has to tell, decided under one lock: looks may overlap now, and one absence is told once, as the
    /// TS check decided it on its one thread before its first await.
    fn news(&self, now: u64) -> Option<String> {
        let connected = self.registry.active_count() > 0;
        let elsewhere = !connected && self.none_expected();
        let mut seen = self.seen();
        if connected {
            seen.last_seen = now;
            return std::mem::take(&mut seen.alerted).then(|| crate::tr!("Колонка снова на связи со шлюзом."));
        }
        // The clock not set yet (a station's boot): nothing counted. Set now while the watch's last look was before:
        // the count starts now.
        if now < SANE_CLOCK_MS {
            return None;
        }
        // On the computer, every station's gateway runs on the station: none is missing here. The count starts again
        // when one is to come back.
        if elsewhere || seen.last_seen < SANE_CLOCK_MS {
            seen.last_seen = now;
            return None;
        }
        if seen.alerted || now.saturating_sub(seen.last_seen) < self.config.offline_alert_ms {
            return None;
        }
        seen.alerted = true;
        let minutes = (now.saturating_sub(seen.last_seen) as f64 / 60_000.0).round() as u64;
        Some(crate::tr!(
            "Колонка не подключена к шлюзу {minutes} мин. Голоса у неё нет; вернуть к Яндексу: scripts/station-voice.sh yandex.",
            minutes = minutes
        ))
    }

    /// Whether a word to the owner reaches them now: a chat «Оповещения» chosen in Telegram, a webhook, or the Mac's
    /// notifications — the log alone is no way (plugins/schedule.rs, `notify`).
    pub fn can_tell(&self) -> bool {
        let telegram = self.telegram.as_ref().is_some_and(|telegram| telegram.enabled() && telegram.chat().is_some());
        let mac = cfg!(target_os = "macos") && self.config.notify;
        telegram || self.config.webhook_url.is_some() || mac
    }

    /// Tells the household something by the same ways as «колонка не на связи»: the log, macOS, the webhook,
    /// Telegram (src/placement: the station's gateway, the safety net).
    pub async fn tell(&self, text: &str) {
        self.alert(text).await;
    }

    async fn alert(&self, text: &str) {
        tracing::warn!("{text}");
        #[cfg(target_os = "macos")]
        if self.config.notify {
            let script = format!("display notification {} with title \"DuoWave\"", json_string(text));
            if let Err(error) = spawn_reaped(tokio::process::Command::new("osascript").arg("-e").arg(script)) {
                tracing::debug!("notification not shown: {error}");
            }
        }
        if let Some(url) = &self.config.webhook_url {
            if let Err(error) = self.post_webhook(url, text).await {
                tracing::warn!("alert webhook: {error}");
            }
        }
        if let Some((chat, telegram)) = self.telegram.as_ref().and_then(|telegram| Some((telegram.chat()?, telegram))) {
            if telegram.enabled() {
                match tokio::time::timeout(self.send_timeout, telegram.send_text(&chat, text)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => tracing::warn!("alert to Telegram: {error}"),
                    Err(_) => tracing::warn!("alert to Telegram: no answer in {} s", self.send_timeout.as_secs()),
                }
            }
        }
    }

    /// Posts the alert to the webhook. The error comes without the URL: its path or query is the hook's secret
    /// (Slack's, Discord's), and Node's «fetch failed» named none.
    async fn post_webhook(&self, url: &str, text: &str) -> Result<(), String> {
        let body = serde_json::json!({ "text": text });
        match crate::utils::http::client().post(url).timeout(self.send_timeout).json(&body).send().await {
            Ok(_) => Ok(()),
            Err(error) => Err(format!("{:#}", anyhow::Error::new(error.without_url()))),
        }
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl Drop for StationWatch {
    /// The ticker ends with the watch.
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn iso_at(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64).map(iso).unwrap_or_else(crate::utils::time::now_iso)
}

/// A string as a JSON literal, for the AppleScript notification (the TS `JSON.stringify(text)`).
#[cfg(target_os = "macos")]
fn json_string(text: &str) -> String {
    serde_json::Value::String(text.to_string()).to_string()
}

/// Starts a program and waits for it in a task of its own, as Node's execFile did: a child nobody waits for stays a
/// zombie for the gateway's life. The task ends with how the program ended.
#[cfg(target_os = "macos")]
fn spawn_reaped(
    command: &mut tokio::process::Command,
) -> std::io::Result<tokio::task::JoinHandle<std::io::Result<std::process::ExitStatus>>> {
    let mut child = command.spawn()?;
    Ok(tokio::spawn(async move { child.wait().await }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Env;

    struct Sessions(Mutex<usize>);
    impl ActiveSessions for Sessions {
        fn active_count(&self) -> usize {
            *self.0.lock().unwrap()
        }
    }

    struct Telegram(Arc<Mutex<Vec<String>>>);
    impl TelegramSender for Telegram {
        fn enabled(&self) -> bool {
            true
        }
        fn chat(&self) -> Option<String> {
            Some("42".into())
        }
        fn send_text<'a>(&'a self, chat: &'a str, text: &'a str) -> BoxFuture<'a, Result<(), String>> {
            assert_eq!(chat, "42");
            self.0.lock().unwrap().push(text.to_string());
            Box::pin(async { Ok(()) })
        }
    }

    fn operations() -> OperationsConfig {
        let mut config = crate::config::Config::for_tests(&Env::default()).operations;
        config.offline_alert_ms = 60_000;
        config.notify = false;
        config
    }

    #[tokio::test]
    async fn tells_once_when_the_station_is_away_too_long_and_again_when_it_is_back() {
        let sessions = Arc::new(Sessions(Mutex::new(0)));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let watch =
            StationWatch::new(sessions.clone(), Some(Arc::new(Telegram(sent.clone()))), operations(), EventBus::new());
        let start = now_ms();
        watch.check(start + 30_000).await;
        assert!(sent.lock().unwrap().is_empty());
        watch.check(start + 61_000).await;
        watch.check(start + 120_000).await;
        assert_eq!(sent.lock().unwrap().len(), 1);
        assert!(sent.lock().unwrap()[0].contains("не подключена к шлюзу 1 мин"), "{:?}", sent.lock().unwrap());
        *sessions.0.lock().unwrap() = 1;
        watch.check(start + 130_000).await;
        assert_eq!(sent.lock().unwrap()[1], "Колонка снова на связи со шлюзом.");
        assert!(watch.status().connected);
    }

    /// The log lines written on this thread while the guard lives.
    fn capture_logs() -> (Arc<Mutex<Vec<u8>>>, tracing::subscriber::DefaultGuard) {
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let logs = Arc::new(Mutex::new(Vec::new()));
        let into = logs.clone();
        let subscriber = tracing_subscriber::fmt().with_ansi(false).with_writer(move || Sink(into.clone())).finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    #[tokio::test]
    async fn logs_a_failed_webhook_without_its_secret_url() {
        let (logs, _guard) = capture_logs();
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let mut config = operations();
        config.webhook_url = Some(format!("http://127.0.0.1:{port}/services/T000/B000/XXXXSECRET?token=hook-token"));
        let watch = StationWatch::new(Arc::new(Sessions(Mutex::new(0))), None, config, EventBus::new());
        watch.check(now_ms() + 61_000).await;
        let logged = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        assert!(logged.contains("alert webhook: "), "{logged}");
        assert!(!logged.contains("XXXXSECRET") && !logged.contains("hook-token"), "{logged}");
    }

    #[tokio::test]
    async fn a_webhook_that_never_answers_holds_up_neither_telegram_nor_the_next_look() {
        // It takes the connection and never says a word.
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = silent.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = silent.accept().await {
                held.push(socket);
            }
        });
        let sessions = Arc::new(Sessions(Mutex::new(0)));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let mut config = operations();
        config.webhook_url = Some(format!("http://127.0.0.1:{port}/hook"));
        let mut watch =
            StationWatch::new(sessions.clone(), Some(Arc::new(Telegram(sent.clone()))), config, EventBus::new());
        watch.send_timeout = Duration::from_millis(200);
        let start = now_ms();
        let away = tokio::time::timeout(Duration::from_secs(5), watch.check(start + 61_000)).await;
        assert!(away.is_ok(), "the look gave the webhook up");
        assert_eq!(sent.lock().unwrap().len(), 1, "Telegram heard it all the same");
        *sessions.0.lock().unwrap() = 1;
        let back = tokio::time::timeout(Duration::from_secs(5), watch.check(start + 70_000)).await;
        assert!(back.is_ok());
        assert_eq!(sent.lock().unwrap()[1], "Колонка снова на связи со шлюзом.");
        server.abort();
    }

    /// Live, 10.11: the station's own gateway started while the station's clock still said 1970, and once the clock was
    /// set it told «Колонка не подключена к шлюзу 29905133 мин». The count starts once the clock is right.
    #[tokio::test]
    async fn counts_an_absence_only_from_a_clock_that_is_set() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let watch = StationWatch::new(
            Arc::new(Sessions(Mutex::new(0))),
            Some(Arc::new(Telegram(sent.clone()))),
            operations(),
            EventBus::new(),
        );
        // Started at the station's boot: 1970 and a few seconds.
        watch.seen().last_seen = 5_000;
        watch.check(65_000).await;
        assert!(sent.lock().unwrap().is_empty(), "a clock not set yet counts nothing");
        assert!(watch.status().last_seen_at.starts_with("20"), "/health does not say 1970 once the clock is set");
        let start = now_ms();
        watch.seen().last_seen = 5_000;
        watch.check(start).await;
        assert!(sent.lock().unwrap().is_empty(), "the clock is set now: the count starts");
        watch.check(start + 61_000).await;
        assert_eq!(sent.lock().unwrap().len(), 1);
        assert!(sent.lock().unwrap()[0].contains("не подключена к шлюзу 1 мин"), "{:?}", sent.lock().unwrap());
    }

    /// Live, 10.11: the computer told «Колонка не подключена к шлюзу 5 мин… вернуть к Яндексу» while the station talked
    /// to its own gateway. While no station is to come here, nothing is missing; the count starts again when one is.
    #[tokio::test]
    async fn the_computer_is_silent_while_every_station_runs_its_own_gateway() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let watch = StationWatch::new(
            Arc::new(Sessions(Mutex::new(0))),
            Some(Arc::new(Telegram(sent.clone()))),
            operations(),
            EventBus::new(),
        );
        let elsewhere = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let says = elsewhere.clone();
        watch.set_elsewhere(Arc::new(move || says.load(std::sync::atomic::Ordering::Relaxed)));
        let start = now_ms();
        watch.check(start + 61_000).await;
        watch.check(start + 600_000).await;
        assert!(sent.lock().unwrap().is_empty(), "the station's gateway is on the station");
        // Back to the computer: its absence counts from now.
        elsewhere.store(false, std::sync::atomic::Ordering::Relaxed);
        watch.check(start + 630_000).await;
        assert!(sent.lock().unwrap().is_empty());
        watch.check(start + 661_000).await;
        assert_eq!(sent.lock().unwrap().len(), 1, "{:?}", sent.lock().unwrap());
    }

    #[tokio::test]
    async fn its_listener_and_its_ticker_do_not_keep_it_alive() {
        let watch = Arc::new(StationWatch::new(Arc::new(Sessions(Mutex::new(0))), None, operations(), EventBus::new()));
        watch.start();
        let left = Arc::downgrade(&watch);
        drop(watch);
        assert!(left.upgrade().is_none(), "a restarted engine leaves the old watch behind");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn waits_for_the_program_it_starts_so_none_is_left_a_zombie() {
        let ended = spawn_reaped(&mut tokio::process::Command::new("true")).unwrap().await.unwrap().unwrap();
        assert!(ended.success());
    }
}
