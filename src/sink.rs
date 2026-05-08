use std::fs::File;
use std::io::{self, Read, Write};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::ResponseError;

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
    File(Arc<Mutex<Option<File>>>),
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
    /// Write the body directly to a file.
    pub fn file(target_file: File) -> Self {
        Self {
            inner: SinkInner::File(Arc::new(Mutex::new(Some(target_file)))),
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
            SinkInner::File(file) => {
                let mut guard = file.lock().unwrap();
                let mut f = guard.take().expect("file already finalized");
                copy_stream_maybe_gunzip(stream, &mut f)
            }
            SinkInner::Buffer(buf) => {
                let mut g = buf.lock().unwrap();
                copy_stream_maybe_gunzip(stream, &mut *g)
            }
        })
    }
}
