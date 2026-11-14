//! What a file is, by its first bytes — not by its name (docs/plans/feed.md, «Один движок показа»): a photo, a video
//! or a sound, and its type for the browser. The name only tells which of two alike a container holds — an MP4 of
//! sound alone (`.m4a`) is `audio/mp4`, a WebM of sound alone `audio/webm` — and never makes a page a picture.
//! Anything a browser would run or draw as a page (HTML, SVG, XML, scripts) is never media, whatever its name says.
//! A sound or a video whose first bytes say nothing — an MP3 with junk before its first frame — goes by its name, but
//! only when those bytes are no markup and no script: a browser plays it or does not, and runs nothing.

use serde::Serialize;

/// A photo, a video or a sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Image,
    Video,
    Audio,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Image => "image",
            Kind::Video => "video",
            Kind::Audio => "audio",
        }
    }
}

/// What the first bytes say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sniffed {
    pub kind: Kind,
    /// Its type for the browser: `image/jpeg`, `video/mp4`, `audio/mpeg`…
    pub mime: &'static str,
    /// Whether browsers show it as it is: HEIC and TIFF are photos only Safari draws — the viewer asks for them made
    /// small (`?size=`), as JPEG.
    pub shown: bool,
}

/// How many first bytes `sniff` wants: the longest it reads is an MP4's `ftyp` box with its brands.
pub const HEAD: usize = 64;

/// The extensions a browser runs or draws as a page: never served, whatever the bytes are.
pub const RUNNABLE: &[&str] = &["svg", "svgz", "html", "htm", "xhtml", "xht", "xml", "xsl", "js", "mjs", "cjs", "wasm"];

fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).unwrap_or_default()
}

const fn media(kind: Kind, mime: &'static str) -> Option<Sniffed> {
    Some(Sniffed { kind, mime, shown: true })
}

/// A photo, a video or a sound by its first bytes (`HEAD` of them is enough), the name only telling alike containers
/// apart; bytes that say nothing and run nothing — a sound or a video by its name. Anything else — none.
pub fn sniff(head: &[u8], name: &str) -> Option<Sniffed> {
    if runnable(head, name) {
        return None;
    }
    let ext = extension(name);
    by_bytes(head, &ext).or_else(|| by_name(&ext))
}

/// A sound or a video by its name alone (never a photo: photos always say what they are).
fn by_name(ext: &str) -> Option<Sniffed> {
    let (kind, mime) = match ext {
        "mp3" => (Kind::Audio, "audio/mpeg"),
        "aac" => (Kind::Audio, "audio/aac"),
        "m4a" | "m4b" => (Kind::Audio, "audio/mp4"),
        "wav" => (Kind::Audio, "audio/wav"),
        "ogg" | "oga" | "opus" => (Kind::Audio, "audio/ogg"),
        "flac" => (Kind::Audio, "audio/flac"),
        "mp4" | "m4v" => (Kind::Video, "video/mp4"),
        "mov" => (Kind::Video, "video/quicktime"),
        "webm" => (Kind::Video, "video/webm"),
        _ => return None,
    };
    Some(Sniffed { kind, mime, shown: true })
}

