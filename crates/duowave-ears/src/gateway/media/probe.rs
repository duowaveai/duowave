//! How long a sound or a video lasts, and a video's cover frame (docs/plans/feed.md, «Один движок показа»).
//!
//! The length is read from the file's own header, never by playing it: MP4, MOV and M4A (`mvhd`), MP3 (its Xing or
//! VBRI frame count, else its bit rate), WAV, FLAC, Ogg (Opus, Vorbis), WebM and Matroska, AVI. A few kilobytes at most
//! are read — an Ogg's last 64 KB — so a folder's page of songs is measured at once. A format it does not know has
//! no length; the player still knows it once it plays.
//!
//! The cover is one frame a tenth of the way in (at most at 3 s — past a black start), made by `ffmpeg` when the
//! computer has it (`FFMPEG`, else on the PATH), else on a Mac by Quick Look (`qlmanage`), which knows MP4 and MOV.
//! What was made is kept beside the photos made small (`folder-previews`) under the file's place, size and time.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::kind::Sniffed;
use crate::utils::preview::{self, SIDE_MOST};

/// A cover is made in this time or not at all.
const COVER_WAIT: Duration = Duration::from_secs(20);

/// How long the file plays, in seconds (to a tenth); none when its header does not say.
pub fn duration(path: &Path, sniffed: Sniffed) -> Option<f64> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let seconds = match sniffed.mime {
        "video/mp4" | "video/quicktime" | "video/3gpp" | "audio/mp4" => mp4(&mut file, len),
        "audio/mpeg" => mp3(&mut file, len),
        "audio/wav" => wav(&mut file, len),
        "audio/flac" => flac(&mut file),
        "audio/ogg" | "video/ogg" => ogg(&mut file, len),
        "video/webm" | "audio/webm" | "video/x-matroska" | "audio/x-matroska" => matroska(&mut file),
        "video/x-msvideo" => avi(&mut file),
        _ => None,
    }?;
    (seconds.is_finite() && seconds > 0.0 && seconds < 1e7).then(|| (seconds * 10.0).round() / 10.0)
}

fn read_at(file: &mut File, at: u64, buf: &mut [u8]) -> Option<usize> {
    file.seek(SeekFrom::Start(at)).ok()?;
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return None,
        }
    }
    Some(filled)
}

