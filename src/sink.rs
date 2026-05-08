use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::ResponseError;
use crate::tempfile::TmpFile;

/// Where a backend writes the downloaded response body.
///
/// Child processes (or a [`std::io::PipeReader`]) stream bytes into this sink from a worker thread
/// via [`DownloadSink::spawn_stdout_drain`]. If the stream begins with the gzip magic bytes,
/// bytes are piped through `gzip -dc` while copying (streaming decompress).
#[derive(Clone, Debug)]
pub struct DownloadSink {
    inner: SinkInner,
}

#[derive(Clone, Debug)]
enum SinkInner {
    File((Arc<Mutex<Option<TmpFile>>>, PathBuf)),
    Buffer(Arc<Mutex<Vec<u8>>>),
}

fn sniff_is_gzip(n: usize, peek: &[u8; 2]) -> bool {
    n == 2 && peek[0] == 0x1f && peek[1] == 0x8b
}

fn copy_gzip_stream<R: Read + Send + 'static>(
    mut stream: R,
    peek: [u8; 2],
    mut out: impl Write,
) -> Result<u64, ResponseError> {
    let mut cmd = Command::new("gzip");
    cmd.arg("-dc");
    let (mut child, mut stdin, mut stdout, mut stderr) =
        crate::process::spawn_stdin_stdout_stderr(&mut cmd).map_err(ResponseError::Io)?;

    let feed = thread::spawn(move || -> io::Result<()> {
        stdin.write_all(&peek)?;
        io::copy(&mut stream, &mut stdin)?;
        Ok(())
    });

    let stderr_join = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let copied = io::copy(&mut stdout, &mut out).map_err(ResponseError::Io)?;

    match feed.join() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = child.wait();
            return Err(ResponseError::Io(e));
        }
        Err(_) => {
            let _ = child.wait();
            return Err(ResponseError::ThreadPanicked);
        }
    }

    let status = child.wait().map_err(ResponseError::Io)?;
    let stderr_bytes = stderr_join.join().unwrap_or_default();
    if !status.success() {
        return Err(ResponseError::GzipFailed {
            exit_code: status.code(),
            stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
        });
    }
    Ok(copied)
}

fn copy_stream_maybe_gunzip<R: Read + Send + 'static>(
    mut stream: R,
    mut out: impl Write,
) -> Result<u64, ResponseError> {
    let mut peek = [0u8; 2];
    let n = stream.read(&mut peek).map_err(ResponseError::Io)?;
    if sniff_is_gzip(n, &peek) {
        return copy_gzip_stream(stream, peek, out);
    }
    out.write_all(&peek[..n]).map_err(ResponseError::Io)?;
    io::copy(&mut stream, &mut out).map_err(ResponseError::Io)
}

impl DownloadSink {
    /// Write the body via a temp file, then persist to `target_path` on [`DownloadSink::finalize_file`].
    pub fn file(tmp_path: TmpFile, target_path: PathBuf) -> Self {
        Self {
            inner: SinkInner::File((Arc::new(Mutex::new(Some(tmp_path))), target_path)),
        }
    }

    /// Accumulate the decompressed body in memory (gzipped payloads are expanded while streaming).
    pub fn buffer(buffer: Arc<Mutex<Vec<u8>>>) -> Self {
        Self {
            inner: SinkInner::Buffer(buffer),
        }
    }

    /// Spawn a thread that reads `stream` into this sink (file or buffer).
    pub(crate) fn spawn_stdout_drain(
        self,
        stream: impl Read + Send + 'static,
    ) -> thread::JoinHandle<Result<u64, ResponseError>> {
        thread::spawn(move || match self.inner {
            SinkInner::File((arc, _)) => {
                let guard = arc.lock().unwrap();
                let tmp = guard
                    .as_ref()
                    .ok_or_else(|| ResponseError::Io(io::Error::other("tmp file missing")))?;
                let mut f = open_body_stream(tmp.as_ref())?;
                copy_stream_maybe_gunzip(stream, &mut f)
            }
            SinkInner::Buffer(buf) => {
                let mut g = buf.lock().unwrap();
                copy_stream_maybe_gunzip(stream, &mut *g)
            }
        })
    }

    /// Rename the temp download to the final path (body is already decompressed if it was gzip).
    pub(crate) fn finalize_file(&self) -> Result<(), ResponseError> {
        let SinkInner::File((arc, target)) = &self.inner else {
            return Ok(());
        };
        let tmp = arc
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| ResponseError::Io(io::Error::other("tmp file already finalized")))?;
        tmp.persist(target).map_err(ResponseError::Io)?;
        Ok(())
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
