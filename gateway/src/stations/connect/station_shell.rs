//! The gateway's own SSH to a station: root, an empty password, port 2222 — as the hackalice wiki opens it. In the
//! gateway itself (russh on ring), so the panel's button works the same in Docker, on Windows and on a Mac, with no
//! ssh, bash or python on the machine. The station's host key is learned on first use and held to afterwards: a
//! different key is another station, or a reflashed one, and the gateway stops there.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use futures::future::BoxFuture;
use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{HashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect, MethodKind};
use sha2::{Digest, Sha256};
use tokio::time::timeout;

use super::discovery::ssh_banner;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// A shell on a station: one command at a time.
pub trait StationShell: Send + Sync {
    /// Runs one command; `input` goes to its stdin. Fails on a timeout or a lost connection.
    fn run<'a>(&'a self, command: &'a str, input: Option<&'a str>) -> BoxFuture<'a, anyhow::Result<ShellResult>>;
    /// The host key as OpenSSH prints it: SHA256:…
    fn fingerprint(&self) -> &str;
    fn close(&self) -> BoxFuture<'_, ()>;
    /// Writes `bytes` to `path` on the station, whole or not at all: beside it first, then renamed over it. The
    /// program for the station's own gateway goes this way (12 MB).
    fn upload<'a>(&'a self, path: &'a str, bytes: &'a [u8]) -> BoxFuture<'a, anyhow::Result<()>> {
        let _ = (path, bytes);
        Box::pin(async { anyhow::bail!("this shell cannot write files") })
    }
}

/// How long an upload may take: 12 MB over the home network is seconds; a slow Wi-Fi gets a few minutes.
const UPLOAD_MS: u64 = 300_000;

#[derive(Debug, Clone)]
pub struct ShellOptions {
    pub port: u16,
    pub user: String,
    pub password: String,
    /// The fingerprint the gateway learned before; a different key refuses the connection.
    pub trust: Option<String>,
    pub timeout_ms: Option<u64>,
}

/// The station's host key is not the one learned before.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the station’s SSH key changed")]
pub struct HostKeyChanged {
    pub fingerprint: String,
}

/// Why a shell did not open, in the words the connect service turns into the panel's problems.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShellError {
    #[error("All configured authentication methods failed")]
    AuthFailed,
    /// Nothing answered: refused, no route, or no handshake in time.
    #[error("{0}")]
    Unreachable(String),
    #[error("{0}")]
    Other(String),
}

/// What reaches a station: its SSH banner (reading only) and its shell. TCP and russh here, a pretend one in tests.
pub trait ShellOpener: Send + Sync {
    /// The SSH software that answers at address:port, or None.
    fn banner<'a>(&'a self, address: &'a str, port: u16) -> BoxFuture<'a, Option<String>>;
    fn open<'a>(
        &'a self,
        address: &'a str,
        options: ShellOptions,
    ) -> BoxFuture<'a, anyhow::Result<Box<dyn StationShell>>>;
}

/// A host key blob's fingerprint as OpenSSH prints it.
pub fn fingerprint_of(key: &[u8]) -> String {
    format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(key)))
}

/// How long a command may take.
const RUN_MS: u64 = 20_000;

/// The real one: TCP for the banner, russh for the shell.
pub struct RusshOpener;

impl ShellOpener for RusshOpener {
    fn banner<'a>(&'a self, address: &'a str, port: u16) -> BoxFuture<'a, Option<String>> {
        Box::pin(ssh_banner(address, port, 1500))
    }

    fn open<'a>(
        &'a self,
        address: &'a str,
        options: ShellOptions,
    ) -> BoxFuture<'a, anyhow::Result<Box<dyn StationShell>>> {
        Box::pin(async move { open_station_shell(address, options).await.map(|shell| Box::new(shell) as _) })
    }
}

/// Checks the host key against the one learned before, and keeps the one it saw.
struct KeyCheck {
    trust: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
}

impl client::Handler for KeyCheck {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let fingerprint = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.fingerprint(HashAlg::Sha256).to_string(),
            PublicKeyOrCertificate::Certificate(certificate) => {
                certificate.public_key().fingerprint(HashAlg::Sha256).to_string()
            }
        };
        // An empty pin is none, as ssh2's verifier took a falsy trust: it would refuse every key.
        let trusted = self.trust.as_deref().filter(|trust| !trust.is_empty()).is_none_or(|trust| trust == fingerprint);
        *self.seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(fingerprint);
        Ok(trusted)
    }
}

