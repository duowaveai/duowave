//! A melody from a `.mid` file (docs/plans/midi.md, «Мелодии из файла»): the panel hands the file over, the gateway
//! reads it with midly and answers with its tracks — a name, whether it is drums, how many notes, and the track's
//! melody: its top voice, of notes struck together the highest. Times are in beats, so a page can play it at any
//! tempo. The file itself is kept nowhere.
use serde::Serialize;

/// A file at most this big, tracks and events at most this many (docs/plans/midi.md, «Лимиты»).
pub const FILE_MAX: usize = 256 * 1024;
pub const TRACKS_MAX: usize = 64;
pub const EVENTS_MAX: usize = 20_000;

/// A note of a track: the note, its force, when it starts and how long it sounds, in beats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FileNote {
    pub note: u8,
    pub velocity: u8,
    pub start: f64,
    pub length: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrackView {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// MIDI channel 10: drums, no melody to learn.
    pub drums: bool,
    pub notes: usize,
    /// The top voice: `[note, start, length]`, in beats, at most `wire::MELODY_NOTES`.
    pub melody: Vec<[f64; 3]>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SongView {
    /// Beats a minute, from the file's first tempo; 120 without one.
    pub tempo: f64,
    /// The time signature, as 3/4 is `[3, 4]`; 4/4 without one.
    pub meter: [u8; 2],
    /// Tracks that have notes.
    pub tracks: Vec<TrackView>,
}

/// The top voice of a track: of notes that start together the highest, each sounding until the next one starts.
pub fn melody_of(notes: &[FileNote]) -> Vec<[f64; 3]> {
    const TOGETHER: f64 = 1.0 / 32.0;
    let mut sorted: Vec<FileNote> = notes.to_vec();
    sorted.sort_by(|a, b| a.start.total_cmp(&b.start).then(b.note.cmp(&a.note)));
    let mut top: Vec<FileNote> = Vec::new();
    for note in sorted {
        match top.last_mut() {
            Some(last) if (note.start - last.start).abs() < TOGETHER => {
                if note.note > last.note {
                    *last = note;
                }
            }
            _ => top.push(note),
        }
    }
    let round = |beats: f64| (beats * 1000.0).round() / 1000.0;
    let mut melody = Vec::with_capacity(top.len().min(super::wire::MELODY_NOTES));
    for (i, note) in top.iter().enumerate().take(super::wire::MELODY_NOTES) {
        let until = top.get(i + 1).map_or(note.start + note.length, |next| next.start.min(note.start + note.length));
        melody.push([note.note as f64, round(note.start), round((until - note.start).max(0.05))]);
    }
    melody
}

/// The file's tracks with their melodies, or why it does not read — in people's words.
pub fn read(bytes: &[u8]) -> Result<SongView, String> {
    if bytes.len() > FILE_MAX {
        return Err(crate::tr!("Файл больше {size} КБ — это не простая мелодия.", size = FILE_MAX / 1024));
    }
    if !bytes.starts_with(b"MThd") {
        return Err(crate::tr!("Это не MIDI-файл (.mid)."));
    }
    parse(bytes)
}

#[cfg(feature = "midi")]
fn parse(bytes: &[u8]) -> Result<SongView, String> {
    use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};

    let smf = Smf::parse(bytes).map_err(|error| crate::tr!("Файл повреждён: {error}", error = error))?;
    if smf.tracks.len() > TRACKS_MAX {
        return Err(crate::tr!("В файле больше {max} дорожек.", max = TRACKS_MAX));
    }
    let events: usize = smf.tracks.iter().map(Vec::len).sum();
    if events > EVENTS_MAX {
        return Err(crate::tr!("В файле больше {max} событий — это не простая мелодия.", max = EVENTS_MAX));
    }
    let mut tempo_us: Option<u32> = None;
    let mut meter: Option<[u8; 2]> = None;
    for track in &smf.tracks {
        for event in track {
            match event.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(us)) if tempo_us.is_none() => tempo_us = Some(us.as_int()),
                TrackEventKind::Meta(MetaMessage::TimeSignature(top, power, _, _)) if meter.is_none() => {
                    meter = Some([top, 1u8.checked_shl(power as u32).unwrap_or(4)])
                }
                _ => {}
            }
        }
    }
    let tempo = tempo_us.filter(|us| *us > 0).map_or(120.0, |us| 60_000_000.0 / us as f64);
    // Ticks to beats: by the quarter note, or — a file timed in frames — by seconds at the file's tempo.
    let beats_per_tick = match smf.header.timing {
        Timing::Metrical(ppq) => 1.0 / ppq.as_int().max(1) as f64,
        Timing::Timecode(fps, sub) => tempo / 60.0 / (fps.as_f32() as f64 * sub.max(1) as f64),
    };
    let mut tracks = Vec::new();
    for (index, track) in smf.tracks.iter().enumerate() {
        let mut name = None;
        let mut tick: u64 = 0;
        let mut sounding: std::collections::HashMap<(u8, u8), (u64, u8)> = Default::default();
        let mut notes = Vec::new();
        let mut channels = std::collections::BTreeSet::new();
        for event in track {
            tick += event.delta.as_int() as u64;
            match event.kind {
                TrackEventKind::Meta(MetaMessage::TrackName(raw)) if name.is_none() => {
                    let text = String::from_utf8_lossy(raw).trim().to_string();
                    name = (!text.is_empty()).then(|| text.chars().take(60).collect());
                }
                TrackEventKind::Midi { channel, message } => {
                    let channel = channel.as_int();
                    let (key, on) = match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => (key.as_int(), Some(vel.as_int())),
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => (key.as_int(), None),
                        _ => continue,
                    };
                    channels.insert(channel);
                    if let Some((start, velocity)) = sounding.remove(&(channel, key)) {
                        notes.push(note_of(key, velocity, start, tick, beats_per_tick));
                    }
                    if let Some(velocity) = on {
                        sounding.insert((channel, key), (tick, velocity));
                    }
                }
                _ => {}
            }
        }
        // A note never let go sounds a beat.
        for ((_, key), (start, velocity)) in sounding {
            let end = start + (1.0 / beats_per_tick) as u64;
            notes.push(note_of(key, velocity, start, end, beats_per_tick));
        }
        if notes.is_empty() {
            continue;
        }
        let drums = !channels.is_empty() && channels.iter().all(|channel| *channel == 9);
        let melody = if drums { Vec::new() } else { melody_of(&notes) };
        tracks.push(TrackView { index, name, drums, notes: notes.len(), melody });
    }
    if tracks.is_empty() {
        return Err(crate::tr!("В файле нет нот."));
    }
    Ok(SongView { tempo: (tempo * 10.0).round() / 10.0, meter: meter.unwrap_or([4, 4]), tracks })
}

