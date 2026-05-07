use std::path::PathBuf;

#[test]
fn fetch_httpbin_redirect_curl() {
    httpbin_test(shell_download::Downloader::Curl);
}

#[test]
fn fetch_httpbin_redirect_wget() {
    httpbin_test(shell_download::Downloader::Wget);
}

#[test]
fn fetch_httpbin_redirect_powershell() {
    httpbin_test(shell_download::Downloader::PowerShell);
}

#[test]
fn fetch_httpbin_redirect_openssl() {
    httpbin_test(shell_download::Downloader::OpenSsl);
}

fn httpbin_test(driver: shell_download::Downloader) {
    httpbin_test_redirect(driver);
    httpbin_test_get_tough_chars(driver);
    httpbin_test_redirect_follow_off(driver);
    httpbin_test_custom_status(driver);
    httpbin_test_gzip(driver);
}

fn httpbin_test_redirect(driver: shell_download::Downloader) {
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

fn httpbin_test_get_tough_chars(driver: shell_download::Downloader) {
    let url = r#"https://httpbin.org/anything/foo$%25?!&1"'\"#;
    let Some(body) = fetch_httpbin(driver, url.to_string()) else {
        return;
    };

    assert!(
        body.contains(r#""url": "https://httpbin.org/anything/foo$%25?!&1\"'\\""#),
        "body did not look like final /get response; got prefix: {:?}",
        body.chars().take(250).collect::<String>()
    );
}

fn fetch_httpbin(driver: shell_download::Downloader, url: String) -> Option<String> {
    fetch_httpbin_with(driver, url, true, |status| status >= 200 && status < 400)
}

fn fetch_httpbin_with(
    driver: shell_download::Downloader,
    url: String,
    follow_redirects: bool,
    ok_status: impl FnOnce(u16) -> bool,
) -> Option<String> {
    let (body, status_code) = fetch_httpbin_raw(driver, url, follow_redirects)?;

    assert!(
        ok_status(status_code),
        "unexpected status code: {}",
        status_code
    );

    Some(body)
}

fn fetch_httpbin_raw(
    driver: shell_download::Downloader,
    url: String,
    follow_redirects: bool,
) -> Option<(String, u16)> {
    let mut out = std::env::temp_dir();
    out.push(unique_name(&format!("shell-download-httpbin-{driver:?}")));

    let handle = shell_download::RequestBuilder::new(url)
        .quiet(shell_download::Quiet::Never)
        .preferred_downloader(driver)
        .follow_redirects(follow_redirects)
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

    let resp = match handle.join() {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&out);
            panic!("download failed: {e:?}");
        }
    };

    let body = std::fs::read_to_string(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    Some((body, resp.status_code))
}

fn httpbin_test_redirect_follow_off(driver: shell_download::Downloader) {
    let url = "https://httpbin.org/redirect/5";
    let mut out = std::env::temp_dir();
    out.push(unique_name(&format!(
        "shell-download-httpbin-follow-off-{driver:?}"
    )));

    let handle = shell_download::RequestBuilder::new(url.to_string())
        .quiet(shell_download::Quiet::Never)
        .preferred_downloader(driver)
        .follow_redirects(false)
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

    match handle.join() {
        Ok(resp) => {
            assert!(
                resp.status_code >= 300 && resp.status_code < 400,
                "expected 3xx when redirects are disabled; got {}",
                resp.status_code
            );

            let body = std::fs::read_to_string(&out).unwrap_or_default();
            let _ = std::fs::remove_file(&out);
            assert!(
                !body.contains("\"url\": \"https://httpbin.org/get\""),
                "expected not to follow redirects; got body prefix: {:?}",
                body.chars().take(250).collect::<String>()
            );
        }
        Err(shell_download::ResponseError::CommandFailed {
            program, stderr, ..
        }) => {
            let _ = std::fs::remove_file(&out);
            // `wget` and PowerShell treat "redirects disabled / max redirects exceeded" as an error exit.
            assert!(
                stderr.to_ascii_lowercase().contains("redirection")
                    || stderr.to_ascii_lowercase().contains("redirecting"),
                "expected a redirect-related failure when redirects are disabled; program={program} stderr={stderr:?}"
            );
        }
        Err(e) => {
            let _ = std::fs::remove_file(&out);
            panic!("unexpected error: {e:?}");
        }
    }
}

fn httpbin_test_custom_status(driver: shell_download::Downloader) {
    // Use a custom status endpoint that most tools treat as successful (no 4xx/5xx error exit).
    let url = "https://httpbin.org/status/204";
    let Some(body) = fetch_httpbin_with(driver, url.to_string(), true, |s| s == 204) else {
        return;
    };

    assert!(
        body.trim().is_empty(),
        "expected empty body for 204; got prefix: {:?}",
        body.chars().take(250).collect::<String>()
    );
}

fn httpbin_test_gzip(driver: shell_download::Downloader) {
    // httpbin endpoint that returns a gzip-compressed JSON response.
    let url = "https://httpbin.org/gzip";
    let Some(body) = fetch_httpbin(driver, url.to_string()) else {
        return;
    };

    // If we didn't decode gzip, this would be binary and not contain JSON markers.
    assert!(
        body.contains("\"gzipped\": true"),
        "body did not look like decoded /gzip response; got prefix: {:?}",
        body.chars().take(250).collect::<String>()
    );
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
