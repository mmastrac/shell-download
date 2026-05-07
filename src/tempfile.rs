/// A simple and reasonably secure stdlib-only alternative if `tempfile` isn't
/// available.
#[cfg(not(feature = "tempfile"))]
mod simple {
    use std::fs::{File, OpenOptions};
    use std::hash::{BuildHasher, Hash, Hasher, RandomState};
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    use crate::url_parser::Url;

    #[derive(Debug)]
    pub(crate) struct TmpFile {
        path: Option<PathBuf>,
    }

    impl TmpFile {
        fn new(path: PathBuf) -> Self {
            Self { path: Some(path) }
        }

        pub(crate) fn persist<P: AsRef<Path>>(self, new_path: P) -> io::Result<File> {
            let mut this = self;
            let new_path = new_path.as_ref();
            let _ = std::fs::remove_file(new_path);
            let src = this.path.as_deref().expect("tmp path present");
            std::fs::rename(src, new_path)?;
            // Once persisted, we no longer own a path to clean up.
            this.path = None;
            File::open(new_path)
        }
    }

    impl AsRef<Path> for TmpFile {
        fn as_ref(&self) -> &Path {
            self.path.as_deref().expect("tmp path present")
        }
    }

    impl Drop for TmpFile {
        fn drop(&mut self) {
            if let Some(p) = &self.path {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    pub(crate) fn create_tmp_file_in_path(
        seed: &str,
        url: Option<&Url>,
        dir: &Path,
        hint: &str,
    ) -> io::Result<TmpFile> {
        // Keep filenames readable while still making them unique.
        let base = if hint.trim().is_empty() {
            "download"
        } else {
            hint
        };

        for attempt in 0u32..200 {
            let hash = create_random_suffix(seed, url, dir, base, attempt);

            let name = format!(".{base}.{hash:x}.tmp");
            let path = dir.join(name);

            let mut opts = OpenOptions::new();
            opts.write(true).create_new(true);

            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                opts.mode(0o600);
            }

            match opts.open(&path) {
                Ok(_file) => return Ok(TmpFile::new(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "failed to create unique temporary download file",
        ))
    }

    fn create_random_suffix(
        seed: &str,
        url: Option<&Url>,
        dir: &Path,
        hint: &str,
        attempt: u32,
    ) -> u64 {
        let mut hasher = RandomState::new().build_hasher();
        seed.hash(&mut hasher);
        attempt.hash(&mut hasher);
        url.hash(&mut hasher);
        dir.hash(&mut hasher);
        hint.hash(&mut hasher);
        let now = SystemTime::now();
        now.hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        std::thread::current().id().hash(&mut hasher);
        hasher.finish()
    }
}

#[cfg(feature = "tempfile")]
mod tf {
    use std::fs::File;
    use std::io;
    use std::path::Path;

    use crate::url_parser::Url;

    #[derive(Debug)]
    pub(crate) struct TmpFile {
        inner: tempfile::NamedTempFile<File>,
    }

    impl AsRef<Path> for TmpFile {
        fn as_ref(&self) -> &Path {
            self.inner.path()
        }
    }

    pub(crate) fn create_tmp_file_in_path(
        _seed: &str,
        _url: Option<&Url>,
        dir: &Path,
        _hint: &str,
    ) -> io::Result<TmpFile> {
        #[cfg(windows)]
        {
            use std::fs::OpenOptions;
            use std::os::windows::fs::OpenOptionsExt as _;

            const FILE_SHARE_READ: u32 = 0x00000001;
            const FILE_SHARE_WRITE: u32 = 0x00000002;

            let ntf = tempfile::Builder::new().make_in(dir, |path| {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                    .open(path)
            })?;
            return Ok(TmpFile { inner: ntf });
        }

        #[cfg(not(windows))]
        {
            let ntf = tempfile::NamedTempFile::new_in(dir)?;
            Ok(TmpFile { inner: ntf })
        }
    }

    impl TmpFile {
        pub(crate) fn persist<P: AsRef<Path>>(self, new_path: P) -> io::Result<File> {
            let new_path = new_path.as_ref();
            let _ = std::fs::remove_file(new_path);
            self.inner.persist(new_path).map_err(Into::into)
        }
    }
}

#[cfg(feature = "tempfile")]
pub(crate) use tf::{TmpFile, create_tmp_file_in_path};

#[cfg(not(feature = "tempfile"))]
pub(crate) use simple::{TmpFile, create_tmp_file_in_path};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use super::create_tmp_file_in_path;

    fn wait_for_gone(path: &PathBuf) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            !path.exists(),
            "expected temp file to be removed on drop: {}",
            path.display()
        );
    }

    #[cfg(unix)]
    fn sh_single_quote(s: &str) -> String {
        // POSIX-safe single-quoting: close, escape, reopen.
        format!("'{}'", s.replace('\'', "'\\''"))
    }

    #[test]
    fn tmpfile_is_deleted_on_drop_after_external_write() {
        let path: PathBuf = {
            let tmp = create_tmp_file_in_path(
                "test",
                None,
                &std::env::temp_dir(),
                "shell-download-tempfile-test",
            )
            .expect("create tmpfile");

            let path = tmp.as_ref().to_path_buf();

            // Spawn an external command that writes to the file by path.
            #[cfg(unix)]
            {
                let cmd = format!("echo hi > {}", sh_single_quote(&path.to_string_lossy()));
                let status = Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .status()
                    .expect("spawn sh");
                assert!(status.success(), "sh command failed");
            }

            #[cfg(windows)]
            {
                // `cmd.exe` quoting is fiddly; wrap the whole command in quotes and avoid spaces around `>`.
                let cmd = format!("\"echo hi>\\\"{}\\\"\"", path.display());
                let out = Command::new("cmd")
                    .arg("/C")
                    .arg(cmd)
                    .output()
                    .expect("spawn cmd");
                assert!(
                    out.status.success(),
                    "cmd command failed: status={:?} stdout={:?} stderr={:?}",
                    out.status.code(),
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
            }

            assert!(path.exists(), "expected temp file to exist after write");
            path
            // `tmp` drops here: should remove the file.
        };

        wait_for_gone(&path);
    }
}