fn exact<const N: usize>(file: &mut File, at: u64) -> Option<[u8; N]> {
    let mut buf = [0u8; N];
    (read_at(file, at, &mut buf)? == N).then_some(buf)
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn le32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// An ISO media box at `at`: its size, type and header length; none past `limit`.
fn iso_box(file: &mut File, at: u64, limit: u64) -> Option<(u64, [u8; 4], u64)> {
    let head: [u8; 8] = exact(file, at)?;
    let kind = [head[4], head[5], head[6], head[7]];
    let (size, header) = match be32(&head) {
        1 => (u64::from_be_bytes(exact(file, at + 8)?), 16),
        0 => (limit - at, 8),
        size => (u64::from(size), 8),
    };
    (size >= header && at + size <= limit).then_some((size, kind, header))
}

/// MP4, MOV, M4A: the movie header's duration by its time scale.
fn mp4(file: &mut File, len: u64) -> Option<f64> {
    let mut at = 0;
    while at + 8 <= len {
        let (size, kind, header) = iso_box(file, at, len)?;
        if &kind == b"moov" {
            let end = at + size;
            let mut child = at + header;
            while child + 8 <= end {
                let (child_size, child_kind, child_header) = iso_box(file, child, end)?;
                if &child_kind == b"mvhd" {
                    let body: [u8; 32] = exact(file, child + child_header)?;
                    let (scale, duration) = if body[0] == 1 {
                        (be32(&body[20..24]), u64::from_be_bytes(body[24..32].try_into().ok()?))
                    } else {
                        (be32(&body[12..16]), u64::from(be32(&body[16..20])))
                    };
                    return (scale > 0 && duration != u64::from(u32::MAX)).then(|| duration as f64 / f64::from(scale));
                }
                child += child_size;
            }
            return None;
        }
        at += size;
    }
    None
}

/// MP3: the first frame's Xing/Info or VBRI count of frames; without one, a steady bit rate over the file.
fn mp3(file: &mut File, len: u64) -> Option<f64> {
    let tag: [u8; 10] = exact(file, 0)?;
    let start = if &tag[..3] == b"ID3" {
        let size = tag[6..10].iter().fold(0u64, |size, byte| (size << 7) | u64::from(byte & 0x7F));
        10 + size + if tag[5] & 0x10 != 0 { 10 } else { 0 }
    } else {
        0
    };
    let mut buf = vec![0u8; 4096];
    let filled = read_at(file, start, &mut buf)?;
    let buf = &buf[..filled];
    let at = (0..buf.len().saturating_sub(4)).find(|&i| {
        buf[i] == 0xFF && buf[i + 1] & 0xE0 == 0xE0 && (buf[i + 1] >> 1) & 3 != 0 && (buf[i + 1] >> 3) & 3 != 1 && {
            let bitrate = buf[i + 2] >> 4;
            bitrate != 0 && bitrate != 15 && (buf[i + 2] >> 2) & 3 != 3
        }
    })?;
    let (b1, b2, b3) = (buf[at + 1], buf[at + 2], buf[at + 3]);
    let version = (b1 >> 3) & 3; // 3: MPEG 1, 2: MPEG 2, 0: MPEG 2.5
    let layer = 4 - ((b1 >> 1) & 3); // 1, 2, 3
    let rate = [44100.0, 48000.0, 32000.0][usize::from((b2 >> 2) & 3)]
        / match version {
            3 => 1.0,
            2 => 2.0,
            _ => 4.0,
        };
    let per_frame = match (layer, version) {
        (1, _) => 384.0,
        (2, _) | (3, 3) => 1152.0,
        _ => 576.0,
    };
    let mono = b3 >> 6 == 3;
    let side = match (version == 3, mono) {
        (true, false) => 32,
        (true, true) | (false, false) => 17,
        (false, true) => 9,
    };
    // A count of frames `offset` bytes into the frame.
    let count = |offset: usize| buf.get(at + offset..at + offset + 4).map(be32).filter(|count| *count > 0);
    if layer == 3 {
        // Xing (or Info): after the side information — its flags, then the frames when the first flag is up.
        let tag = buf.get(at + 4 + side..at + 8 + side);
        let flags = buf.get(at + 8 + side..at + 12 + side).map_or(0, be32);
        if (tag == Some(b"Xing") || tag == Some(b"Info")) && flags & 1 != 0 {
            if let Some(frames) = count(12 + side) {
                return Some(f64::from(frames) * per_frame / rate);
            }
        }
        // VBRI: 32 bytes after the frame's header; its frames 14 bytes into it.
        if buf.get(at + 36..at + 40) == Some(b"VBRI") {
            if let Some(frames) = count(36 + 14) {
                return Some(f64::from(frames) * per_frame / rate);
            }
        }
    }
    const V1: [[u16; 15]; 3] = [
        [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
        [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
    ];
    const V2: [[u16; 15]; 2] = [
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
    ];
    let index = usize::from(b2 >> 4);
    let kbps = if version == 3 { V1[usize::from(layer - 1)][index] } else { V2[usize::from(layer > 1)][index] };
    let audio = len.saturating_sub(start + at as u64);
    (kbps > 0).then(|| audio as f64 * 8.0 / (f64::from(kbps) * 1000.0))
}

/// WAV: its data's bytes by the format's bytes a second.
fn wav(file: &mut File, len: u64) -> Option<f64> {
    let mut at = 12;
    let mut per_second = None;
    while at + 8 <= len {
        let chunk: [u8; 8] = exact(file, at)?;
        let size = u64::from(le32(&chunk[4..8]));
        match &chunk[..4] {
            b"fmt " => {
                let format: [u8; 12] = exact(file, at + 8)?;
                per_second = Some(le32(&format[8..12]));
            }
            b"data" => {
                let data = if size == u64::from(u32::MAX) || at + 8 + size > len { len - at - 8 } else { size };
                return per_second.filter(|rate| *rate > 0).map(|rate| data as f64 / f64::from(rate));
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    None
}

/// The rate and the samples of a FLAC STREAMINFO block (its 34 bytes).
fn streaminfo(info: &[u8]) -> Option<f64> {
    let rate = (u32::from(info[10]) << 12) | (u32::from(info[11]) << 4) | (u32::from(info[12]) >> 4);
    let samples = (u64::from(info[13] & 0x0F) << 32) | u64::from(be32(&info[14..18]));
    (rate > 0 && samples > 0).then(|| samples as f64 / f64::from(rate))
}

/// FLAC: its first metadata block is STREAMINFO.
fn flac(file: &mut File) -> Option<f64> {
    let head: [u8; 42] = exact(file, 0)?;
    (&head[..4] == b"fLaC" && head[4] & 0x7F == 0).then(|| streaminfo(&head[8..42]))?
}

/// Ogg: the last page's granule position by the codec's rate (Opus always counts 48 kHz, less its pre-skip).
fn ogg(file: &mut File, len: u64) -> Option<f64> {
    let mut first = [0u8; 256];
    let filled = read_at(file, 0, &mut first)?;
    let first = &first[..filled];
    // The first page's header (27 bytes), its segment table, then the codec's first packet.
    let segments = usize::from(*first.get(26)?);
    let packet = first.get(27 + segments..)?;
    let (rate, skip) = if packet.starts_with(b"OpusHead") {
        (48000.0, f64::from(u16::from_le_bytes(packet.get(10..12)?.try_into().ok()?)))
    } else if packet.starts_with(b"\x01vorbis") {
        (f64::from(le32(packet.get(12..16)?)), 0.0)
    } else if packet.starts_with(b"\x7fFLAC") {
        // FLAC in Ogg: 9 bytes of the mapping, «fLaC», the STREAMINFO block's header, the block.
        let info = packet.get(17..17 + 34)?;
        (f64::from((u32::from(info[10]) << 12) | (u32::from(info[11]) << 4) | (u32::from(info[12]) >> 4)), 0.0)
    } else {
        return None;
    };
    let granule = last_granule(file, len)?;
    (rate > 0.0).then(|| (granule as f64 - skip).max(0.0) / rate)
}

fn last_granule(file: &mut File, len: u64) -> Option<u64> {
    let from = len.saturating_sub(64 * 1024);
    let mut buf = vec![0u8; (len - from) as usize];
    let filled = read_at(file, from, &mut buf)?;
    let buf = &buf[..filled];
    let at = buf.windows(4).rposition(|window| window == b"OggS")?;
    let granule = buf.get(at + 6..at + 14)?;
    let granule = i64::from_le_bytes(granule.try_into().ok()?);
    (granule > 0).then_some(granule as u64)
}

/// An EBML element's id (its length marker kept) at `at`.
fn ebml_id(buf: &[u8], at: usize) -> Option<(u32, usize)> {
    let first = *buf.get(at)?;
    let len = first.leading_zeros() as usize + 1;
    (len <= 4).then_some(())?;
    let bytes = buf.get(at..at + len)?;
    Some((bytes.iter().fold(0u32, |id, byte| (id << 8) | u32::from(*byte)), len))
}

/// An EBML element's size at `at`; none — unknown.
fn ebml_size(buf: &[u8], at: usize) -> Option<(Option<u64>, usize)> {
    let first = *buf.get(at)?;
    let len = first.leading_zeros() as usize + 1;
    (len <= 8).then_some(())?;
    let bytes = buf.get(at..at + len)?;
    // The length marker off: an eight-byte size keeps none of its first byte.
    let mut value = u64::from(first & (0xFFu16 >> len) as u8);
    for byte in &bytes[1..] {
        value = (value << 8) | u64::from(*byte);
    }
    let unknown = value == (1u64 << (7 * len)) - 1;
    Some(((!unknown).then_some(value), len))
}

/// WebM and Matroska: the segment's Info — its Duration in units of its TimecodeScale (a millisecond by default).
fn matroska(file: &mut File) -> Option<f64> {
    let mut buf = vec![0u8; 64 * 1024];
    let filled = read_at(file, 0, &mut buf)?;
    let buf = &buf[..filled];
    let mut at = 0;
    // The EBML header, then the segment.
    let (id, id_len) = ebml_id(buf, at)?;
    (id == 0x1A45_DFA3).then_some(())?;
    let (size, size_len) = ebml_size(buf, at + id_len)?;
    at += id_len + size_len + size? as usize;
    let (id, id_len) = ebml_id(buf, at)?;
    (id == 0x1853_8067).then_some(())?;
    let (_, size_len) = ebml_size(buf, at + id_len)?;
    at += id_len + size_len;
    while at < buf.len() {
        let (id, id_len) = ebml_id(buf, at)?;
        let (size, size_len) = ebml_size(buf, at + id_len)?;
        let body = at + id_len + size_len;
        match id {
            // Info.
            0x1549_A966 => {
                let end = body + size? as usize;
                let (mut scale, mut duration) = (1_000_000.0, None);
                let mut inner = body;
                while inner < end.min(buf.len()) {
                    let (id, id_len) = ebml_id(buf, inner)?;
                    let (size, size_len) = ebml_size(buf, inner + id_len)?;
                    let size = size? as usize;
                    let value = buf.get(inner + id_len + size_len..inner + id_len + size_len + size)?;
                    match (id, size) {
                        (0x2AD7B1, 1..=8) => {
                            scale = value.iter().fold(0u64, |n, byte| (n << 8) | u64::from(*byte)) as f64
                        }
                        (0x4489, 4) => duration = Some(f64::from(f32::from_be_bytes(value.try_into().ok()?))),
                        (0x4489, 8) => duration = Some(f64::from_be_bytes(value.try_into().ok()?)),
                        _ => {}
                    }
                    inner += id_len + size_len + size;
                }
                return duration.map(|duration| duration * scale / 1e9);
            }
            // A cluster: the frames began, and no Info before them.
            0x1F43_B675 => return None,
            _ => at = body + size? as usize,
        }
    }
    None
}

/// AVI: the main header's frames by its microseconds a frame.
fn avi(file: &mut File) -> Option<f64> {
    let mut head = [0u8; 512];
    let filled = read_at(file, 0, &mut head)?;
    let at = head[..filled].windows(4).position(|window| window == b"avih")?;
    let main = head.get(at + 8..at + 8 + 20)?;
    let (per_frame, frames) = (le32(&main[0..4]), le32(&main[16..20]));
    (per_frame > 0 && frames > 0).then(|| f64::from(frames) * f64::from(per_frame) / 1e6)
}

/// A JPEG of the video's cover frame, its longer side `side` at most; from the kept ones when it was made before.
pub fn cover(source: &Path, sniffed: Sniffed, side: u32, kept: &Path) -> Result<Vec<u8>, String> {
    let side = side.clamp(64, SIDE_MOST);
    let meta = std::fs::metadata(source).map_err(|error| error.to_string())?;
    let modified = meta.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok());
    let key = format!("cover|{}|{}|{:?}|{side}", source.display(), meta.len(), modified);
    let file = kept.join(format!("{}.jpg", hex::encode(&Sha256::digest(key.as_bytes())[..16])));
    if let Ok(bytes) = std::fs::read(&file) {
        return Ok(bytes);
    }
    // A tenth of the way in, at most 3 s: past a fade from black, still the opening.
    let at = duration(source, sniffed).map_or(1.0, |seconds| (seconds / 10.0).min(3.0));
    let bytes = match ffmpeg() {
        Some(ffmpeg) => by_ffmpeg(&ffmpeg, source, at, side)
            .or_else(|_| by_ffmpeg(&ffmpeg, source, 0.0, side))
            .or_else(|error| by_quick_look(source, side, kept).map_err(|_| error))?,
        None => by_quick_look(source, side, kept)?,
    };
    if std::fs::create_dir_all(kept).is_ok() && std::fs::write(&file, &bytes).is_ok() {
        preview::trim(kept);
    }
    Ok(bytes)
}

/// The computer's ffmpeg: `FFMPEG`, else on the PATH, else where Homebrew puts it (an app started by the system has
/// no shell's PATH).
fn ffmpeg() -> Option<PathBuf> {
    if let Some(given) = std::env::var_os("FFMPEG").map(PathBuf::from).filter(|path| path.is_file()) {
        return Some(given);
    }
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from))
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Runs a program to its end within `COVER_WAIT`; its output, or why not.
fn run(command: &mut Command) -> Result<Vec<u8>, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let mut stdout = child.stdout.take().ok_or("no output")?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let began = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if began.elapsed() > COVER_WAIT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(crate::tr!("кадр видео не успел сделаться"));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let out = reader.join().unwrap_or_default();
    if status.success() {
        Ok(out)
    } else {
        Err(crate::tr!("кадр видео не сделался"))
    }
}

fn by_ffmpeg(ffmpeg: &Path, source: &Path, at: f64, side: u32) -> Result<Vec<u8>, String> {
    let scale = format!("scale='min({side},iw)':'min({side},ih)':force_original_aspect_ratio=decrease");
    let out = run(Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &format!("{at:.2}"), "-i"])
        .arg(source)
        .args(["-frames:v", "1", "-vf", &scale, "-f", "image2pipe", "-vcodec", "mjpeg", "-q:v", "4", "-"]))?;
    if out.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Ok(out)
    } else {
        Err(crate::tr!("кадр видео не сделался"))
    }
}

