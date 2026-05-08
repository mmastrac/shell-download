use std::io::{self, PipeWriter, Read as _};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::{
    ContentEncoding, DownloadResult, DownloadSink, Quiet, ResponseError, StartError,
    drivers::Request,
};

/// Ensure common headers are present (notably gzip support).
pub(crate) fn add_common_headers(req: &Request) -> Vec<(String, String)> {
    let mut headers = req.headers.clone();
    if !headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("accept-encoding"))
    {
        headers.push(("Accept-Encoding".into(), "gzip".into()));
    }
    headers
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
fn wait_child_into_sink(
    mut child: Child,
    stdout: ChildStdout,
    stderr_pipe: ChildStderr,
    sink: &DownloadSink,
    cancel: &Arc<AtomicBool>,
    program: &'static str,
    quiet: Quiet,
) -> Result<std::process::Output, ResponseError> {
    let stderr_handle = thread::spawn(move || {
        let mut stderr = stderr_pipe;
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

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
                    .map_err(|_| ResponseError::ThreadPanicked)??;

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

/// Spawn a download child ([`spawn_child_for_download`]), split stdout/stderr, then on the worker
/// thread wait for it (streaming stdout into `sink`) and pass the resulting [`std::process::Output`]
/// to `body`.
pub(crate) fn spawn_download_cmd_thread<F>(
    cmd: Command,
    program: &'static str,
    req: Request,
    sink: DownloadSink,
    cancel: Arc<AtomicBool>,
    body: F,
) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError>
where
    F: Send
        + 'static
        + FnOnce(
            std::process::Output,
            &Request,
        ) -> Result<(u16, Option<ContentEncoding>), ResponseError>,
{
    let mut child = {
        let mut cmd = cmd;
        let cmd: &mut Command = &mut cmd;
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match cmd.spawn() {
            Ok(c) => Ok(c),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(StartError::NoDriverFound),
            Err(e) => Err(StartError::IoError(e)),
        }
    }?;
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(StartError::IoError(io::Error::other(
                "missing child stdout",
            )));
        }
    };
    let stderr = match child.stderr.take() {
        Some(s) => s,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(StartError::IoError(io::Error::other(
                "missing child stderr",
            )));
        }
    };

    Ok(thread::spawn(move || {
        let output =
            wait_child_into_sink(child, stdout, stderr, &sink, &cancel, program, req.quiet)?;
        let (status_code, content_encoding) = body(output, &req)?;
        if cancel.load(Ordering::SeqCst) {
            return Err(ResponseError::Cancelled);
        }
        Ok(DownloadResult {
            status_code,
            content_encoding,
        })
    }))
}

/// Tunnel / streaming body path: worker creates a pipe, drains the read end with
/// [`DownloadSink::spawn_stdout_drain`], and passes the write end to `download_to_tmp`.
pub(crate) fn spawn_download_thread<F>(
    req: Request,
    sink: DownloadSink,
    cancel: Arc<AtomicBool>,
    download_to_tmp: F,
) -> JoinHandle<Result<DownloadResult, ResponseError>>
where
    F: Send
        + 'static
        + FnOnce(
            &Request,
            &DownloadSink,
            &Arc<AtomicBool>,
            PipeWriter,
        ) -> Result<(u16, Option<ContentEncoding>), ResponseError>,
{
    thread::spawn(move || {
        let (pipe_reader, pipe_writer) = std::io::pipe().map_err(ResponseError::Io)?;
        let copy_handle = sink.clone().spawn_stdout_drain(pipe_reader);

        let download_result = download_to_tmp(&req, &sink, &cancel, pipe_writer);
        let copy_result = copy_handle
            .join()
            .map_err(|_| ResponseError::ThreadPanicked)?;

        let (status_code, content_encoding) = download_result?;
        copy_result?;

        if cancel.load(Ordering::SeqCst) {
            return Err(ResponseError::Cancelled);
        }

        Ok(DownloadResult {
            status_code,
            content_encoding,
        })
    })
}
