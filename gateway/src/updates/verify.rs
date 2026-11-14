//! Update signatures: minisign, with the key `tauri signer generate` made (Tauri keeps both the key and the signature
//! as base64 of minisign's own text). A file without a valid signature by this key is never installed.
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};

/// The keys releases are signed with, built into the program (one per line): the release key, then a reserve kept
/// offline for the day the release key is lost (docs/plans/key-rotation.md). UPDATE_PUBKEY replaces them.
pub const BUILT_IN_KEY: &str = include_str!("../../updates.pub");

/// A public key in Tauri's form (base64 of the .pub file), or minisign's own text, or its bare base64 line.
pub fn public_key(text: &str) -> Result<PublicKey, String> {
    let text = text.trim();
    if let Ok(key) = PublicKey::from_base64(text) {
        return Ok(key);
    }
    let decoded = B64.decode(text).ok().and_then(|bytes| String::from_utf8(bytes).ok()).unwrap_or_else(|| text.into());
    PublicKey::decode(&decoded).map_err(|error| crate::tr!("Ключ обновлений не читается ({error})", error = error))
}

/// All the keys from `text`: one per non-empty line (`updates.pub`, `UPDATE_PUBKEY`), so a rotation adds its key
/// beside the one already trusted. An unreadable line is an error, not a shorter list — a mistyped key must turn
/// updates off with a visible message, not quietly shrink what is trusted. minisign's own text keeps its comment on
/// a line of its own: that line is no key and is skipped, so the .pub file of minisign reads as its one key.
pub fn public_keys(text: &str) -> Result<Vec<PublicKey>, String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("untrusted comment:"))
        .collect();
    if lines.is_empty() {
        return Err(crate::tr!("ключи обновлений не читаются: нет ни одной строки"));
    }
    lines.into_iter().map(public_key).collect()
}

fn mismatch() -> String {
    crate::tr!("Подпись не сходится: выпуск подписан не нашим ключом или повреждён.")
}

/// Whether `signature` (Tauri's base64 of the .sig text, or the text itself) signs `data` with `key`.
pub fn verify(key: &PublicKey, data: &[u8], signature: &str) -> Result<(), String> {
    let signature = signature.trim();
    let text =
        B64.decode(signature).ok().and_then(|bytes| String::from_utf8(bytes).ok()).unwrap_or_else(|| signature.into());
    let signature =
        Signature::decode(&text).map_err(|error| crate::tr!("Подпись выпуска не читается ({error})", error = error))?;
    key.verify(data, &signature, false).map_err(|_| mismatch())
}

/// Whether `signature` signs `data` with any of `keys`: one trusted key is enough, so a release signed by a key
/// that arrived with a rotation installs while the old key is still in the list.
pub fn verify_any(keys: &[PublicKey], data: &[u8], signature: &str) -> Result<(), String> {
    keys.iter().find_map(|key| verify(key, data, signature).ok()).ok_or_else(mismatch)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway key, never trusted by any installation: `cargo tauri signer generate`, then `signer sign` of the
    /// payload below. The release key never signs test data (a signed dummy would install as an update).
    const THROWAWAY: &str =
        "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDA1RkY5MzI3Q0Y2OTA2NTIKUldSU0JtblBKNVAvQlhKZmgrK084L2R5WTN5VWxXd2RmTjcxbFZzeTdBanRDaWVrQTFPVTRIeWEK";
    const THROWAWAY_SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSU0JtblBKNVAvQmIrK21mcnJMYW1PcEFTbFV1T3dEa0N6M1VRWjNCbWR4a3pPdHhOTkkrS1VpT2FyTWkwQlQvaFQxY0pDRS9rRGpMSDJOb2cwV2JBU08xdFF4YkZuT2dVPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0MDU1NDEzCWZpbGU6cGF5bG9hZC50eHQKdU5TNjZjbDNudUpyOVgrS2VRREFDZU11VGZZZkpNTlRZSjNEeHB6eWExcW8yTkpQaExHbFVTMWlIMHlKU2FwUTdmQXVZWWRWYlc3OHpUUGx4T1piQUE9PQo=";
    const THROWAWAY_PAYLOAD: &[u8] = b"duowave: a signed update, for the test";

    #[test]
    fn the_built_in_keys_read() {
        // The release key and the reserve one, kept offline for the day the release key is lost
        // (docs/plans/key-rotation.md): a change here is a rotation, made on purpose.
        assert_eq!(public_keys(BUILT_IN_KEY).expect("updates.pub reads").len(), 2, "the release key and the reserve");
        assert!(public_key("nonsense").is_err());
    }

    #[test]
    fn checks_a_signature_made_by_minisign() {
        // minisign-verify's own example: a key and its prehashed signature of "test" (not the release key).
        let key = public_key("RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3").unwrap();
        let signature = "untrusted comment: signature from minisign secret key
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=
trusted comment: timestamp:1633700835\tfile:test\tprehashed
wLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==";
        assert!(verify(&key, b"test", signature).is_ok());
        assert!(verify(&key, b"tampered", signature).is_err());
        assert!(verify(&key, b"test", &B64.encode(signature)).is_ok(), "Tauri's base64 form");
    }

    #[test]
    fn accepts_what_tauri_signer_signs() {
        let key = public_key(THROWAWAY).unwrap();
        assert!(verify(&key, THROWAWAY_PAYLOAD, THROWAWAY_SIGNATURE).is_ok());
        assert!(verify(&key, b"duowave: something else", THROWAWAY_SIGNATURE).is_err());
        let release = public_keys(BUILT_IN_KEY).unwrap();
        assert!(verify_any(&release, THROWAWAY_PAYLOAD, THROWAWAY_SIGNATURE).is_err(), "another key's signature");
    }

    #[test]
    fn a_list_of_keys_accepts_any_of_them_and_only_them() {
        let example = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3"; // minisign-verify's own example
        let keys = public_keys(&format!("{example}\n\n{THROWAWAY}\n")).expect("one key per non-empty line");
        assert_eq!(keys.len(), 2);
        assert!(verify_any(&keys, THROWAWAY_PAYLOAD, THROWAWAY_SIGNATURE).is_ok(), "the second key's signature");
        let one = public_keys(example).unwrap();
        assert!(verify_any(&one, THROWAWAY_PAYLOAD, THROWAWAY_SIGNATURE).is_err(), "not with the first key alone");
    }

    #[test]
    fn an_unreadable_key_line_is_an_error_not_a_shorter_list() {
        assert!(public_keys("nonsense").is_err());
        assert!(public_keys(&format!("{THROWAWAY}\nnonsense")).is_err(), "a broken line fails the whole list");
        assert!(public_keys("\n  \n").is_err(), "no lines at all");
        assert!(public_keys(&format!("{BUILT_IN_KEY}\nnonsense")).is_err(), "nor after the built-in keys");
    }

    #[test]
    fn minisigns_own_text_is_its_one_key() {
        let example = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3"; // minisign-verify's own example
        let text = format!("untrusted comment: minisign public key E7620F1842B4E81F\n{example}\n");
        assert_eq!(public_keys(&text).expect("the comment line is skipped").len(), 1);
        let two = format!("{text}untrusted comment: the next key\n{THROWAWAY}\n");
        assert_eq!(public_keys(&two).unwrap().len(), 2, "two keys in minisign's text, each with its comment");
        assert!(public_keys("untrusted comment: nothing after it").is_err(), "a comment alone is no key");
    }
}
