//! The worker process: the only place transcribe.cpp's native code runs. It is this same program started with
//! `--asr-worker` (and `--cpu-only` after a GPU failure). It holds one model and serves requests until the gateway
//! closes its stdin; then it ends at once — even if the gateway crashed — so a worker never outlives its gateway. A GPU
//! driver that crashes or hangs takes only the worker down: the gateway starts another one. The way Handy does it
//! (engine_supervisor/worker.rs, MIT).

use std::fs::File;
use std::io::{self, BufReader};
use std::path::Path;
use std::sync::mpsc;
use std::thread;

use transcribe_cpp::{BackendMask, ErrorKind, Model, RunOptions, Session};

use super::protocol::{read_message, write_message, Request, Response};
use super::CPU_ONLY_FLAG;

struct Held {
    // The session borrows nothing from the model in the binding, but it is dropped first all the same.
    session: Session,
    _model: Model,
}

/// Serves until stdin ends; the exit code.
pub fn run() -> i32 {
    // The real stdout carries the protocol; fd 1 then points at stderr, so a stray print of native code (ggml writes
    // to stdout in places) lands in the log instead of breaking a frame.
    let mut out = match take_stdout() {
        Ok(out) => out,
        Err(error) => {
            eprintln!("asr worker: stdout not taken: {error}");
            return 2;
        }
    };
    let (requests_tx, requests) = mpsc::channel();
    // stdin is read on its own thread: the worker ends the moment the gateway goes away, even mid-load.
    let reader = thread::Builder::new().name("asr-worker-in".into()).spawn(move || {
        let mut input = BufReader::new(io::stdin().lock());
        loop {
            match read_message::<Request>(&mut input) {
                Ok(Some(request)) => {
                    if requests_tx.send(request).is_err() {
                        break;
                    }
                }
                Ok(None) => leave(),
                Err(error) => {
                    eprintln!("asr worker: the gateway's stream broke: {error}");
                    leave();
                }
            }
        }
    });
    if let Err(error) = reader {
        eprintln!("asr worker: no reader thread: {error}");
        return 2;
    }

    let cpu_only = std::env::args_os().any(|arg| arg == CPU_ONLY_FLAG);
    // Registering a GPU backend runs its driver: a broken driver crashes or hangs here, in the worker only.
    let init = if cpu_only {
        transcribe_cpp::init_backends_with(None::<&Path>, BackendMask::CPU)
    } else {
        transcribe_cpp::init_backends_default()
    };
    let init_error = init.err().map(|error| format!("движок распознавания не запустился: {error}"));

    let mut held: Option<Held> = None;
    while let Ok((request, pcm)) = requests.recv() {
        let response = match (&init_error, request) {
            (Some(error), _) => Response::Error { message: error.clone(), backend: true },
            (None, Request::Load { path }) => {
                held = None;
                match load(&path, cpu_only) {
                    Ok((loaded, backend, device)) => {
                        held = Some(loaded);
                        Response::Loaded { backend, device }
                    }
                    Err(error) => Response::Error {
                        message: format!("модель не загрузилась: {error}"),
                        backend: error.kind() == ErrorKind::Backend,
                    },
                }
            }
            (None, Request::Run { language }) => match held.as_mut() {
                None => Response::Error { message: "модель не загружена".into(), backend: false },
                Some(held) => recognize(&mut held.session, &pcm, language),
            },
        };
        if let Err(error) = write_message(&mut out, &response, &[]) {
            eprintln!("asr worker: the answer did not go: {error}");
            return 0;
        }
    }
    0
}

/// The gateway is gone: the worker ends at once, from the reader thread, while the model may still be loaded on the
/// main one. `_exit`, not `exit`: the C++ statics' destructors that `exit` runs check that the Metal buffers are freed
/// and abort when they are not (a crash report and «DuoWave quit unexpectedly» at every quit of the app, 12.11). The
/// system takes the memory and the GPU's buffers back either way.
fn leave() -> ! {
    // SAFETY: _exit ends the process without running anything else; nothing is left to flush (the frames are written
    // and flushed one by one, stderr is unbuffered).
    unsafe { libc::_exit(0) }
}

fn load(path: &Path, cpu_only: bool) -> transcribe_cpp::Result<(Held, String, String)> {
    let options = transcribe_cpp::ModelOptions {
        backend: if cpu_only { transcribe_cpp::Backend::Cpu } else { transcribe_cpp::Backend::Auto },
        ..Default::default()
    };
    let model = Model::load_with(path, &options)?;
    let session = model.session()?;
    let backend = model.backend();
    let device = model
        .device()
        .map(|device| if device.description.is_empty() { device.name } else { device.description })
        .unwrap_or_default();
    Ok((Held { session, _model: model }, backend, device))
}

fn recognize(session: &mut Session, pcm: &[f32], language: Option<String>) -> Response {
    let options = RunOptions { language, ..Default::default() };
    match session.run(pcm, &options) {
        Ok(transcript) => Response::Text { text: transcript.text.trim().to_string() },
        // Too short to hear anything (the keep-warm's half second of silence on some models): nothing heard.
        Err(error) if error.kind() == ErrorKind::InputTooShort => Response::Text { text: String::new() },
        Err(error) => Response::Error { message: error.to_string(), backend: error.kind() == ErrorKind::Backend },
    }
}

/// The real stdout as a file of its own, fd 1 pointed at stderr.
#[cfg(unix)]
fn take_stdout() -> io::Result<File> {
    use std::os::fd::FromRawFd;
    // SAFETY: dup and dup2 on the process's own standard descriptors, before any other thread writes to them.
    unsafe {
        let fd = libc::dup(libc::STDOUT_FILENO);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(File::from_raw_fd(fd))
    }
}

#[cfg(windows)]
fn take_stdout() -> io::Result<File> {
    use std::os::windows::io::FromRawHandle;
    // SAFETY: the C runtime's own descriptors 1 and 2, duplicated before any other thread writes to them.
    unsafe {
        let fd = libc::dup(1);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::dup2(2, 1) < 0 {
            return Err(io::Error::last_os_error());
        }
        let handle = libc::get_osfhandle(fd);
        if handle == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(File::from_raw_handle(handle as _))
    }
}