struct RusshShell {
    handle: tokio::sync::Mutex<Handle<KeyCheck>>,
    fingerprint: String,
}

/// Connects, holds the host key to the one learned, and signs in: none, then the password, then keyboard-interactive.
async fn open_station_shell(address: &str, options: ShellOptions) -> anyhow::Result<RusshShell> {
    let seen = Arc::new(Mutex::new(None));
    let check = KeyCheck { trust: options.trust.clone(), seen: seen.clone() };
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(5)),
        nodelay: true,
        ..Default::default()
    });
    let ready = Duration::from_millis(options.timeout_ms.unwrap_or(10_000));
    let user = options.user.clone();
    let password = options.password.clone();
    let opened = timeout(ready, async move {
        let mut handle = client::connect(config, (address, options.port), check).await?;
        sign_in(&mut handle, &user, &password).await?;
        Ok::<_, anyhow::Error>(handle)
    })
    .await;
    let fingerprint = seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    let handle = match opened {
        Ok(Ok(handle)) => handle,
        Ok(Err(error)) => {
            let changed = matches!(error.downcast_ref::<russh::Error>(), Some(russh::Error::UnknownKey));
            if let (true, Some(fingerprint)) = (changed, fingerprint) {
                return Err(HostKeyChanged { fingerprint }.into());
            }
            return Err(classify(error));
        }
        Err(_) => return Err(ShellError::Unreachable("Timed out while waiting for handshake".into()).into()),
    };
    Ok(RusshShell { handle: tokio::sync::Mutex::new(handle), fingerprint: fingerprint.unwrap_or_default() })
}

/// The wiki's dropbear takes the empty password as `none`; the password and keyboard-interactive answer the same
/// elsewhere. Tried in that order, stopping at the first the server accepts.
async fn sign_in(handle: &mut Handle<KeyCheck>, user: &str, password: &str) -> anyhow::Result<()> {
    let mut offered = match handle.authenticate_none(user).await? {
        AuthResult::Success => return Ok(()),
        AuthResult::Failure { remaining_methods, .. } => remaining_methods,
    };
    let allows = |methods: &russh::MethodSet, kind: MethodKind| methods.is_empty() || methods.contains(&kind);
    if allows(&offered, MethodKind::Password) {
        match handle.authenticate_password(user, password).await? {
            AuthResult::Success => return Ok(()),
            AuthResult::Failure { remaining_methods, .. } => offered = remaining_methods,
        }
    }
    if allows(&offered, MethodKind::KeyboardInteractive) {
        let mut answer = handle.authenticate_keyboard_interactive_start(user, None).await?;
        // A server may ask again, or ask nothing; a few rounds are plenty for an empty password.
        for _ in 0..5 {
            match answer {
                KeyboardInteractiveAuthResponse::Success => return Ok(()),
                KeyboardInteractiveAuthResponse::Failure { .. } => break,
                KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                    let responses = prompts.iter().map(|_| password.to_string()).collect();
                    answer = handle.authenticate_keyboard_interactive_respond(responses).await?;
                }
            }
        }
    }
    Err(ShellError::AuthFailed.into())
}

/// Nothing there, or something else.
fn classify(error: anyhow::Error) -> anyhow::Error {
    if error.downcast_ref::<ShellError>().is_some() {
        return error;
    }
    let unreachable = match error.downcast_ref::<russh::Error>() {
        Some(russh::Error::IO(io)) => matches!(
            io.kind(),
            std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::HostUnreachable
                | std::io::ErrorKind::NetworkUnreachable
                | std::io::ErrorKind::TimedOut
        ),
        Some(russh::Error::ConnectionTimeout) => true,
        _ => false,
    };
    if unreachable {
        ShellError::Unreachable(error.to_string()).into()
    } else {
        ShellError::Other(error.to_string()).into()
    }
}