fn by_bytes(head: &[u8], ext: &str) -> Option<Sniffed> {
    let at = |from: usize, bytes: &[u8]| head.get(from..from + bytes.len()) == Some(bytes);
    if at(0, &[0xFF, 0xD8, 0xFF]) {
        return media(Kind::Image, "image/jpeg");
    }
    if at(0, &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return media(Kind::Image, "image/png");
    }
    if at(0, b"GIF87a") || at(0, b"GIF89a") {
        return media(Kind::Image, "image/gif");
    }
    if at(0, b"RIFF") {
        return match head.get(8..12) {
            Some(b"WEBP") => media(Kind::Image, "image/webp"),
            Some(b"WAVE") => media(Kind::Audio, "audio/wav"),
            Some(b"AVI ") => Some(Sniffed { kind: Kind::Video, mime: "video/x-msvideo", shown: false }),
            _ => None,
        };
    }
    if at(0, b"BM")
        && head.len() >= 18
        && matches!(u32::from_le_bytes([head[14], head[15], head[16], head[17]]), 12 | 40 | 52 | 56 | 108 | 124)
    {
        return media(Kind::Image, "image/bmp");
    }
    if at(0, b"II*\0") || at(0, b"MM\0*") {
        return Some(Sniffed { kind: Kind::Image, mime: "image/tiff", shown: false });
    }
    if at(4, b"ftyp") {
        return iso_media(head, ext);
    }
    if at(0, &[0x1A, 0x45, 0xDF, 0xA3]) {
        // EBML: its DocType says WebM or Matroska; sound alone only by the name.
        let webm = head.windows(4).any(|window| window == b"webm");
        let sound = matches!(ext, "weba" | "mka" | "opus");
        return Some(match (webm, sound) {
            (true, true) => Sniffed { kind: Kind::Audio, mime: "audio/webm", shown: true },
            (true, false) => Sniffed { kind: Kind::Video, mime: "video/webm", shown: true },
            (false, true) => Sniffed { kind: Kind::Audio, mime: "audio/x-matroska", shown: false },
            (false, false) => Sniffed { kind: Kind::Video, mime: "video/x-matroska", shown: false },
        });
    }
    if at(0, b"OggS") {
        return if ext == "ogv" { media(Kind::Video, "video/ogg") } else { media(Kind::Audio, "audio/ogg") };
    }
    if at(0, b"fLaC") {
        return media(Kind::Audio, "audio/flac");
    }
    if at(0, b"FORM") && (at(8, b"AIFF") || at(8, b"AIFC")) {
        return Some(Sniffed { kind: Kind::Audio, mime: "audio/aiff", shown: false });
    }
    if at(0, b"ID3") {
        return if ext == "aac" { media(Kind::Audio, "audio/aac") } else { media(Kind::Audio, "audio/mpeg") };
    }
    // An MPEG audio frame: eleven bits set; its layer bits 00 are AAC's ADTS.
    if head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0 {
        let layer = (head[1] >> 1) & 0x03;
        let version = (head[1] >> 3) & 0x03;
        return match (layer, version) {
            (0, _) if head[1] & 0xF6 == 0xF0 => media(Kind::Audio, "audio/aac"),
            (1..=3, 0 | 2 | 3) => media(Kind::Audio, "audio/mpeg"),
            _ => None,
        };
    }
    None
}

/// An ISO media file by its `ftyp` box: HEIC and AVIF photos, sound (M4A), QuickTime, 3GP, or an MP4 video.
fn iso_media(head: &[u8], ext: &str) -> Option<Sniffed> {
    let brand = head.get(8..12)?;
    // The box's own size bounds its brands: the major one, its version, then the compatible ones.
    let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
    let compatible: &[[u8; 4]] = head.get(16..size.min(head.len())).unwrap_or_default().as_chunks::<4>().0;
    let brands: Vec<&[u8]> = std::iter::once(brand).chain(compatible.iter().map(|brand| &brand[..])).collect();
    let has = |wanted: &[u8]| brands.contains(&wanted);
    if has(b"avif") || has(b"avis") {
        return media(Kind::Image, "image/avif");
    }
    if [b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1"].iter().any(|wanted| brand == *wanted) {
        return Some(Sniffed { kind: Kind::Image, mime: "image/heic", shown: false });
    }
    if [b"M4A ", b"M4B ", b"M4P ", b"F4A ", b"F4B "].iter().any(|wanted| brand == *wanted)
        || matches!(ext, "m4a" | "m4b" | "aac" | "mp4a")
    {
        return media(Kind::Audio, "audio/mp4");
    }
    if brand == b"qt  " {
        return media(Kind::Video, "video/quicktime");
    }
    if brand.starts_with(b"3g") {
        return media(Kind::Video, "video/3gpp");
    }
    media(Kind::Video, "video/mp4")
}

