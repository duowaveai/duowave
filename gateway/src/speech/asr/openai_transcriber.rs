use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Result};
use futures::future::{BoxFuture, FutureExt, Shared};
use reqwest::multipart::{Form, Part};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::speech_recognizer::SpeechRecognizer;
use super::transcript::is_js_space;
use crate::config::AsrConfig;
use crate::speech::audio::pcm::encode_wav;
use crate::utils::http::client;
use crate::utils::time::now_ms;

/// The server is asked which model it serves at most this often…
const SERVED_CHECK_MS: u64 = 60_000;
/// …and sooner when it did not answer: it is down, or restarting with another model.
const SERVED_RETRY_MS: u64 = 10_000;

/// What `GET /models` said: whether it answered, and the models' ids when it gave a list.
struct ModelsAnswer {
    ok: bool,
    status: u16,
    ids: Option<Vec<String>>,
}

struct Served {
    /// The model the server says it serves; None: the one asked for.
    served: Option<String>,
    /// When the server is asked again; u64::MAX: not before it restarts (it serves what it is asked for).
    due: u64,
    /// Restarts heard of (forget): an answer asked for before one is dropped.
    restarts: u64,
    asking: Option<Shared<BoxFuture<'static, ()>>>,
}

struct Inner {
    config: AsrConfig,
    base_url: String,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
    served: Mutex<Served>,
}

/// Any OpenAI-compatible `POST /audio/transcriptions`: OpenAI itself, Groq and similar clouds, or our GigaAM server
/// (asr-server/). The utterance goes as one 16 kHz mono WAV after the end of speech.
///
/// Our server recognises with the model it loaded, whatever the request names, and lists only that one on
/// `GET /models` (live, 05.11: ASR_MODEL said gigaam-v3, the journal too, while parakeet-0.6b-v3 listened). A server
/// that lists one model is therefore taken at its word; one that lists many serves the model asked for.
pub struct OpenAiTranscriber {
    inner: Arc<Inner>,
}

/// A body as fetch's `text()` reads it: UTF-8 whatever charset the Content-Type names (JSON is UTF-8, RFC 8259), a
/// leading BOM dropped, broken bytes replaced. reqwest's `text()` would decode by the label.
fn utf8_text(body: &[u8]) -> String {
    String::from_utf8_lossy(body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body)).into_owned()
}

