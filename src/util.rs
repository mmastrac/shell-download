use std::io::{self, Read as _, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{Error, RequestBuilder};

pub(crate) fn unique_suffix() -> Option<String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_millis();
    Some(format!("{}-{}", std::process::id(), now))
}

pub(crate) fn add_common_headers(req: &RequestBuilder) -> Vec<(String, String)> {
    let mut headers = req.headers.clone();
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("accept-encoding")) {
        headers.push(("Accept-Encoding".into(), "gzip".into()));
    }
    headers
}

pub(crate) fn downloader_available(program: &str) -> bool {
    program_in_path(program)
}

fn program_in_path(program: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut exts: Vec<std::ffi::OsString> = Vec::new();
    if cfg!(windows) {
        if let Some(pathext) = std::env::var_os("PATHEXT") {
            exts.extend(std::env::split_paths(&pathext).map(|p| p.into_os_string()));
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
                let mut p = dir.join(program);
                let ext_str = ext.to_string_lossy();
                let ext_no_dot = ext_str.strip_prefix('.').unwrap_or(&ext_str);
                p.set_extension(ext_no_dot);
                if p.is_file() {
                    return true;
                }
            }
        } else if dir.join(program).is_file() {
            return true;
        }
    }
    false
}

pub(crate) fn run_cancellable_command(
    mut cmd: Command,
    cancel: &Arc<AtomicBool>,
    program: &'static str,
    quiet: bool,
) -> Result<std::process::Output, Error> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(Error::Io)?;

    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Cancelled);
        }

        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(Error::Io(e)),
        }
    }

    let output = child.wait_with_output().map_err(Error::Io)?;

    // TODO: This should print "live" output
    if !quiet {
        println!("{}", String::from_utf8_lossy(&output.stdout));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }

    if !output.status.success() {
        return Err(Error::CommandFailed {
            program,
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }
    Ok(output)
}

pub(crate) fn file_looks_gzipped(path: &Path) -> io::Result<bool> {
    let mut f = std::fs::File::open(path)?;
    let mut b = [0u8; 2];
    let n = f.read(&mut b)?;
    Ok(n == 2 && b == [0x1f, 0x8b])
}

pub(crate) fn gunzip_to_target(src: &Path, dst: &Path) -> Result<(), Error> {
    let mut cmd = Command::new("gzip");
    cmd.arg("-dc").arg(src).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(Error::Io)?;

    let mut out_file = std::fs::File::create(dst)?;
    {
        let mut stdout = child.stdout.take().ok_or_else(|| {
            Error::Io(io::Error::new(io::ErrorKind::Other, "missing gzip stdout"))
        })?;
        io::copy(&mut stdout, &mut out_file)?;
        out_file.flush()?;
    }

    let output = child.wait_with_output().map_err(Error::Io)?;
    if !output.status.success() {
        return Err(Error::GzipFailed {
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    Ok(())
}