/// A Mac's Quick Look thumbnail (PNG), made a JPEG.
fn by_quick_look(source: &Path, side: u32, kept: &Path) -> Result<Vec<u8>, String> {
    if !cfg!(target_os = "macos") {
        return Err(crate::tr!("кадр видео здесь не сделать: нужен ffmpeg"));
    }
    let out = kept.join(format!(".cover-{}", crate::utils::ids::random_hex(6)));
    std::fs::create_dir_all(&out).map_err(|error| error.to_string())?;
    let made = run(Command::new("qlmanage").args(["-t", "-s", &side.to_string(), "-o"]).arg(&out).arg(source));
    let png = std::fs::read_dir(&out)
        .ok()
        .and_then(|mut entries| entries.find_map(|entry| entry.ok().map(|entry| entry.path())))
        .and_then(|path| std::fs::read(path).ok());
    let _ = std::fs::remove_dir_all(&out);
    made?;
    let picture = image::load_from_memory(&png.ok_or_else(|| crate::tr!("кадр видео не сделался"))?)
        .map_err(|_| crate::tr!("кадр видео не сделался"))?;
    let small = picture.thumbnail(side, side).to_rgb8();
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 82)
        .encode_image(&small)
        .map_err(|error| error.to_string())?;
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::media::kind::sniff;

    fn measured(name: &str, bytes: &[u8]) -> Option<f64> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        duration(&path, sniff(bytes, name)?)
    }

    fn iso_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((8 + body.len()) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn reads_an_mp4s_length_wherever_its_movie_header_is() {
        // Version 0: created, modified, a time scale of 1000, 41 500 units.
        let mut mvhd = vec![0u8; 4 + 8];
        mvhd.extend_from_slice(&1000u32.to_be_bytes());
        mvhd.extend_from_slice(&41_500u32.to_be_bytes());
        mvhd.resize(100, 0);
        let ftyp = iso_box(b"ftyp", b"isomiso2avc1mp41");
        let moov = iso_box(b"moov", &iso_box(b"mvhd", &mvhd));
        let mdat = iso_box(b"mdat", &vec![7u8; 5000]);
        // Fast start: the header first; a camera's file: after the frames.
        assert_eq!(measured("a.mp4", &[ftyp.clone(), moov.clone(), mdat.clone()].concat()), Some(41.5));
        assert_eq!(measured("IMG.MOV", &[ftyp, mdat, moov].concat()), Some(41.5));
    }

    #[test]
    fn reads_an_mp3_by_its_frame_count_or_its_bit_rate() {
        // MPEG 1 layer III, 128 kbps, 44.1 kHz, stereo; a Xing frame with 1000 frames.
        let mut frame = vec![0xFF, 0xFB, 0x90, 0x00];
        frame.resize(4 + 32, 0);
        frame.extend_from_slice(b"Xing");
        frame.extend_from_slice(&1u32.to_be_bytes());
        frame.extend_from_slice(&1000u32.to_be_bytes());
        frame.resize(417, 0);
        let mut tagged = b"ID3\x04\x00\x00\x00\x00\x00\x0a".to_vec();
        tagged.extend_from_slice(&[0u8; 10]);
        tagged.extend_from_slice(&frame);
        assert_eq!(measured("песня.mp3", &tagged), Some((1000.0 * 1152.0 / 44100.0 * 10.0f64).round() / 10.0));
        // No Xing: 128 kbps over 160 000 bytes — ten seconds.
        let mut plain = vec![0xFF, 0xFB, 0x90, 0x00];
        plain.resize(160_000, 0);
        assert_eq!(measured("плоская.mp3", &plain), Some(10.0));
    }

    #[test]
    fn reads_wav_flac_ogg_and_webm() {
        // WAV: 16 kHz mono 16 bit — 32 000 bytes a second; 48 000 bytes of data.
        let mut wav = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data".to_vec();
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.resize(wav.len() + 48_000, 0);
        assert_eq!(measured("голос.wav", &wav), Some(1.5));
        // FLAC: 44.1 kHz, 441 000 samples.
        let mut flac = b"fLaC\x00\x00\x00\x22".to_vec();
        let mut info = [0u8; 34];
        let (rate, samples) = (44_100u64, 441_000u64);
        let packed = (rate << 44) | (1 << 41) | (15 << 36) | samples;
        info[10..18].copy_from_slice(&packed.to_be_bytes());
        flac.extend_from_slice(&info);
        assert_eq!(measured("a.flac", &flac), Some(10.0));
        // Ogg Opus: pre-skip 312, last granule 48 000 × 3 + 312.
        let mut opus = b"OggS\0\x02".to_vec();
        opus.resize(26, 0);
        opus.push(1);
        opus.push(19);
        opus.extend_from_slice(b"OpusHead\x01\x01");
        opus.extend_from_slice(&312u16.to_le_bytes());
        opus.extend_from_slice(&[0; 7]);
        let mut last = b"OggS\0\x04".to_vec();
        last.extend_from_slice(&(48_000i64 * 3 + 312).to_le_bytes());
        last.resize(40, 0);
        assert_eq!(measured("сказка.opus", &[opus, vec![0; 1000], last].concat()), Some(3.0));
        // WebM: Info with a float Duration of 2500 ms.
        let mut webm = b"\x1a\x45\xdf\xa3\x87\x42\x82\x84webm".to_vec();
        // The segment of unknown size (a live recording's), then its Info: the time scale, the duration.
        webm.extend_from_slice(b"\x18\x53\x80\x67\x01\xff\xff\xff\xff\xff\xff\xff");
        webm.extend_from_slice(b"\x15\x49\xa9\x66\x8e\x2a\xd7\xb1\x83\x0f\x42\x40\x44\x89\x84");
        webm.extend_from_slice(&2500f32.to_be_bytes());
        assert_eq!(measured("клип.webm", &webm), Some(2.5));
    }

    #[test]
    fn a_broken_file_has_no_length() {
        assert_eq!(measured("битое.mp4", b"\0\0\0\x18ftypisom\0\0\0\0isomavc1\0\0\0\x09moov"), None);
        assert_eq!(measured("битое.mp3", &[0xFF, 0xFB, 0x00, 0x00]), None);
    }
}