impl StationShell for RusshShell {
    fn run<'a>(&'a self, command: &'a str, input: Option<&'a str>) -> BoxFuture<'a, anyhow::Result<ShellResult>> {
        Box::pin(async move {
            let work = async {
                let handle = self.handle.lock().await;
                let mut channel = handle.channel_open_session().await?;
                drop(handle);
                channel.exec(true, command).await?;
                if let Some(input) = input {
                    channel.data(input.as_bytes()).await?;
                }
                channel.eof().await?;
                let (mut stdout, mut stderr, mut code) = (Vec::new(), Vec::new(), None);
                while let Some(message) = channel.wait().await {
                    match message {
                        ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                        ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
                        ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status as i32),
                        ChannelMsg::Close => break,
                        _ => {}
                    }
                }
                Ok::<_, anyhow::Error>(ShellResult {
                    code: code.unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                })
            };
            match timeout(Duration::from_millis(RUN_MS), work).await {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!(
                    "no answer in {} s: {}",
                    RUN_MS / 1000,
                    command.split(' ').next().unwrap_or_default()
                )),
            }
        })
    }

    fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    fn upload<'a>(&'a self, path: &'a str, bytes: &'a [u8]) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            if !path.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte)) {
                anyhow::bail!("not a path to write: {path}");
            }
            let command = format!("cat > '{path}.incoming' && mv -f '{path}.incoming' '{path}'");
            let work = async {
                let handle = self.handle.lock().await;
                let mut channel = handle.channel_open_session().await?;
                drop(handle);
                channel.exec(true, command.as_str()).await?;
                channel.data(bytes).await?;
                channel.eof().await?;
                let (mut stderr, mut code) = (Vec::new(), None);
                while let Some(message) = channel.wait().await {
                    match message {
                        ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
                        ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status as i32),
                        ChannelMsg::Close => break,
                        _ => {}
                    }
                }
                match code {
                    Some(0) => Ok(()),
                    _ => Err(anyhow::anyhow!("{path} not written: {}", String::from_utf8_lossy(&stderr).trim())),
                }
            };
            match timeout(Duration::from_millis(UPLOAD_MS), work).await {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("{path} not written in {} s", UPLOAD_MS / 1000)),
            }
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let handle = self.handle.lock().await;
            let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
        })
    }
}

#[cfg(test)]
mod tests {
    //! The shell against russh's own server on loopback — never a station on the network.

    use std::net::Ipv4Addr;

    use russh::keys::{Algorithm, PrivateKey};
    use russh::server::{self, Auth, Msg, Session};
    use russh::{Channel, ChannelId};
    use tokio::net::TcpListener;

    use super::*;

    /// Which auth methods the pretend server accepts.
    #[derive(Clone, Copy)]
    enum Accept {
        None,
        KeyboardInteractive,
        Nothing,
    }

    #[derive(Clone)]
    struct Server {
        accept: Accept,
    }

    impl server::Server for Server {
        type Handler = Handler;
        fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> Handler {
            Handler { accept: self.accept, command: None }
        }
    }

    struct Handler {
        accept: Accept,
        command: Option<Vec<u8>>,
    }

    impl server::Handler for Handler {
        type Error = russh::Error;

        async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
            Ok(match self.accept {
                Accept::None => Auth::Accept,
                _ => Auth::reject(),
            })
        }

        async fn auth_keyboard_interactive(
            &mut self,
            _user: &str,
            _submethods: &str,
            _response: Option<server::Response<'_>>,
        ) -> Result<Auth, Self::Error> {
            Ok(match self.accept {
                Accept::KeyboardInteractive => Auth::Accept,
                _ => Auth::reject(),
            })
        }

        async fn channel_open_session(
            &mut self,
            _channel: Channel<Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: ChannelId,
            command: &[u8],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.command = Some(command.to_vec());
            session.channel_success(channel)?;
            Ok(())
        }

        async fn data(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
            // The pretend station answers `cat` by echoing its stdin; any other command names itself on stderr.
            let command = self.command.clone().unwrap_or_default();
            if command == b"cat" {
                session.data(channel, data.to_vec())?;
            }
            Ok(())
        }

