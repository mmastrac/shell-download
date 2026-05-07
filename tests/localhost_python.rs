use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dtor::dtor;

static SERVER_CHILD: LazyLock<Mutex<Option<Child>>> = LazyLock::new(|| Mutex::new(None));

static SERVER: LazyLock<Option<Server>> = LazyLock::new(|| start_server());

#[derive(Debug)]
struct Server {
    _root: PathBuf,
    base_url: String,
}

#[dtor(unsafe)]
fn shutdown_server() {
    if let Ok(mut guard) = SERVER_CHILD.lock() {
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn localhost_python_curl() {
    hit_localhost(shell_download::Downloader::Curl);
}

#[test]
fn localhost_python_wget() {
    hit_localhost(shell_download::Downloader::Wget);
}

#[test]
fn localhost_python_pwsh() {
    hit_localhost(shell_download::Downloader::Pwsh);
}

#[test]
fn localhost_python_powershell() {
    hit_localhost(shell_download::Downloader::PowerShell);
}

#[test]
fn localhost_python_fetch() {
    hit_localhost(shell_download::Downloader::Fetch);
}

#[test]
fn localhost_python_openssl() {
    hit_localhost(shell_download::Downloader::OpenSsl);
}

fn hit_localhost(driver: shell_download::Downloader) {
    let Some(server) = SERVER.as_ref() else {
        return; // skip: python3 missing or server couldn't start
    };

    let mut out = std::env::temp_dir();
    out.push(unique_name(&format!("shell-download-localhost-{driver:?}")));

    let url = format!("{}/test-file.html", server.base_url.trim_end_matches('/'));
    let handle = shell_download::RequestBuilder::new(url)
        .preferred_downloader(driver)
        .start(&out);

    let handle = match handle {
        Ok(h) => h,
        Err(shell_download::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return, // skip: tool not installed
        Err(shell_download::Error::NoDownloader) => return,
        Err(e) => panic!("start failed: {e:?}"),
    };

    let resp = match handle.join() {
        Ok(r) => r,
        Err(shell_download::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return, // skip: tool not installed
        Err(shell_download::Error::NoDownloader) => return,
        Err(e) => panic!("download failed: {e:?}"),
    };

    assert!(
        resp.status_code >= 200 && resp.status_code < 400,
        "unexpected status code: {}",
        resp.status_code
    );

    let body = std::fs::read_to_string(&out).expect("read output file");
    assert!(
        body.contains("Hello from python"),
        "body did not contain expected marker; got prefix: {:?}",
        body.chars().take(200).collect::<String>()
    );

    let _ = std::fs::remove_file(&out);
}

fn start_server() -> Option<Server> {
    // Write deterministic content
    let root = make_temp_dir("shell-download-localhost-root")?;
    std::fs::write(root.join("test-file.html"), "Hello from python\n").ok()?;

    let mut child = Command::new("python3")
        .arg("-m")
        .arg("http.server")
        .arg("12121")
        .arg("--bind")
        .arg("127.0.0.1")
        .arg("--directory")
        .arg(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    if !wait_for_port("127.0.0.1", 12121, Duration::from_secs(3)) {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }

    if let Ok(mut guard) = SERVER_CHILD.lock() {
        *guard = Some(child);
    } else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }

    Some(Server {
        _root: root,
        base_url: "http://127.0.0.1:12121".into(),
    })
}

fn wait_for_port(host: &str, port: u16, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if std::net::TcpStream::connect((host, port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn make_temp_dir(prefix: &str) -> Option<PathBuf> {
    let mut p = std::env::temp_dir();
    p.push(unique_name(prefix));
    std::fs::create_dir_all(&p).ok()?;
    Some(p)
}

fn unique_name(prefix: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    PathBuf::from(format!("{prefix}-{}-{}", std::process::id(), now))
}

#[allow(dead_code)]
fn _touch(path: &Path) {
    let _ = std::fs::write(path, "");
}