/// The innermost cause of an error: what went wrong, without the layers that carried it.
fn innermost(error: &(dyn std::error::Error + 'static)) -> String {
    let mut cause = error;
    while let Some(source) = cause.source() {
        cause = source;
    }
    cause.to_string()
}

impl OpenAiTranscriber {
    pub fn new(config: AsrConfig) -> OpenAiTranscriber {
        OpenAiTranscriber::with_clock(config, now_ms)
    }

    /// With a clock of its own (ms), for tests.
    pub fn with_clock(config: AsrConfig, now: impl Fn() -> u64 + Send + Sync + 'static) -> OpenAiTranscriber {
        let base_url = config.base_url.trim_end_matches('/').to_string();
        OpenAiTranscriber {
            inner: Arc::new(Inner {
                config,
                base_url,
                now: Box::new(now),
                served: Mutex::new(Served { served: None, due: 0, restarts: 0, asking: None }),
            }),
        }
    }

    pub fn model(&self) -> String {
        self.inner.lock().served.clone().unwrap_or_else(|| self.inner.config.model.clone())
    }

    pub fn description(&self) -> String {
        format!("{} via {}/audio/transcriptions", self.model(), self.inner.base_url)
    }

    pub async fn recognize(&self, pcm: &[i16], sample_rate: u32, cancel: &CancellationToken) -> Result<String> {
        let config = &self.inner.config;
        let file = Part::bytes(encode_wav(pcm, sample_rate)).file_name("speech.wav").mime_str("audio/wav")?;
        let mut form =
            Form::new().part("file", file).text("model", config.model.clone()).text("response_format", "json");
        if let Some(language) = config.language.as_ref().filter(|language| !language.is_empty()) {
            form = form.text("language", language.clone());
        }
        if let Some(prompt) = config.prompt.as_ref().filter(|prompt| !prompt.is_empty()) {
            form = form.text("prompt", prompt.clone());
        }
        let request = self
            .inner
            .authorized(client().post(format!("{}/audio/transcriptions", self.inner.base_url)))
            .multipart(form)
            .timeout(Duration::from_millis(config.timeout_ms));
        let exchange = async {
            let response = request.send().await?;
            let status = response.status();
            Ok::<_, reqwest::Error>((status, response.bytes().await?))
        };
        let (status, body) = tokio::select! {
            result = exchange => result?,
            _ = cancel.cancelled() => bail!("recognition cancelled"),
        };
        let body = utf8_text(&body);
        if !status.is_success() {
            bail!("HTTP {}: {}", status.as_u16(), body.chars().take(300).collect::<String>());
        }
        let parsed: Value = serde_json::from_str(&body)?;
        Ok(parsed
            .get("text")
            .and_then(Value::as_str)
            .map(|text| text.trim_matches(is_js_space))
            .unwrap_or("")
            .to_string())
    }

    pub async fn probe(&self) -> String {
        match self.inner.list_models(3000).await {
            Ok(answer) => {
                let (ok, status) = (answer.ok, answer.status);
                self.inner.learn(&mut self.inner.lock(), answer);
                if ok {
                    "reachable".into()
                } else {
                    format!("answers HTTP {status} on /models")
                }
            }
            Err(error) => {
                self.inner.lock().due = (self.inner.now)().saturating_add(SERVED_RETRY_MS);
                format!("unreachable ({})", innermost(&error))
            }
        }
    }

    /// Asks the server which model it serves when that is due, one asking at a time. The asking is a task of its
    /// own: dropping the returned future does not stop it (outside a tokio runtime nothing is asked).
    pub fn refresh(&self) -> BoxFuture<'static, ()> {
        let mut served = self.inner.lock();
        if served.asking.is_none() && (self.inner.now)() >= served.due {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                // An answer from before a restart says nothing about the model after it.
                let restarts = served.restarts;
                let inner = self.inner.clone();
                // The lock is held until `asking` is set: the task's own reset of it cannot come first.
                let task = runtime.spawn(async move {
                    let answer = inner.list_models(1500).await;
                    let mut served = inner.lock();
                    if restarts == served.restarts {
                        match answer {
                            Ok(answer) => inner.learn(&mut served, answer),
                            Err(_) => served.due = (inner.now)().saturating_add(SERVED_RETRY_MS),
                        }
                    }
                    served.asking = None;
                });
                served.asking = Some(
                    async move {
                        let _ = task.await;
                    }
                    .boxed()
                    .shared(),
                );
            }
        }
        match &served.asking {
            Some(asking) => asking.clone().boxed(),
            None => futures::future::ready(()).boxed(),
        }
    }

    pub fn forget(&self) {
        let mut served = self.inner.lock();
        served.restarts += 1;
        served.due = 0;
    }
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, Served> {
        self.served.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.config.api_key {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    }

    /// One model listed: the server serves it. Many, none or no list: it serves the one asked for, nothing to follow.
    fn learn(&self, served: &mut Served, answer: ModelsAnswer) {
        served.served = answer.ids.filter(|ids| ids.len() == 1).and_then(|mut ids| ids.pop());
        served.due = if served.served.is_some() { (self.now)().saturating_add(SERVED_CHECK_MS) } else { u64::MAX };
    }

    async fn list_models(&self, timeout_ms: u64) -> Result<ModelsAnswer, reqwest::Error> {
        let response = self
            .authorized(client().get(format!("{}/models", self.base_url)))
            .timeout(Duration::from_millis(timeout_ms))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Ok(ModelsAnswer { ok: false, status: status.as_u16(), ids: None });
        }
        let body: Option<Value> = response.json().await.ok();
        let ids = body.as_ref().and_then(|body| body.get("data")).and_then(Value::as_array).map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("id").and_then(Value::as_str))
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .collect()
        });
        Ok(ModelsAnswer { ok: true, status: status.as_u16(), ids })
    }
}

