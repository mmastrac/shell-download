use std::path::PathBuf;

#[test]
fn fetch_httpbin_redirect_curl() {
    fetch_httpbin_redirect(shell_download::Downloader::Curl);
}

#[test]
fn fetch_httpbin_redirect_wget() {
    fetch_httpbin_redirect(shell_download::Downloader::Wget);
}

#[test]
fn fetch_httpbin_redirect_pwsh() {
    fetch_httpbin_redirect(shell_download::Downloader::Pwsh);
}

#[test]
fn fetch_httpbin_redirect_powershell() {
    fetch_httpbin_redirect(shell_download::Downloader::PowerShell);
}

#[test]
fn fetch_httpbin_redirect_openssl() {
    fetch_httpbin_redirect(shell_download::Downloader::OpenSsl);
}

fn fetch_httpbin_redirect(driver: shell_download::Downloader) {
    let mut out = std::env::temp_dir();
    out.push(unique_name(&format!("shell-download-httpbin-{driver:?}")));

    let handle = shell_download::RequestBuilder::new("https://httpbin.org/redirect/5")
        .quiet(shell_download::Quiet::Never)
        .preferred_downloader(driver)
        .start(&out);

    let handle = match handle {
        Ok(h) => h,
        Err(shell_download::StartError::NoDriverFound) => {
            if is_ci() {
                panic!("failed to start downloader in CI");
            }
            return;
        }
        Err(err) => panic!("failed to start: {err:?}"),
    };

    let resp = handle.join().expect("download failed");

    assert!(
        resp.status_code >= 200 && resp.status_code < 400,
        "unexpected status code: {}",
        resp.status_code
    );

    let body = std::fs::read_to_string(&out).expect("read output file");
    assert!(
        body.contains("\"url\": \"https://httpbin.org/get\"")
            || body.contains("\"url\": \"http://httpbin.org/get\"")
            || body.contains("httpbin.org/get"),
        "body did not look like final /get response; got prefix: {:?}",
        body.chars().take(250).collect::<String>()
    );

    let _ = std::fs::remove_file(&out);
}

fn is_ci() -> bool {
    matches!(std::env::var("CI"), Ok(v) if !v.trim().is_empty() && v != "0" && v.to_lowercase() != "false")
}

fn unique_name(prefix: &str) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    PathBuf::from(format!("{prefix}-{}-{}.txt", std::process::id(), now))
}
