//! `duowave animations`: light animations Yandex sent the station, from the relay journals into
//! CONTENT_DIR/animations/<name>.b64, for the LED skill to replay (src/tools/extract-animations.ts, `pnpm animations`).
//!
//!   duowave animations [journal dir, default RECORDER_DIR] [content dir, default CONTENT_DIR]
//!
//! Names are the time they were seen; rename the ones worth keeping ("thinking", "win", …). An animation already in
//! the folder, under any name, is not written again.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::Failure;

const DIRECTIVE: &str = "draw_animation_directive";

/// A journal line with typed commands, as the relay writes it (gateway::uniproxy::relay_session): field 15 of each
/// directive whole, in base64.
#[derive(Deserialize)]
struct Record {
    #[serde(default)]
    ts: String,
    summary: Option<String>,
    data: Option<RecordData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordData {
    #[serde(default)]
    typed_payloads: Vec<TypedPayload>,
}

#[derive(Deserialize)]
struct TypedPayload {
    #[serde(default)]
    name: String,
    #[serde(default)]
    base64: String,
}

/// An animation written: its file's name without `.b64`, and the summary of the answer that carried it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    pub name: String,
    pub summary: String,
}

pub fn main(args: &[String]) -> anyhow::Result<()> {
    // The defaults are the gateway's own folders; the configuration is read only for them.
    let (journals, content) = match (args.first(), args.get(1)) {
        (Some(journals), Some(content)) => (PathBuf::from(journals), PathBuf::from(content)),
        (journals, content) => {
            let config = super::config()?;
            (
                journals.map_or(config.gateway.recorder.dir, PathBuf::from),
                content.map_or(config.skills.content_dir, PathBuf::from),
            )
        }
    };
    let journals = super::absolute(&journals);
    let target = super::absolute(&content).join("animations");
    if !journals.exists() {
        return Err(Failure(format!("no journals at {}", journals.display())).into());
    }
    let written = extract(&journals, &target, &mut |animation| {
        println!("{}.b64  {}", animation.name, animation.summary);
    })?;
    println!("{} new animation(s) in {}", written.len(), target.display());
    Ok(())
}

/// Every events.jsonl under the folder, at any depth, in name order.
fn journals(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(|entry| Some(entry.ok()?.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            journals(&path, found);
        } else if path.file_name().is_some_and(|name| name == "events.jsonl") {
            found.push(path);
        }
    }
}

fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Writes the animations of the journals under `journals` that `target` does not have yet, telling each as it is
/// written; all that were written.
pub fn extract(journals_dir: &Path, target: &Path, tell: &mut dyn FnMut(&Extracted)) -> anyhow::Result<Vec<Extracted>> {
    std::fs::create_dir_all(target).with_context(|| format!("cannot make {}", target.display()))?;
    let mut seen: HashSet<String> = std::fs::read_dir(target)
        .with_context(|| format!("cannot read {}", target.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "b64"))
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .map(|text| sha256_hex(text.trim()))
        .collect();
    let mut files = Vec::new();
    journals(journals_dir, &mut files);
    let mut written = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        for line in text.lines() {
            if !line.contains(DIRECTIVE) || !line.contains("typedPayloads") {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Record>(line) else { continue };
            let payloads = record.data.map(|data| data.typed_payloads).unwrap_or_default();
            for payload in payloads.into_iter().filter(|payload| payload.name == DIRECTIVE) {
                let hash = sha256_hex(&payload.base64);
                if !seen.insert(hash.clone()) {
                    continue;
                }
                let time: String =
                    record.ts.chars().take(19).filter(|character| !matches!(character, '-' | ':' | 'T')).collect();
                let name = format!("{time}_{}", &hash[..6]);
                let path = target.join(format!("{name}.b64"));
                std::fs::write(&path, format!("{}\n", payload.base64))
                    .with_context(|| format!("cannot write {}", path.display()))?;
                let animation = Extracted { name, summary: record.summary.clone().unwrap_or_default() };
                tell(&animation);
                written.push(animation);
            }
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::skills::own::led_skill::LedSkill;

    /// An answer from Yandex as the relay journals it: its typed commands whole.
    fn answer(ts: &str, summary: &str, payloads: &[(&str, &str)]) -> String {
        let typed: Vec<_> = payloads
            .iter()
            .map(|(name, base64)| json!({ "name": name, "analyticsType": name, "base64": base64 }))
            .collect();
        json!({
            "ts": ts, "dir": "out", "kind": "aliceResponse", "summary": summary,
            "data": { "decoded": { "event": {} }, "raw": "…", "typedPayloads": typed }
        })
        .to_string()
    }

    #[test]
    fn writes_each_animation_of_the_journals_once_for_the_led_skill() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("sessions");
        let content = root.path().join("content");
        let first = sessions.join("2026-11-05/101500_dev_aaaa1111");
        let second = sessions.join("2026-11-06/021100_dev_bbbb2222");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        // gzip magic: the format of field 15 is not known, it is kept as Yandex sent it.
        let stop = "H4sIAAAAAAAAA2NgYGBgAAAAAP//AwA=";
        let weather = "H4sIAAAAAAAAA2NkYGBkAAAAAP//AwA=";
        let kept = "H4sIAAAAAAAAA2NmYGBmAAAAAP//AwA=";
        std::fs::write(
            first.join("events.jsonl"),
            [
                json!({"ts": "2026-11-05T10:15:00.000Z", "dir": "in", "kind": "voiceInput"}).to_string(),
                answer("2026-11-05T10:15:02.120Z", "«стоп»", &[("draw_animation_directive", stop), ("timer", "AAA=")]),
                "{broken draw_animation_directive typedPayloads".into(),
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(
            second.join("events.jsonl"),
            [
                // The same animation again, in another session: written once.
                answer("2026-11-06T02:11:00.331Z", "«стоп» again", &[("draw_animation_directive", stop)]),
                answer("2026-11-06T02:12:30.000Z", "«какая погода»", &[("draw_animation_directive", weather)]),
                answer("2026-11-06T02:13:00.000Z", "kept", &[("draw_animation_directive", kept)]),
            ]
            .join("\n"),
        )
        .unwrap();
        // One is in the folder already, under the name someone gave it.
        std::fs::create_dir_all(content.join("animations")).unwrap();
        std::fs::write(content.join("animations/thinking.b64"), format!("{kept}\n")).unwrap();

        let target = content.join("animations");
        let mut told = Vec::new();
        let written = extract(&sessions, &target, &mut |animation| told.push(animation.clone())).unwrap();
        let stop_name = format!("20261105101502_{}", &sha256_hex(stop)[..6]);
        let weather_name = format!("20261106021230_{}", &sha256_hex(weather)[..6]);
        assert_eq!(
            written,
            [
                Extracted { name: stop_name.clone(), summary: "«стоп»".into() },
                Extracted { name: weather_name.clone(), summary: "«какая погода»".into() },
            ]
        );
        assert_eq!(told, written);
        assert_eq!(std::fs::read_to_string(target.join(format!("{stop_name}.b64"))).unwrap(), format!("{stop}\n"));

        // The LED skill plays them as they are.
        let led = LedSkill::new(target.clone());
        let mut names = vec![stop_name.clone(), weather_name, "thinking".to_string()];
        names.sort();
        assert_eq!(led.names(), names);
        let directive = led.animation(&stop_name).unwrap();
        assert_eq!(directive.name, "draw_animation_directive");
        assert_eq!(
            directive.typed_payload.as_deref(),
            Some(&base64::Engine::decode(&base64::engine::general_purpose::STANDARD, stop).unwrap()[..])
        );

        // Nothing new the second time.
        assert!(extract(&sessions, &target, &mut |_| {}).unwrap().is_empty());
    }
}
