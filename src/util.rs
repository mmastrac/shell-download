use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::{
    ContentEncoding, DownloadResult, DownloadSink, Quiet, RequestBuilder, ResponseError, StartError,
};

/// Ensure common headers are present (notably gzip support).
pub(crate) fn add_common_headers(req: &RequestBuilder) -> Vec<(String, String)> {
    let mut headers = req.headers.clone();
    if !headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("accept-encoding"))
    {
        headers.push(("Accept-Encoding".into(), "gzip".into()));
    }
    headers
}

/// Spawn a download child: stdin null, stdout/stderr piped (body read via [`DownloadSink::spawn_stdout_drain`]).
pub(crate) fn spawn_child_for_download(
    mut cmd: Command,
    _program: &'static str,
) -> Result<Child, StartError> {
    DownloadSink::attach_download_stdio(&mut cmd);
    match cmd.spawn() {
        Ok(c) => Ok(c),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(StartError::NoDriverFound),
        Err(e) => Err(StartError::IoError(e)),
    }
}

/// Find all matching executables in `PATH`.
pub(crate) fn find_program_in_path(program: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();

    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut exts: Vec<std::ffi::OsString> = Vec::new();
    if cfg!(windows) {
        if let Some(pathext) = std::env::var_os("PATHEXT") {
            exts = pathext
                .to_string_lossy()
                .split(';')
                .filter(|s| !s.is_empty())
                .map(|s| s.into())
                .collect();
        }
        if exts.is_empty() {
            exts = vec![".EXE".into(), ".CMD".into(), ".BAT".into()];
        }
    }

    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        if cfg!(windows) {
            for ext in &exts {
                let ext_str = ext.to_string_lossy();
                let ext_no_dot = ext_str.strip_prefix('.').unwrap_or(&ext_str);
                let mut p = dir.join(program);
                p.set_extension(ext_no_dot);
                if p.is_file() {
                    out.push(p);
                }
            }
        } else {
            let p = dir.join(program);
            if p.is_file() {
                out.push(p);
            }
        }
    }

    out
}

/// Wait for a download child: stream stdout into `sink`, buffer stderr.
pub(crate) fn wait_child_into_sink(
    mut child: Child,
    sink: &DownloadSink,
    cancel: &Arc<AtomicBool>,
    program: &'static str,
    quiet: Quiet,
) -> Result<std::process::Output, ResponseError> {
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| ResponseError::Io(io::Error::other("missing child stderr")))?;

    let stderr_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ResponseError::Io(io::Error::other("missing child stdout")))?;
    let copy_handle = sink.clone().spawn_stdout_drain(stdout);

    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = copy_handle.join();
            let _ = stderr_handle.join();
            return Err(ResponseError::Cancelled);
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                copy_handle
                    .join()
                    .map_err(|_| ResponseError::ThreadPanicked)?
                    .map_err(ResponseError::Io)?;

                let stderr_bytes = stderr_handle
                    .join()
                    .map_err(|_| ResponseError::ThreadPanicked)?;

                let should_forward = match quiet {
                    Quiet::Always => false,
                    Quiet::Never => true,
                    Quiet::OnSuccess => !status.success(),
                };
                if should_forward {
                    eprintln!("{}", String::from_utf8_lossy(&stderr_bytes));
                }
                if !status.success() {
                    return Err(ResponseError::CommandFailed {
                        program,
                        exit_code: status.code(),
                        stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
                    });
                }
                return Ok(std::process::Output {
                    status,
                    stdout: Vec::new(),
                    stderr: stderr_bytes,
                });
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(ResponseError::Io(e)),
        }
    }
}

/// Spawn a worker thread that runs a backend download function.
pub(crate) fn spawn_download_thread<F>(
    req: RequestBuilder,
    sink: DownloadSink,
    cancel: Arc<AtomicBool>,
    download_to_tmp: F,
) -> JoinHandle<Result<DownloadResult, ResponseError>>
where
    F: Send
        + 'static
        + FnOnce(
            &RequestBuilder,
            &DownloadSink,
            &Arc<AtomicBool>,
        ) -> Result<(u16, Option<ContentEncoding>), ResponseError>,
{
    thread::spawn(move || {
        let (status_code, content_encoding) = download_to_tmp(&req, &sink, &cancel)?;

        if cancel.load(Ordering::SeqCst) {
            sink.cleanup_on_cancel();
            return Err(ResponseError::Cancelled);
        }

        Ok(DownloadResult {
            status_code,
            content_encoding,
        })
    })
}

