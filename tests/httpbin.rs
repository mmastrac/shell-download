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
fn fetch_httpbin_redirect_powershell() {
    fetch_httpbin_redirect(shell_download::Downloader::PowerShell);
}

#[test]
fn fetch_httpbin_redirect_openssl() {
    fetch_httpbin_redirect(shell_download::Downloader::OpenSsl);
}

fn fetch_httpbin_redirect(driver: shell_download::Downloader) {
    let url = "https://httpbin.org/redirect/5";
    let Some(body) = fetch_httpbin(driver, url.to_string()) else {
        return;
    };

    assert!(
        body.contains("\"url\": \"https://httpbin.org/get\""),
        "body did not look like final /get response; got prefix: {:?}",
        body.chars().take(250).collect::<String>()
    );
}

fn fetch_httpbin(driver: shell_download::Downloader, url: String) -> Option<String> {
    let mut out = std::env::temp_dir();
    out.push(unique_name(&format!("shell-download-httpbin-{driver:?}")));

    let handle = shell_download::RequestBuilder::new(url)
        .quiet(shell_download::Quiet::Never)
        .preferred_downloader(driver)
        .start(&out);

    let handle = match handle {
        Ok(h) => h,
        Err(shell_download::StartError::NoDriverFound) => {
            if is_ci() {
                panic!("failed to start downloader in CI");
            }
            return None;
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
    std::fs::remove_file(&out).expect("remove output file");
    Some(body)
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
