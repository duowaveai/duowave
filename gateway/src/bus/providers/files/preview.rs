//! A picture of the home folder shown small (docs/plans/shared-folders.md, «Новые умения папки»): a photo of 20 MB
//! never goes to a phone or a card whole. JPEG, PNG, WebP and GIF are made small here, turned as the camera's EXIF
//! says; a Mac makes HEIC (an iPhone's photos) small by its own `sips`. What was made is kept in DATA_DIR's
//! `folder-previews` — up to 500 MB, the oldest going first — under the file's place, size and time, so the same
//! photo at the same size is made once.
use std::io::Cursor;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The longest side of a picture made small, at most.
pub const SIDE_MOST: u32 = 2048;
/// The side when none is asked.
pub const SIDE: u32 = 1024;
/// The made pictures kept at most.
pub const KEPT_MOST: u64 = 500 * 1024 * 1024;
/// A picture made small at most, in bytes of its own.
pub const SOURCE_MOST: u64 = 64 * 1024 * 1024;

/// The pictures made small.
pub fn picture(name: &str) -> bool {
    let extension = name.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).unwrap_or_default();
    matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif" | "heic" | "heif")
}

/// A JPEG of the picture at `source`, its longer side `side` at most; from the kept ones when it was made before.
pub fn preview(source: &Path, side: u32, kept: &Path) -> Result<Vec<u8>, String> {
    let side = side.clamp(64, SIDE_MOST);
    let meta = std::fs::metadata(source).map_err(|error| error.to_string())?;
    if meta.len() > SOURCE_MOST {
        return Err(crate::tr!("картинка больше {max} МБ", max = SOURCE_MOST / 1024 / 1024));
    }
    let modified = meta.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok());
    let key = format!("{}|{}|{:?}|{side}", source.display(), meta.len(), modified);
    let file = kept.join(format!("{}.jpg", hex::encode(&Sha256::digest(key.as_bytes())[..16])));
    if let Ok(bytes) = std::fs::read(&file) {
        return Ok(bytes);
    }
    let name = source.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    let bytes = if name.ends_with(".heic") || name.ends_with(".heif") {
        heic(source, side)?
    } else {
        made_small(source, side)?
    };
    if std::fs::create_dir_all(kept).is_ok() && std::fs::write(&file, &bytes).is_ok() {
        trim(kept);
    }
    Ok(bytes)
}

fn made_small(source: &Path, side: u32) -> Result<Vec<u8>, String> {
    use image::ImageDecoder;
    let reader = image::ImageReader::open(source)
        .map_err(|error| error.to_string())?
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|_| crate::tr!("эту картинку не открыть"))?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut picture = image::DynamicImage::from_decoder(decoder).map_err(|_| crate::tr!("эту картинку не открыть"))?;
    picture.apply_orientation(orientation);
    let small = picture.thumbnail(side, side).to_rgb8();
    let mut out = Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82)
        .encode_image(&small)
        .map_err(|error| error.to_string())?;
    Ok(out.into_inner())
}

/// HEIC by the Mac's own `sips`; elsewhere it is not shown.
fn heic(source: &Path, side: u32) -> Result<Vec<u8>, String> {
    if !cfg!(target_os = "macos") {
        return Err(crate::tr!("фото в формате HEIC здесь пока не показать"));
    }
    let out = std::env::temp_dir().join(format!("duowave-heic-{}.jpg", crate::utils::ids::random_hex(6)));
    let done = std::process::Command::new("sips")
        .args(["-s", "format", "jpeg", "-Z", &side.to_string()])
        .arg(source)
        .arg("--out")
        .arg(&out)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    let bytes = std::fs::read(&out).map_err(|_| crate::tr!("фото HEIC не открылось"));
    let _ = std::fs::remove_file(&out);
    if !done.success() {
        return Err(crate::tr!("фото HEIC не открылось"));
    }
    bytes
}

/// The kept pictures within their room, the oldest going first.
fn trim(kept: &Path) {
    let Ok(entries) = std::fs::read_dir(kept) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    if total <= KEPT_MOST {
        return;
    }
    files.sort_by_key(|(at, _, _)| *at);
    for (_, size, path) in files {
        if total <= KEPT_MOST * 9 / 10 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_picture_is_made_small_once() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("большое.png");
        image::RgbImage::from_pixel(1600, 900, image::Rgb([200, 120, 40])).save(&source).unwrap();
        let kept = dir.path().join("kept");
        let bytes = preview(&source, 400, &kept).unwrap();
        let small = image::load_from_memory(&bytes).unwrap();
        assert_eq!((small.width(), small.height()), (400, 225));
        assert_eq!(std::fs::read_dir(&kept).unwrap().count(), 1);
        assert_eq!(preview(&source, 400, &kept).unwrap(), bytes, "made once");
        assert!(preview(&dir.path().join("нет.png"), 400, &kept).is_err());
        std::fs::write(dir.path().join("битое.jpg"), "not a picture").unwrap();
        assert!(preview(&dir.path().join("битое.jpg"), 400, &kept).is_err());
        assert!(picture("IMG_0001.HEIC") && !picture("doc.pdf"));
    }
}
