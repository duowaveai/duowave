//! Passwords: scrypt (N=16384, r=8, p=1, 32 bytes), stored as `scrypt:<salt>:<digest>`, both hex.

// scrypt off the async runtime: the gateway streams the station's audio on the same threads, and a hash (~50 ms) at
// every sign-in would be heard as a stutter. And a few at a time: each takes 16 MiB, and a burst of sign-ins (an
// unknown name costs a hash too) must not take the station's memory — Node queued them behind libuv's four threads.

use std::sync::{Arc, LazyLock};

use serde_json::Value;
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

use super::js_len;
use crate::platform::http::ApiError;
use crate::utils::ids::random_hex;

/// N = 2^14 = 16384.
const LOG_N: u8 = 14;
const R: u32 = 8;
const P: u32 = 1;
const KEY_LENGTH: usize = 32;

/// The smallest password the panel takes; a home tool, but not a four-digit one.
pub const MIN_PASSWORD: usize = 8;
const MAX_PASSWORD: usize = 256;

/// Hashes at once in the whole process: 16 MiB each, so 32 MiB at most on the station, 64 on a PC.
pub const PARALLEL_HASHES: usize = if cfg!(feature = "pc") { 4 } else { 2 };
const _: () = assert!(PARALLEL_HASHES >= 1 && PARALLEL_HASHES <= 4, "16 MiB a hash");

static HASHING: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(PARALLEL_HASHES)));

fn derive(password: &str, salt: &str) -> [u8; KEY_LENGTH] {
    let params = scrypt::Params::new(LOG_N, R, P).expect("the scrypt parameters are valid");
    let mut key = [0u8; KEY_LENGTH];
    scrypt::scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut key).expect("a 32-byte output is valid");
    key
}

/// Runs `work` on the blocking pool once one of the permits is free. The permit goes with the work, so a caller that
/// stops waiting (a client gone mid-hash) does not free its place before the hash is done.
async fn limited<T: Send + 'static>(
    permits: &Arc<Semaphore>,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    let permit = permits.clone().acquire_owned().await.map_err(ApiError::internal)?;
    tokio::task::spawn_blocking(move || {
        let done = work();
        drop(permit);
        done
    })
    .await
    .map_err(ApiError::internal)
}

async fn derive_blocking(password: &str, salt: &str) -> Result<[u8; KEY_LENGTH], ApiError> {
    let (password, salt) = (password.to_string(), salt.to_string());
    limited(&HASHING, move || derive(&password, &salt)).await
}

/// `scrypt:<salt>:<digest>`, both hex. The salt goes into scrypt as its hex text, as the Node gateway gave it.
pub async fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = random_hex(16);
    let digest = derive_blocking(password, &salt).await?;
    Ok(format!("scrypt:{salt}:{}", hex::encode(digest)))
}

pub async fn verify_password(password: &str, stored: Option<&str>) -> bool {
    let mut parts = stored.unwrap_or("").split(':');
    let (Some("scrypt"), Some(salt), Some(digest)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if salt.is_empty() || digest.is_empty() {
        return false;
    }
    let Ok(expected) = hex::decode(digest) else { return false };
    let Ok(actual) = derive_blocking(password, salt).await else { return false };
    expected.len() == actual.len() && bool::from(expected.ct_eq(&actual))
}

/// What is wrong with a new password, in the panel's words; None when it will do.
pub fn password_problem(password: Option<&Value>) -> Option<String> {
    match password.and_then(Value::as_str) {
        Some(text) if js_len(text) >= MIN_PASSWORD => {
            (js_len(text) > MAX_PASSWORD).then(|| "Пароль слишком длинный.".to_string())
        }
        _ => Some(format!("Пароль — хотя бы {MIN_PASSWORD} символов.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn hashes_a_few_at_a_time_however_many_sign_in_at_once_even_when_callers_give_up() {
        let permits = Arc::new(Semaphore::new(2));
        let (running, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let mut calls = Vec::new();
        for n in 0..12 {
            let (permits, running, peak) = (permits.clone(), running.clone(), peak.clone());
            calls.push(tokio::spawn(async move {
                let call = limited(&permits, move || {
                    peak.fetch_max(running.fetch_add(1, SeqCst) + 1, SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    running.fetch_sub(1, SeqCst);
                });
                // Every third caller goes away mid-hash: its work keeps its place until it is done.
                if n % 3 == 0 {
                    let _ = tokio::time::timeout(Duration::from_millis(5), call).await;
                } else {
                    call.await.unwrap();
                }
            }));
        }
        for call in calls {
            call.await.unwrap();
        }
        while permits.available_permits() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(peak.load(SeqCst), 2);
    }

    #[tokio::test]
    async fn hashes_with_a_salt_of_its_own_and_verifies_only_the_same_password() {
        let stored = hash_password("пароль-для-пульта").await.unwrap();
        let parts: Vec<&str> = stored.split(':').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "scrypt");
        assert_eq!(parts[1].len(), 32, "16 random bytes as hex");
        assert_eq!(parts[2].len(), 64, "32 bytes of digest as hex");
        assert!(verify_password("пароль-для-пульта", Some(&stored)).await);
        assert!(!verify_password("пароль-для-пульт", Some(&stored)).await);
        assert_ne!(hash_password("пароль-для-пульта").await.unwrap(), stored, "every hash has its own salt");
    }

    #[tokio::test]
    async fn refuses_what_is_not_a_hash_of_ours() {
        assert!(!verify_password("x", None).await);
        assert!(!verify_password("x", Some("")).await);
        assert!(!verify_password("x", Some("bcrypt:aa:bb")).await);
        assert!(!verify_password("x", Some("scrypt::bb")).await);
        assert!(!verify_password("x", Some("scrypt:aa:not-hex")).await);
    }

    #[test]
    fn names_what_is_wrong_with_a_new_password() {
        assert_eq!(password_problem(None).as_deref(), Some("Пароль — хотя бы 8 символов."));
        assert_eq!(password_problem(Some(&json!(12345678))).as_deref(), Some("Пароль — хотя бы 8 символов."));
        assert_eq!(password_problem(Some(&json!("short"))).as_deref(), Some("Пароль — хотя бы 8 символов."));
        assert_eq!(password_problem(Some(&json!("восемь!!"))), None);
        assert_eq!(password_problem(Some(&json!("x".repeat(257)))).as_deref(), Some("Пароль слишком длинный."));
        assert_eq!(password_problem(Some(&json!("x".repeat(256)))), None);
    }
}