#[cfg(feature = "midi")]
fn note_of(note: u8, velocity: u8, start: u64, end: u64, beats_per_tick: f64) -> FileNote {
    FileNote {
        note,
        velocity,
        start: start as f64 * beats_per_tick,
        length: end.saturating_sub(start) as f64 * beats_per_tick,
    }
}

#[cfg(not(feature = "midi"))]
fn parse(_bytes: &[u8]) -> Result<SongView, String> {
    Err(super::ports::not_in_build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_top_voice_of_notes_struck_together() {
        let note = |note, start, length| FileNote { note, velocity: 90, start, length };
        // A chord C-E-G, then D held across the next note.
        let melody = melody_of(&[
            note(60, 0.0, 1.0),
            note(67, 0.0, 1.0),
            note(64, 0.0, 1.0),
            note(62, 1.0, 4.0),
            note(65, 2.0, 1.0),
        ]);
        assert_eq!(melody, vec![[67.0, 0.0, 1.0], [62.0, 1.0, 1.0], [65.0, 2.0, 1.0]]);
    }

    #[test]
    fn not_a_midi_file_says_so() {
        assert!(read(b"RIFF....").unwrap_err().contains("не MIDI"));
        assert!(read(&vec![0u8; FILE_MAX + 1]).unwrap_err().contains("КБ"));
    }

    /// A file of format 1: a tempo track and a melody of three notes, one of them by a note-on of force 0, with running
    /// status; and a drum track.
    #[cfg(feature = "midi")]
    fn file() -> Vec<u8> {
        fn track(events: &[u8]) -> Vec<u8> {
            let mut out = b"MTrk".to_vec();
            out.extend_from_slice(&(events.len() as u32).to_be_bytes());
            out.extend_from_slice(events);
            out
        }
        let mut file = b"MThd".to_vec();
        file.extend_from_slice(&[0, 0, 0, 6, 0, 1, 0, 3, 0, 96]); // format 1, 3 tracks, 96 ticks a beat
                                                                  // 100 bpm (600000 µs), 3/4.
        file.extend(track(&[0, 0xFF, 0x51, 3, 0x09, 0x27, 0xC0, 0, 0xFF, 0x58, 4, 3, 2, 24, 8, 0, 0xFF, 0x2F, 0]));
        file.extend(track(&[
            0, 0xFF, 0x03, 5, b'R', b'i', b'g', b'h', b't', // the name
            0, 0x90, 60, 90, // C on
            96, 60, 0, // C off by force 0, running status
            0, 62, 80, // D on, running status
            48, 0x80, 62, 0, // D off after half a beat
            0, 0x90, 64, 70, // E, never let go
            0, 0xFF, 0x2F, 0,
        ]));
        file.extend(track(&[0, 0x99, 36, 100, 96, 0x89, 36, 0, 0, 0xFF, 0x2F, 0]));
        file
    }

    #[cfg(feature = "midi")]
    #[test]
    fn a_file_reads_into_tracks_and_a_melody() {
        let song = read(&file()).unwrap();
        assert_eq!(song.tempo, 100.0);
        assert_eq!(song.meter, [3, 4]);
        assert_eq!(song.tracks.len(), 2, "the tempo track has no notes");
        let right = &song.tracks[0];
        assert_eq!(right.name.as_deref(), Some("Right"));
        assert_eq!(right.notes, 3);
        assert_eq!(right.melody, vec![[60.0, 0.0, 1.0], [62.0, 1.0, 0.5], [64.0, 1.5, 1.0]]);
        assert!(song.tracks[1].drums && song.tracks[1].melody.is_empty());
        // midly forgives a cut track (what it read stays), not a cut header.
        let mut broken = file();
        broken.truncate(10);
        assert!(read(&broken).unwrap_err().contains("повреждён"));
        broken = file();
        broken.truncate(30);
        assert!(read(&broken).is_err(), "only the tempo track is left: no notes");
    }
}