/// What a browser would run or draw as a page: a runnable name, or a text that opens with markup (`<html`, `<svg`,
/// `<?xml` — a byte-order mark and spaces aside) or a script's `#!`.
pub fn runnable(head: &[u8], name: &str) -> bool {
    if RUNNABLE.contains(&extension(name).as_str()) {
        return true;
    }
    let text = head.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(head);
    let start = text.iter().position(|byte| !byte.is_ascii_whitespace()).map_or(&[][..], |at| &text[at..]);
    start.starts_with(b"<") || start.starts_with(b"#!")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ftyp(major: &[u8; 4], compatible: &[&[u8; 4]]) -> Vec<u8> {
        let size = 16 + compatible.len() * 4;
        let mut head = (size as u32).to_be_bytes().to_vec();
        head.extend_from_slice(b"ftyp");
        head.extend_from_slice(major);
        head.extend_from_slice(&[0, 0, 0, 0]);
        for brand in compatible {
            head.extend_from_slice(*brand);
        }
        head.extend_from_slice(b"\0\0\0\x08free");
        head
    }

    #[test]
    fn knows_photos_videos_and_sounds_by_their_bytes() {
        let kind = |head: &[u8], name: &str| sniff(head, name).map(|sniffed| (sniffed.kind, sniffed.mime));
        assert_eq!(
            kind(&[0xFF, 0xD8, 0xFF, 0xE1], "кот.png"),
            Some((Kind::Image, "image/jpeg")),
            "the bytes, not the name"
        );
        assert_eq!(kind(b"\x89PNG\r\n\x1a\n....", "a"), Some((Kind::Image, "image/png")));
        assert_eq!(kind(b"GIF89a", "a.gif"), Some((Kind::Image, "image/gif")));
        assert_eq!(kind(b"RIFF\0\0\0\0WEBPVP8 ", "a"), Some((Kind::Image, "image/webp")));
        assert_eq!(kind(b"RIFF\0\0\0\0WAVEfmt ", "a"), Some((Kind::Audio, "audio/wav")));
        assert_eq!(kind(&ftyp(b"heic", &[b"mif1", b"heic"]), "IMG_0001.HEIC"), Some((Kind::Image, "image/heic")));
        assert!(!sniff(&ftyp(b"heic", &[b"mif1"]), "a").unwrap().shown, "the viewer asks for a HEIC made small");
        assert_eq!(kind(&ftyp(b"avif", &[b"mif1"]), "a"), Some((Kind::Image, "image/avif")));
        assert_eq!(kind(&ftyp(b"mif1", &[b"avif"]), "a"), Some((Kind::Image, "image/avif")), "AVIF among the brands");
        assert_eq!(kind(&ftyp(b"isom", &[b"iso2", b"avc1"]), "серия.mp4"), Some((Kind::Video, "video/mp4")));
        assert_eq!(
            kind(&ftyp(b"isom", &[b"iso2"]), "голос.m4a"),
            Some((Kind::Audio, "audio/mp4")),
            "sound alone by its name"
        );
        assert_eq!(kind(&ftyp(b"M4A ", &[b"isom"]), "a"), Some((Kind::Audio, "audio/mp4")));
        assert_eq!(kind(&ftyp(b"qt  ", &[b"qt  "]), "IMG_0002.MOV"), Some((Kind::Video, "video/quicktime")));
        assert_eq!(
            kind(b"\x1aE\xdf\xa3\x9fB\x86\x81\x01B\xf7\x81\x01B\xf2\x81\x04B\xf3\x81\x08B\x82\x84webm", "a"),
            Some((Kind::Video, "video/webm"))
        );
        assert_eq!(kind(b"OggS\0\x02", "сказка.opus"), Some((Kind::Audio, "audio/ogg")));
        assert_eq!(kind(b"fLaC\0\0\0\x22", "a"), Some((Kind::Audio, "audio/flac")));
        assert_eq!(kind(b"ID3\x04\0\0\0\0\0\0", "песня.mp3"), Some((Kind::Audio, "audio/mpeg")));
        assert_eq!(kind(&[0xFF, 0xFB, 0x90, 0x64], "a"), Some((Kind::Audio, "audio/mpeg")), "an MP3 frame");
        assert_eq!(kind(&[0xFF, 0xF1, 0x50, 0x80], "a"), Some((Kind::Audio, "audio/aac")), "AAC's ADTS");
        assert_eq!(kind(b"<svg xmlns='http://www.w3.org/2000/svg'/>", "кот.jpg"), None, "a page named a photo");
        assert_eq!(kind(b"\x89PNG\r\n\x1a\n", "logo.svg"), None, "a photo named a page");
        assert_eq!(kind(b"%PDF-1.7", "a.pdf"), None);
        assert_eq!(
            kind(b"\0\0\0\0\xff\xfb", "старая.mp3"),
            Some((Kind::Audio, "audio/mpeg")),
            "junk first: by its name"
        );
        assert_eq!(kind(b"<html>", "песня.mp3"), None, "but never markup");
        assert_eq!(kind(b"\0\0\0\0", "кот.jpg"), None, "a photo always says what it is");
    }

    #[test]
    fn never_serves_what_a_browser_runs() {
        assert!(runnable(b"\xEF\xBB\xBF  <html><script>", "notes.txt"), "markup, whatever the name");
        assert!(runnable(b"<?xml version='1.0'?><svg/>", "a.png"));
        assert!(runnable(b"#!/bin/sh", "a"));
        assert!(runnable(b"\x89PNG", "picture.SVG"), "a runnable name, whatever the bytes");
        assert!(!runnable(b"\x89PNG\r\n\x1a\n", "picture.png"));
        assert!(!runnable(b"wOF2", "font.woff2"));
    }
}