        async fn channel_eof(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
            let command = self.command.clone().unwrap_or_default();
            if command != b"cat" {
                let message = format!("ran: {}", String::from_utf8_lossy(&command));
                session.extended_data(channel, 1, message.into_bytes())?;
            }
            session.exit_status_request(channel, if command.is_empty() { 1 } else { 0 })?;
            session.eof(channel)?;
            session.close(channel)?;
            Ok(())
        }
    }

    /// A pretend SSH server on loopback with a fixed host key; returns the port and the key's fingerprint.
    async fn pretend(accept: Accept) -> (u16, String) {
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::from_millis(1),
            auth_rejection_time_initial: Some(Duration::from_millis(0)),
            ..Default::default()
        });
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut server = Server { accept };
            while let Ok((socket, _peer)) = listener.accept().await {
                let config = config.clone();
                let handler = server::Server::new_client(&mut server, None);
                tokio::spawn(async move {
                    let _ = server::run_stream(config, socket, handler).await;
                });
            }
        });
        (port, fingerprint)
    }

    fn options(trust: Option<String>) -> ShellOptions {
        ShellOptions { port: 0, user: "root".into(), password: String::new(), trust, timeout_ms: Some(4000) }
    }

    #[tokio::test]
    async fn signs_in_with_no_password_runs_a_command_and_learns_the_key() {
        let (port, fingerprint) = pretend(Accept::None).await;
        let shell = open_station_shell("127.0.0.1", ShellOptions { port, ..options(None) }).await.unwrap();
        assert_eq!(shell.fingerprint(), fingerprint, "the host key is learned on first use");
        let echoed = shell.run("cat", Some("hello station")).await.unwrap();
        assert_eq!(echoed.stdout, "hello station");
        assert_eq!(echoed.code, 0);
        let other = shell.run("uname -m", None).await.unwrap();
        assert_eq!(other.stderr, "ran: uname -m");
        shell.close().await;
    }

    #[tokio::test]
    async fn answers_keyboard_interactive_when_that_is_what_the_server_asks() {
        let (port, _) = pretend(Accept::KeyboardInteractive).await;
        let shell = open_station_shell("127.0.0.1", ShellOptions { port, ..options(None) }).await.unwrap();
        assert_eq!(shell.run("cat", Some("ok")).await.unwrap().stdout, "ok");
        shell.close().await;
    }

    /// The error of an attempt that must not open a shell (the Ok side is not Debug).
    async fn fails(port: u16, trust: Option<String>) -> anyhow::Error {
        match open_station_shell("127.0.0.1", ShellOptions { port, ..options(trust) }).await {
            Ok(_) => panic!("the shell opened"),
            Err(error) => error,
        }
    }

    #[tokio::test]
    async fn stops_at_a_station_whose_key_is_not_the_one_it_learned() {
        let (port, fingerprint) = pretend(Accept::None).await;
        let error = fails(port, Some("SHA256:other".into())).await;
        let changed = error.downcast_ref::<HostKeyChanged>().expect("a host-key-changed");
        assert_eq!(changed.fingerprint, fingerprint);
        // The learned key opens it again.
        assert!(open_station_shell("127.0.0.1", ShellOptions { port, ..options(Some(fingerprint)) }).await.is_ok());
    }

    #[tokio::test]
    async fn takes_an_empty_pin_for_none_and_learns_the_key() {
        let (port, fingerprint) = pretend(Accept::None).await;
        let shell = open_station_shell("127.0.0.1", ShellOptions { port, ..options(Some(String::new())) }).await;
        let shell = shell.expect("an empty pin pins nothing");
        assert_eq!(shell.fingerprint(), fingerprint);
        shell.close().await;
    }

    #[tokio::test]
    async fn tells_an_auth_refusal_from_nothing_listening() {
        let (port, _) = pretend(Accept::Nothing).await;
        let refused = fails(port, None).await;
        assert!(matches!(refused.downcast_ref::<ShellError>(), Some(ShellError::AuthFailed)), "{refused}");
        // Nothing on the port: unreachable, not an auth failure.
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let dead = free.local_addr().unwrap().port();
        drop(free);
        let gone = fails(dead, None).await;
        assert!(matches!(gone.downcast_ref::<ShellError>(), Some(ShellError::Unreachable(_))), "{gone}");
    }

    #[test]
    fn prints_a_fingerprint_as_openssh_does() {
        assert_eq!(fingerprint_of(b""), "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU");
    }
}
