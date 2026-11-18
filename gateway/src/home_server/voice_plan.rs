//! The character's voice on this Турбо, as a plan for the home's centre (clean-path check 18.11, item 40):
//! `POST /home/v1/character/plan` (serve.rs) answers how each voice model stands here for this character — whether its
//! package has a snapshot for it or a reference to take one, whether it runs here and where, whether it is installed
//! or coming (and how far), what it takes on the disk and in memory against what is free — the model the voice goes
//! with (the one the centre named, else this computer's pick), the fallback voice, what is under way, and whether the
//! character already speaks with its own voice here.
//!
//! The package need not be here yet: the plan is made from what the centre says of it (`voices`, `reference`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::characters::contributes::{Reference, Snapshot};
use crate::characters::{Character, Gender};
use crate::models::models_service::OWN_PREFIX;
use crate::platform::http::ApiError;
use crate::voice_lab::clone_engines::EngineId;
use crate::voice_lab::jobs::{JobKind, JobRunState, JobState};
use crate::voice_lab::service::EngineStatus;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PackageRef {
    pub id: String,
    #[serde(default)]
    pub version: Option<String>,
}

/// What the centre says of the character.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRequest {
    pub id: String,
    #[serde(default)]
    pub package: Option<PackageRef>,
    /// The models its package has a voice snapshot for.
    #[serde(default)]
    pub voices: Vec<String>,
    /// Its package has a reference recording to take the voice from.
    #[serde(default)]
    pub reference: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub gender: Option<String>,
    #[serde(default)]
    pub fallback_voice: Option<String>,
    /// The model the centre's owner chose: the plan's `chosen` when it can go here.
    #[serde(default)]
    pub engine: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnginePlan {
    pub id: String,
    pub title: String,
    /// snapshot, reference, none.
    pub way: &'static str,
    pub supported: bool,
    pub reason: Option<String>,
    pub state: EngineStatus,
    pub percent: Option<u32>,
    pub done_mb: Option<u64>,
    pub total_mb: u64,
    pub disk_mb: u64,
    pub free_disk_mb: u64,
    pub disk_fits: bool,
    pub ram_mb: u64,
    pub free_ram_mb: u64,
    pub ram_fits: bool,
    pub device: Option<&'static str>,
    pub recommended: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FallbackPlan {
    pub card_id: String,
    pub title: String,
    pub installed: bool,
    pub size_mb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// model — its download; voice — the voice taken for the model.
    pub stage: &'static str,
    pub engine: String,
    pub percent: u32,
    pub done_mb: Option<u64>,
    pub total_mb: Option<u64>,
    /// What it does now, in words: «Скачиваю веса модели · 603 из ~2 300 МБ · 1,2 МБ/с», «Скачивание стояло —
    /// повторяю (1 из 3) · …» (п. 66). The panel's pill and window show it under the percent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoicePlan {
    pub engines: Vec<EnginePlan>,
    pub chosen: Option<String>,
    pub fallback: Option<FallbackPlan>,
    pub progress: Option<Progress>,
    pub voice_on: bool,
    pub computer: String,
    /// The model's install for this plan failed, in words («Hugging Face не отвечает — …»): the panel says it and offers
    /// «Повторить» (п. 66–67). None — nothing failed, or it is going again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

/// How the character's voice comes for a model: its snapshot, taken from its reference, or not at all.
fn way(request: &PlanRequest, engine: &str) -> &'static str {
    if request.voices.iter().any(|voice| voice == engine) {
        "snapshot"
    } else if request.reference {
        "reference"
    } else {
        "none"
    }
}

/// What the voice lab is doing now: a model downloaded (its share of the download) or a voice taken for one.
fn progress(job: Option<&JobState>, downloads: &BTreeMap<String, u64>) -> Option<Progress> {
    let job = job.filter(|job| job.state == JobRunState::Running)?;
    let total = downloads.get(&job.engine).copied();
    Some(match job.kind {
        JobKind::Install => Progress {
            stage: "model",
            engine: job.engine.clone(),
            percent: job.percent,
            done_mb: total.map(|total| total * u64::from(job.percent.min(100)) / 100),
            total_mb: total,
            detail: Some(job.label.clone()).filter(|label| !label.is_empty()),
        },
        JobKind::Imprint => Progress {
            stage: "voice",
            engine: job.engine.clone(),
            percent: job.percent,
            done_mb: None,
            total_mb: None,
            detail: None,
        },
    })
}

/// The character as the plan sees it: the one installed here when it is that version, else made of the centre's words.
fn character_of(app: &App, request: &PlanRequest) -> Character {
    let version = request.package.as_ref().and_then(|package| package.version.clone());
    if let Some(here) = app.characters.character(&request.id) {
        if version.is_none() || version.as_deref() == Some(here.version.as_str()) {
            return here;
        }
    }
    let (package, local) = match request.id.split_once(':') {
        Some((package, local)) => (package.to_string(), local.to_string()),
        None => (request.id.clone(), request.id.clone()),
    };
    let package = request.package.as_ref().map(|package| package.id.clone()).unwrap_or(package);
    let gender = match request.gender.as_deref() {
        Some("male") => Some(Gender::Male),
        Some("female") => Some(Gender::Female),
        Some("any") => Some(Gender::Any),
        _ => None,
    };
    let voices = request
        .voices
        .iter()
        .filter(|engine| EngineId::parse(engine).is_some())
        .map(|engine| (engine.clone(), Snapshot { file: String::new(), model: None }))
        .collect();
    Character {
        id: request.id.clone(),
        local,
        package,
        version: version.unwrap_or_default(),
        license: None,
        title: request.title.clone().unwrap_or_else(|| request.id.clone()),
        description: None,
        gender,
        color: None,
        avatar: None,
        prompt: String::new(),
        tones: Vec::new(),
        samples: Vec::new(),
        fallback_voice: request.fallback_voice.clone(),
        voices,
        skipped: Vec::new(),
        reference: request.reference.then(|| Reference { file: String::new(), text: String::new() }),
        voice_origin: None,
        assets: PathBuf::new(),
    }
}

/// A model to install that does not fit on this computer's disk is refused before anything is downloaded (clean-path
/// check 18.11, item 40); one installed or coming has its room.
pub async fn check_room(app: &App, engine: &str) -> Result<(), ApiError> {
    let Some(id) = EngineId::parse(engine) else { return Ok(()) };
    let Some(card) = app.voice_lab.engine_cards().await.into_iter().find(|card| card.id == id) else { return Ok(()) };
    if card.status != EngineStatus::Missing {
        return Ok(());
    }
    let (_, disk) = app.voice_lab.engine_size(id).await;
    let free = app.voice_lab.free_disk_mb();
    if disk > free {
        return Err(crate::home::character::no_room(card.title, disk, free));
    }
    Ok(())
}

/// The plan of the character's voice on this computer.
pub async fn plan(app: &App, request: &PlanRequest) -> VoicePlan {
    let character = character_of(app, request);
    let lab = &app.voice_lab;
    let (free_disk, free_ram) = (lab.free_disk_mb(), lab.free_memory_mb().await);
    let job = lab.job();
    let running = job.as_ref().filter(|job| job.state == JobRunState::Running);
    let mut downloads = BTreeMap::new();
    let mut engines = Vec::new();
    for card in lab.engine_cards().await {
        let id = card.id.as_str().to_string();
        let (download, disk) = lab.engine_size(card.id).await;
        downloads.insert(id.clone(), download);
        let installing = running.filter(|job| job.kind == JobKind::Install && job.engine == id);
        let percent = installing.map(|job| job.percent);
        let supported = card.status != EngineStatus::Unsupported;
        engines.push(EnginePlan {
            way: way(request, &id),
            title: card.title.to_string(),
            supported,
            reason: if supported { None } else { card.reason.clone() },
            state: card.status,
            percent,
            done_mb: percent.map(|percent| download * u64::from(percent.min(100)) / 100),
            total_mb: download,
            disk_mb: disk,
            free_disk_mb: free_disk,
            // Installed or coming: its room is taken already.
            disk_fits: card.status != EngineStatus::Missing || free_disk >= disk,
            ram_mb: card.need_mb.serve,
            free_ram_mb: free_ram,
            ram_fits: free_ram >= card.need_mb.serve,
            device: card.device.map(|device| device.as_str()),
            recommended: card.recommended,
            id,
        });
    }
    let wanted = request.engine.as_deref().filter(|engine| EngineId::parse(engine).is_some());
    let (chosen, fallback) = app.characters.turbo_choice(&character, wanted).await;
    let fallback = fallback.map(|fallback| FallbackPlan {
        size_mb: app.models.voice_card_mb(&fallback.card_id),
        card_id: fallback.card_id,
        title: fallback.title,
        installed: fallback.installed,
    });
    let theirs = format!("{OWN_PREFIX}c-{}-{}-", character.package, character.local);
    let voice_on = app.models.chosen_voice().is_some_and(|voice| voice.model_id.starts_with(&theirs));
    // The last job was this plan's model's install, and it failed: said, not hidden behind «ставится».
    let failed = job
        .as_ref()
        .filter(|job| job.kind == JobKind::Install && job.state == JobRunState::Error)
        .filter(|job| chosen.as_deref() == Some(job.engine.as_str()))
        .and_then(|job| job.error.clone());
    VoicePlan {
        engines,
        chosen,
        fallback,
        progress: progress(job.as_ref(), &downloads),
        voice_on,
        computer: app.identity.gateway().map(|me| me.name).unwrap_or_default(),
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(kind: JobKind, engine: &str, state: JobRunState, percent: u32) -> JobState {
        JobState {
            id: "j1".into(),
            kind,
            engine: engine.into(),
            state,
            stage: "weights".into(),
            label: String::new(),
            percent,
            log: Vec::new(),
            memory_mb: None,
            peak_mb: None,
            limit_mb: None,
            error: None,
            reason: None,
            started_at: "2026-11-18T10:00:00Z".into(),
            finished_at: None,
            result: None,
        }
    }

    #[test]
    fn a_models_way_is_its_snapshot_else_the_reference_else_none() {
        let request = PlanRequest { voices: vec!["omnivoice".into()], reference: true, ..PlanRequest::default() };
        assert_eq!(way(&request, "omnivoice"), "snapshot");
        assert_eq!(way(&request, "voxcpm"), "reference");
        let bare = PlanRequest { voices: vec!["omnivoice".into()], ..PlanRequest::default() };
        assert_eq!(way(&bare, "cosyvoice3"), "none");
    }

    #[test]
    fn the_progress_is_the_models_download_or_the_voice_taken_and_nothing_once_done() {
        let downloads = BTreeMap::from([("cosyvoice3".to_string(), 4_000)]);
        let mut installing = job(JobKind::Install, "cosyvoice3", JobRunState::Running, 25);
        installing.label = "Скачиваю веса модели · 603 из ~2\u{a0}300 МБ · 0 КБ/с".into();
        assert_eq!(
            progress(Some(&installing), &downloads),
            Some(Progress {
                stage: "model",
                engine: "cosyvoice3".into(),
                percent: 25,
                done_mb: Some(1_000),
                total_mb: Some(4_000),
                // The job's own words go along: megabytes and speed, not only the percent (п. 66).
                detail: Some("Скачиваю веса модели · 603 из ~2\u{a0}300 МБ · 0 КБ/с".into()),
            })
        );
        let imprinting = job(JobKind::Imprint, "omnivoice", JobRunState::Running, 55);
        let voice = progress(Some(&imprinting), &downloads).unwrap();
        assert_eq!(
            (voice.stage, voice.engine.as_str(), voice.percent, voice.done_mb),
            ("voice", "omnivoice", 55, None)
        );
        assert_eq!(progress(Some(&job(JobKind::Install, "cosyvoice3", JobRunState::Done, 100)), &downloads), None);
        assert_eq!(progress(None, &downloads), None);
    }
}
