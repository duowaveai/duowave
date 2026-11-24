//! Where a spoken answer's audio goes as it is played: a tale kept to be played again from /media.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Mutex;

pub trait AudioRecording: Send + Sync {
    fn write(&self, bytes: &[u8]);
    /// Kept when the answer was spoken to the end, dropped when it was cut.
    fn finish(&self, complete: bool);
}

/// The joined Ogg stream into a file: written as `.part`, renamed when complete.
pub struct FileRecording {
    file: PathBuf,
    partial: PathBuf,
    stream: Mutex<Option<BufWriter<File>>>,
}

impl FileRecording {
    pub fn new(file: PathBuf) -> FileRecording {
        let mut partial = file.clone().into_os_string();
        partial.push(".part");
        FileRecording { file, partial: PathBuf::from(partial), stream: Mutex::new(None) }
    }
}

impl AudioRecording for FileRecording {
    fn write(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut stream = self.stream.lock().unwrap();
        if stream.is_none() {
            if let Some(dir) = self.file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            *stream = File::create(&self.partial).ok().map(BufWriter::new);
        }
        // A recording is a convenience: losing it loses nothing else.
        if let Some(stream) = stream.as_mut() {
            let _ = stream.write_all(bytes);
        }
    }

    fn finish(&self, complete: bool) {
        let Some(mut stream) = self.stream.lock().unwrap().take() else { return };
        let flushed = stream.flush().is_ok();
        drop(stream);
        if complete && flushed {
            let _ = std::fs::rename(&self.partial, &self.file);
        } else {
            let _ = std::fs::remove_file(&self.partial);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_whole_answer_and_drops_a_cut_one() {
        let dir = tempfile::tempdir().unwrap();
        let kept = dir.path().join("tales/generated/ezhik.ogg");
        let recording = FileRecording::new(kept.clone());
        recording.write(b"");
        recording.write(b"OggS");
        recording.write(b" more");
        recording.finish(true);
        assert_eq!(std::fs::read(&kept).unwrap(), b"OggS more");

        let cut = dir.path().join("tales/generated/cut.ogg");
        let recording = FileRecording::new(cut.clone());
        recording.write(b"OggS");
        recording.finish(false);
        assert!(!cut.exists());
        assert!(!dir.path().join("tales/generated/cut.ogg.part").exists());
    }
}
