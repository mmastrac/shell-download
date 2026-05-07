use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{Quiet, RequestBuilder, Response, ResponseError, StartError};

pub(crate) fn unique_suffix() -> Option<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    Some(format!("{}-{}", std::process::id(), now))
}

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

pub(crate) fn spawn_child_for_output(
    mut cmd: Command,
    _program: &'static str,
) -> Result<Child, StartError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match cmd.spawn() {
        Ok(c) => Ok(c),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(StartError::NoDriverFound),
        Err(e) => Err(StartError::IoError(e)),
    }
}

pub(crate) fn wait_child_with_output(
    mut child: Child,
    cancel: &Arc<AtomicBool>,
    program: &'static str,
    quiet: Quiet,
) -> Result<std::process::Output, ResponseError> {
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ResponseError::Cancelled);
        }

        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(ResponseError::Io(e)),
        }
    }

    let output = child.wait_with_output().map_err(ResponseError::Io)?;

    let should_forward = match quiet {
        Quiet::Always => false,
        Quiet::Never => true,
        Quiet::OnSuccess => !output.status.success(),
    };

    // TODO: We use println to ensure that tests don't print debugging data.
    // This should spawn a thread to capture output, however.
    if should_forward {
        println!("{}", String::from_utf8_lossy(&output.stdout));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }

    if !output.status.success() {
        return Err(ResponseError::CommandFailed {
            program,
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }
    Ok(output)
}

pub(crate) fn spawn_request_thread<F>(
    req: RequestBuilder,
    target_path: PathBuf,
    tmp_path: PathBuf,
    cancel: Arc<AtomicBool>,
    download_to_tmp: F,
) -> JoinHandle<Result<Response, ResponseError>>
where
    F: Send
        + 'static
        + FnOnce(&RequestBuilder, &Path, &Arc<AtomicBool>) -> Result<(u16, bool), ResponseError>,
{
    thread::spawn(move || {
        let _ = std::fs::remove_file(&tmp_path);

        let (status_code, content_encoding_gzip) = download_to_tmp(&req, &tmp_path, &cancel)?;

        if cancel.load(Ordering::SeqCst) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(ResponseError::Cancelled);
        }

        finalize_download(&tmp_path, &target_path, content_encoding_gzip)?;

        Ok(Response { status_code })
    })
}

pub(crate) fn tmp_path_for_target(target_path: &Path) -> PathBuf {
    let mut tmp = target_path.to_path_buf();
    tmp.set_extension(format!(
        "{}.tmp",
        unique_suffix().unwrap_or_else(|| "download".into())
    ));
    tmp
}

pub(crate) fn finalize_download(
    tmp_path: &Path,
    target_path: &Path,
    content_encoding_gzip: bool,
) -> Result<(), ResponseError> {
    let needs_gunzip = content_encoding_gzip || file_looks_gzipped(tmp_path).unwrap_or(false);
    if needs_gunzip {
        gunzip_to_target(tmp_path, target_path)?;
        let _ = std::fs::remove_file(tmp_path);
    } else {
        let _ = std::fs::remove_file(target_path);
        std::fs::rename(tmp_path, target_path).map_err(ResponseError::Io)?;
    }
    Ok(())
}

pub(crate) fn file_looks_gzipped(path: &Path) -> io::Result<bool> {
    let mut f = std::fs::File::open(path)?;
    let mut b = [0u8; 2];
    let n = f.read(&mut b)?;
    Ok(n == 2 && b == [0x1f, 0x8b])
}

pub(crate) fn gunzip_to_target(src: &Path, dst: &Path) -> Result<(), ResponseError> {
    let mut cmd = Command::new("gzip");
    cmd.arg("-dc")
        .arg(src)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(ResponseError::Io)?;

    let mut out_file = std::fs::File::create(dst)?;
    {
        let mut stdout = child.stdout.take().ok_or_else(|| {
            ResponseError::Io(io::Error::new(io::ErrorKind::Other, "missing gzip stdout"))
        })?;
        io::copy(&mut stdout, &mut out_file)?;
        out_file.flush()?;
    }

    let output = child.wait_with_output().map_err(ResponseError::Io)?;
    if !output.status.success() {
        return Err(ResponseError::GzipFailed {
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    Ok(())
}