/// Decompress gzip bytes using the system `gzip` CLI (stdin → stdout).
pub(crate) fn gunzip_from_bytes(input: &[u8]) -> Result<Vec<u8>, ResponseError> {
    let mut cmd = Command::new("gzip");
    cmd.arg("-dc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(ResponseError::Io)?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ResponseError::Io(io::Error::other("missing gzip stdin")))?;
    stdin.write_all(input).map_err(ResponseError::Io)?;
    drop(stdin);

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ResponseError::Io(io::Error::other("missing gzip stdout")))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ResponseError::Io(io::Error::other("missing gzip stderr")))?;

    let stderr_join = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let mut out = Vec::new();
    stdout
        .read_to_end(&mut out)
        .map_err(ResponseError::Io)?;

    let status = child.wait().map_err(ResponseError::Io)?;
    let stderr_bytes = stderr_join.join().unwrap_or_default();

    if !status.success() {
        return Err(ResponseError::GzipFailed {
            exit_code: status.code(),
            stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
        });
    }
    Ok(out)
}

/// Finish an in-memory download: apply gzip detection / decoding like [`finalize_download`].
pub(crate) fn finalize_memory_body(
    sink: DownloadSink,
    content_encoding: Option<ContentEncoding>,
) -> Result<Vec<u8>, ResponseError> {
    let bytes = sink.take_buffer_bytes().map_err(ResponseError::Io)?;
    let declared_gzip = matches!(content_encoding, Some(ContentEncoding::Gzip));
    let looks_gzip = bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b;
    if declared_gzip || looks_gzip {
        gunzip_from_bytes(&bytes)
    } else {
        Ok(bytes)
    }
}

/// Move or decode the temp file into its final location.
pub(crate) fn finalize_download(
    tmp_file: crate::tempfile::TmpFile,
    target_path: &Path,
    content_encoding: Option<ContentEncoding>,
) -> Result<(), ResponseError> {
    let declared_gzip = matches!(content_encoding, Some(ContentEncoding::Gzip));
    let needs_gunzip = declared_gzip || file_looks_gzipped(&tmp_file).unwrap_or(false);
    if needs_gunzip {
        gunzip_to_target(&tmp_file, target_path)?;
    } else {
        tmp_file.persist(target_path).map_err(ResponseError::Io)?;
    }
    Ok(())
}

/// True if the file begins with gzip magic (`read` uses share flags on Windows).
pub(crate) fn file_looks_gzipped(path: impl AsRef<Path>) -> io::Result<bool> {
    let path = path.as_ref();
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_SHARE_READ: u32 = 0x00000001;
        const FILE_SHARE_WRITE: u32 = 0x00000002;
        opts.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let mut f = opts.open(path)?;
    let mut b = [0u8; 2];
    let n = f.read(&mut b)?;
    Ok(n == 2 && b == [0x1f, 0x8b])
}

pub(crate) fn gunzip_to_target(
    src: impl AsRef<Path>,
    dst: impl AsRef<Path>,
) -> Result<(), ResponseError> {
    let src = src.as_ref();
    let dst = dst.as_ref();

    let mut cmd = Command::new("gzip");
    cmd.arg("-dc")
        .arg(src)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(ResponseError::Io)?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ResponseError::Io(io::Error::other("missing gzip stdout")))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ResponseError::Io(io::Error::other("missing gzip stderr")))?;

    // Read stderr concurrently to avoid pipe deadlocks if gzip is noisy.
    let stderr_join = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    // Write to a temp file and then atomically rename into place.
    let parent = dst.parent().unwrap_or_else(|| Path::new("."));
    let hint = dst.file_name().and_then(|s| s.to_str()).unwrap_or("gunzip");
    let tmp_dst = crate::tempfile::create_tmp_file_in_path("gunzip", None, parent, hint)?;
    let mut out_file = std::fs::File::options()
        .write(true)
        .truncate(true)
        .open(&tmp_dst)
        .map_err(ResponseError::Io)?;

    io::copy(&mut stdout, &mut out_file).map_err(ResponseError::Io)?;
    out_file.flush().map_err(ResponseError::Io)?;

    let status = child.wait().map_err(ResponseError::Io)?;
    let stderr_bytes = stderr_join.join().unwrap_or_default();

    if !status.success() {
        return Err(ResponseError::GzipFailed {
            exit_code: status.code(),
            stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
        });
    }

    tmp_dst.persist(dst).map_err(ResponseError::Io)?;
    Ok(())
}
