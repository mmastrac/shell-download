use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;

/// Where a backend writes the downloaded response body.
#[derive(Clone, Debug)]
pub struct DownloadSink {
    inner: SinkInner,
}

#[derive(Clone, Debug)]
enum SinkInner {
    File(PathBuf),
    Buffer(Arc<Mutex<Vec<u8>>>),
}

impl DownloadSink {
    /// Write the body to a new or existing file at `path` (second handle; see temp-file docs).
    pub fn file(path: PathBuf) -> Self {
        Self {
            inner: SinkInner::File(path),
        }
    }

    /// Accumulate the raw body in memory.
    pub fn buffer() -> Self {
        Self {
            inner: SinkInner::Buffer(Arc::new(Mutex::new(Vec::new()))),
        }
    }

    pub(crate) fn write_all_body(&self, bytes: &[u8]) -> io::Result<()> {
        match &self.inner {
            SinkInner::File(path) => {
                let mut f = open_body_stream(path)?;
                f.write_all(bytes)?;
                f.flush()?;
                Ok(())
            }
            SinkInner::Buffer(buf) => {
                buf.lock().unwrap().extend_from_slice(bytes);
                Ok(())
            }
        }
    }

    /// Configure `cmd` for a download child: stdin null, stderr piped.
    /// On a [`DownloadSink::file`] sink, stdout is that file (child writes directly).
    /// On a buffer sink, stdout is piped for the parent to drain.
    ///
    /// Returns `true` if stdout was attached to the file (no parent read of stdout).
    pub(crate) fn attach_stdout(&self, cmd: &mut Command) -> io::Result<bool> {
        cmd.stdin(Stdio::null()).stderr(Stdio::piped());
        match &self.inner {
            SinkInner::File(path) => {
                let f = open_body_stream(path)?;
                cmd.stdout(Stdio::from(f));
                Ok(true)
            }
            SinkInner::Buffer(_) => {
                cmd.stdout(Stdio::piped());
                Ok(false)
            }
        }
    }

    pub(crate) fn cleanup_on_cancel(&self) {
        match &self.inner {
            SinkInner::File(p) => {
                let _ = std::fs::remove_file(p);
            }
            SinkInner::Buffer(b) => {
                b.lock().unwrap().clear();
            }
        }
    }

    pub(crate) fn drain_piped_stdout(self, mut stdout: ChildStdout) -> JoinHandle<io::Result<u64>> {
        thread::spawn(move || match &self.inner {
            SinkInner::File(_) => Err(io::Error::other(
                "internal error: piped stdout with file sink",
            )),
            SinkInner::Buffer(buf) => {
                let mut g = buf.lock().unwrap();
                io::copy(&mut stdout, &mut *g)
            }
        })
    }
}

pub(crate) fn open_body_stream(path: &Path) -> io::Result<std::fs::File> {
    let mut opts = OpenOptions::new();
    opts.write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_SHARE_READ: u32 = 0x00000001;
        const FILE_SHARE_WRITE: u32 = 0x00000002;
        opts.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    opts.open(path)
}
