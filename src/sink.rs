use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;

/// Where a backend writes the downloaded response body.
///
/// Child processes always use piped stdout; the body is processed from [`ChildStdout`] in a worker
/// thread (to file or in-memory buffer) via [`DownloadSink::spawn_stdout_drain`].
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

    /// Spawn a thread that reads the child's stdout into this sink (file or buffer).
    pub(crate) fn spawn_stdout_drain(self, mut stdout: ChildStdout) -> JoinHandle<io::Result<u64>> {
        thread::spawn(move || match self.inner {
            SinkInner::File(path) => {
                let mut f = open_body_stream(&path)?;
                io::copy(&mut stdout, &mut f)
            }
            SinkInner::Buffer(buf) => {
                let mut g = buf.lock().unwrap();
                io::copy(&mut stdout, &mut *g)
            }
        })
    }

    /// Takes ownership and returns accumulated bytes (buffer sink only).
    pub(crate) fn take_buffer_bytes(self) -> Result<Vec<u8>, io::Error> {
        match self.inner {
            SinkInner::File(_) => Err(io::Error::other("not a buffer sink")),
            SinkInner::Buffer(b) => Ok(std::mem::take(&mut *b.lock().unwrap())),
        }
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