impl SpeechRecognizer for OpenAiTranscriber {
    fn description(&self) -> String {
        OpenAiTranscriber::description(self)
    }

    fn model(&self) -> String {
        OpenAiTranscriber::model(self)
    }

    fn recognize<'a>(
        &'a self,
        pcm: &'a [i16],
        sample_rate: u32,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<String>> {
        OpenAiTranscriber::recognize(self, pcm, sample_rate, cancel).boxed()
    }

    fn probe(&self) -> BoxFuture<'_, String> {
        OpenAiTranscriber::probe(self).boxed()
    }

    fn refresh(&self) -> BoxFuture<'static, ()> {
        OpenAiTranscriber::refresh(self)
    }

    fn forget(&self) {
        OpenAiTranscriber::forget(self)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::{oneshot, Notify};

    use super::*;

    /// What the scripted server does with the next request.
    pub(crate) enum Reply {
        Answer {
            status: u16,
            content_type: &'static str,
            body: String,
        },
        /// Reads the request and drops the connection: the server went away.
        Close,
        /// The reply the test sends later.
        Held(oneshot::Receiver<Reply>),
    }

    pub(crate) struct Request {
        pub path: String,
        pub head: String,
        pub body: Vec<u8>,
    }

    /// A server that answers each request with the next reply of its script, and keeps what it was asked.
    pub(crate) struct ScriptedServer {
        pub origin: String,
        script: Arc<Mutex<VecDeque<Reply>>>,
        pub requests: Arc<Mutex<Vec<Request>>>,
        arrived: Arc<Notify>,
    }

    impl ScriptedServer {
        pub async fn start() -> ScriptedServer {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let script: Arc<Mutex<VecDeque<Reply>>> = Arc::default();
            let requests: Arc<Mutex<Vec<Request>>> = Arc::default();
            let arrived = Arc::new(Notify::new());
            let (s, r, a) = (script.clone(), requests.clone(), arrived.clone());
            tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let (script, requests, arrived) = (s.clone(), r.clone(), a.clone());
                    tokio::spawn(async move {
                        let Some(request) = read_request(&mut socket).await else { return };
                        requests.lock().unwrap().push(request);
                        arrived.notify_waiters();
                        let mut reply = script.lock().unwrap().pop_front().unwrap_or(Reply::Answer {
                            status: 500,
                            content_type: "text/plain",
                            body: "an unexpected request".into(),
                        });
                        while let Reply::Held(later) = reply {
                            reply = later.await.unwrap_or(Reply::Close);
                        }
                        if let Reply::Answer { status, content_type, body } = reply {
                            let head = format!(
                                "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = socket.write_all(head.as_bytes()).await;
                            let _ = socket.write_all(body.as_bytes()).await;
                            let _ = socket.shutdown().await;
                        }
                    });
                }
            });
            ScriptedServer { origin, script, requests, arrived }
        }

        pub fn then(&self, reply: Reply) {
            self.script.lock().unwrap().push_back(reply);
        }

        pub fn count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }

        /// Until `count` requests have come.
        pub async fn wait_for(&self, count: usize) {
            loop {
                let arrived = self.arrived.notified();
                if self.count() >= count {
                    return;
                }
                tokio::time::timeout(Duration::from_secs(5), arrived).await.expect("the request comes");
            }
        }
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Request> {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        let head_end = loop {
            let read = socket.read(&mut buffer).await.ok()?;
            if read == 0 {
                return None;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(at) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let head = String::from_utf8_lossy(&bytes[..head_end]).to_string();
        let length = head
            .lines()
            .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").map(|n| n.trim().to_string()))
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0);
        while bytes.len() < head_end + length {
            let read = socket.read(&mut buffer).await.ok()?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
        let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
        Some(Request { path, head, body: bytes[head_end..].to_vec() })
    }

    /// `GET /models` the OpenAI way.
    fn models(ids: &[&str]) -> Reply {
        let data: Vec<Value> = ids.iter().map(|id| serde_json::json!({"id": id, "object": "model"})).collect();
        Reply::Answer {
            status: 200,
            content_type: "application/json",
            body: serde_json::json!({"object": "list", "data": data}).to_string(),
        }
    }

    fn config(base_url: &str) -> AsrConfig {
        AsrConfig {
            enabled: true,
            base_url: base_url.to_string(),
            api_key: None,
            model: "gigaam-v3".into(),
            language: None,
            prompt: None,
            timeout_ms: 8000,
            wake_words: vec![],
        }
    }

    struct Setup {
        server: ScriptedServer,
        now: Arc<AtomicU64>,
        asr: OpenAiTranscriber,
    }

    async fn setup() -> Setup {
        let server = ScriptedServer::start().await;
        let now = Arc::new(AtomicU64::new(0));
        let clock = now.clone();
        let asr = OpenAiTranscriber::with_clock(config(&format!("{}/v1/", server.origin)), move || {
            clock.load(Ordering::SeqCst)
        });
        Setup { server, now, asr }
    }

    #[tokio::test]
    async fn names_the_model_the_server_serves_not_the_one_asr_model_asks_for_ours_lists_only_what_it_loaded() {
        let Setup { server, asr, .. } = setup().await;
        server.then(models(&["parakeet-0.6b-v3"]));
        assert_eq!(asr.model(), "gigaam-v3");

        assert_eq!(asr.probe().await, "reachable");
        assert_eq!(server.requests.lock().unwrap()[0].path, "/v1/models");
        assert_eq!(asr.model(), "parakeet-0.6b-v3");
        assert_eq!(asr.description(), format!("parakeet-0.6b-v3 via {}/v1/audio/transcriptions", server.origin));
    }

    #[tokio::test]
    async fn takes_a_server_of_many_models_a_cloud_as_serving_the_one_asked_for_and_does_not_ask_it_again() {
        let Setup { server, now, asr } = setup().await;
        server.then(models(&["whisper-1", "gpt-4o-mini-transcribe"]));
        asr.probe().await;
        assert_eq!(asr.model(), "gigaam-v3");

        now.store(3_600_000, Ordering::SeqCst);
        asr.refresh().await;
        assert_eq!(server.count(), 1);
    }

    #[tokio::test]
    async fn asks_again_a_minute_after_an_answer_ten_seconds_after_none_at_once_after_a_restart() {
        let Setup { server, now, asr } = setup().await;
        server.then(models(&["parakeet-0.6b-v3"]));
        asr.probe().await;

        now.store(59_000, Ordering::SeqCst);
        asr.refresh().await;
        assert_eq!(server.count(), 1);

        now.store(60_000, Ordering::SeqCst);
        server.then(Reply::Close);
        asr.refresh().await;
        // Down: its last word stands until it says another.
        assert_eq!(asr.model(), "parakeet-0.6b-v3");

        now.store(69_000, Ordering::SeqCst);
        asr.refresh().await;
        assert_eq!(server.count(), 2);
        now.store(70_000, Ordering::SeqCst);
        server.then(models(&["multilingual_ctc"]));
        asr.refresh().await;
        assert_eq!(asr.model(), "multilingual_ctc");

        // The panel restarted it with another model: asked at once, not a minute later.
        now.store(71_000, Ordering::SeqCst);
        asr.forget();
        server.then(models(&["v3_e2e_rnnt"]));
        asr.refresh().await;
        assert_eq!(asr.model(), "v3_e2e_rnnt");
        assert_eq!(server.count(), 4);
    }

    #[tokio::test]
    async fn asks_one_at_a_time_and_drops_an_answer_asked_for_before_a_restart() {
        let Setup { server, asr, .. } = setup().await;
        let (answer, later) = oneshot::channel();
        server.then(Reply::Held(later));
        let first = asr.refresh();
        let second = asr.refresh();
        server.wait_for(1).await;
        assert_eq!(server.count(), 1);

        asr.forget();
        let _ = answer.send(models(&["parakeet-0.6b-v3"]));
        futures::future::join(first, second).await;
        assert_eq!(asr.model(), "gigaam-v3");
        assert_eq!(server.count(), 1);

        server.then(models(&["multilingual_ctc"]));
        asr.refresh().await;
        assert_eq!(asr.model(), "multilingual_ctc");
    }

    #[tokio::test]
    async fn tells_the_startup_log_why_the_server_is_not_there_and_keeps_the_model_asked_for() {
        let gone = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = gone.local_addr().unwrap();
        drop(gone);
        let state = OpenAiTranscriber::new(config(&format!("http://{address}/v1"))).probe().await;
        assert!(state.starts_with("unreachable ("), "{state}");
        assert!(state.to_lowercase().contains("refused"), "{state}");

        let Setup { server, asr, .. } = setup().await;
        server.then(Reply::Answer { status: 404, content_type: "text/plain", body: "no route".into() });
        assert_eq!(asr.probe().await, "answers HTTP 404 on /models");
        assert_eq!(asr.model(), "gigaam-v3");
    }

    #[tokio::test]
    async fn sends_the_utterance_as_one_wav_with_the_model_and_takes_the_text() {
        let server = ScriptedServer::start().await;
        let asr = OpenAiTranscriber::new(AsrConfig {
            api_key: Some("asr-key".into()),
            language: Some("ru".into()),
            ..config(&format!("{}/v1", server.origin))
        });
        server.then(Reply::Answer {
            status: 200,
            content_type: "application/json",
            body: r#"{"text": " Алиса, привет. "}"#.into(),
        });
        let text = asr.recognize(&[0i16; 1600], 16000, &CancellationToken::new()).await.unwrap();
        assert_eq!(text, "Алиса, привет.");
        {
            let requests = server.requests.lock().unwrap();
            let request = &requests[0];
            assert_eq!(request.path, "/v1/audio/transcriptions");
            assert!(request.head.to_lowercase().contains("authorization: bearer asr-key"));
            let body = String::from_utf8_lossy(&request.body);
            assert!(body.contains("filename=\"speech.wav\""));
            assert!(body.contains("name=\"model\"\r\n\r\ngigaam-v3"));
            assert!(body.contains("name=\"language\"\r\n\r\nru"));
        }

        server.then(Reply::Answer { status: 503, content_type: "text/plain", body: "busy".into() });
        let error = asr.recognize(&[0i16; 160], 16000, &CancellationToken::new()).await.unwrap_err();
        assert_eq!(error.to_string(), "HTTP 503: busy");
    }

    #[tokio::test]
    async fn reads_the_answer_as_utf_8_whatever_charset_its_content_type_names() {
        let server = ScriptedServer::start().await;
        let asr = OpenAiTranscriber::new(config(&format!("{}/v1", server.origin)));
        let cancel = CancellationToken::new();
        let recognize = || asr.recognize(&[0i16; 160], 16000, &cancel);
        server.then(Reply::Answer {
            status: 200,
            content_type: "application/json; charset=iso-8859-1",
            body: r#"{"text": "Алиса, включи свет"}"#.into(),
        });
        assert_eq!(recognize().await.unwrap(), "Алиса, включи свет");
        // A leading BOM is dropped, as fetch drops it.
        server.then(Reply::Answer {
            status: 200,
            content_type: "application/json",
            body: "\u{feff}{\"text\": \"Привет\"}".into(),
        });
        assert_eq!(recognize().await.unwrap(), "Привет");
        server.then(Reply::Answer {
            status: 503,
            content_type: "text/plain; charset=windows-1251",
            body: "занято".into(),
        });
        assert_eq!(recognize().await.unwrap_err().to_string(), "HTTP 503: занято");
    }
}
