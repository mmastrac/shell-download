use std::path::PathBuf;
use std::sync::LazyLock;

use serde_json::Value;

static HTTPBIN_BASE: LazyLock<String> = LazyLock::new(|| {
    std::env::var("SHELL_DOWNLOAD_HTTPBIN")
        .unwrap_or_else(|_| "https://httpbin.org".to_string())
        .trim()
        .trim_end_matches('/')
        .to_string()
});

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
    let base = HTTPBIN_BASE.as_str();
    let url = format!("{base}/redirect/5");
    let Some(body) = fetch_httpbin(driver, url) else {
        return;
    };

    let want = format!("{base}/get");
    assert_httpbin_url_field(&body, &want, "final /get response");
}

fn httpbin_test_get_tough_chars(driver: shell_download::Downloader) {
    let base = HTTPBIN_BASE.as_str();
    let path = "anything/foo$%25?!&1\"'\\";
    let url = format!("{base}/{path}");
    let Some(body) = fetch_httpbin(driver, url) else {
        return;
    };

    let want = format!("{base}/{path}");
    assert_httpbin_url_field_allow_pct25_echo(&body, &want, "/anything response");
}

fn fetch_httpbin(driver: shell_download::Downloader, url: String) -> Option<String> {
    fetch_httpbin_with(driver, url, true, |status| (200..400).contains(&status))
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
    let base = HTTPBIN_BASE.as_str();
    let url = format!("{base}/redirect/2");
    let mut out = std::env::temp_dir();
    out.push(unique_name(&format!(
        "shell-download-httpbin-follow-off-{driver:?}"
    )));

    let handle = shell_download::RequestBuilder::new(url)
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

    let not_final_url = format!("{base}/get");
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
                !httpbin_response_url_matches(&body, &not_final_url),
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
    let base = HTTPBIN_BASE.as_str();
    let url = format!("{base}/status/204");
    let Some(body) = fetch_httpbin_with(driver, url, true, |s| s == 204) else {
        return;
    };

    assert!(
        body.trim().is_empty(),
        "expected empty body for 204; got prefix: {:?}",
        body.chars().take(250).collect::<String>()
    );
}

fn httpbin_test_gzip(driver: shell_download::Downloader) {
    let base = HTTPBIN_BASE.as_str();
    let url = format!("{base}/gzip");
    let Some(body) = fetch_httpbin(driver, url) else {
        return;
    };

    assert_httpbin_gzip_field(&body);
}

fn is_ci() -> bool {
    matches!(std::env::var("CI"), Ok(v) if !v.trim().is_empty() && v != "0" && v.to_lowercase() != "false")
}

/// Parse JSON, then round-trip through serialize/parse so comparisons match serde's normalized view.
fn json_roundtrip(v: &Value) -> Value {
    let s = serde_json::to_string(v).expect("serde_json serialize");
    serde_json::from_str(&s).expect("serde_json re-parse")
}

/// `{"url": ...}` with `url` embedded using JSON string rules (same as a raw literal + parse).
fn expected_httpbin_url_document(url: &str) -> Value {
    let encoded = serde_json::to_string(url).expect("encode url as JSON string");
    let raw = format!("{{\"url\":{encoded}}}");
    serde_json::from_str(&raw).expect("minimal httpbin url document")
}

fn httpbin_url_field_eq(body: &str, expected_url: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let actual = json_roundtrip(&v);
    let expected = json_roundtrip(&expected_httpbin_url_document(expected_url));
    actual.get("url") == expected.get("url")
}

fn assert_httpbin_url_field(body: &str, expected_url: &str, ctx: &str) {
    serde_json::from_str::<Value>(body).unwrap_or_else(|e| {
        panic!(
            "{ctx}: invalid JSON ({e}); prefix {:?}",
            body.chars().take(250).collect::<String>()
        )
    });
    assert!(
        httpbin_url_field_eq(body, expected_url),
        "{ctx}: url field; prefix {:?}",
        body.chars().take(250).collect::<String>()
    );
}

/// Werkzeug sometimes echoes `%` where the request had `%25` in the path (depends on stack).
///
/// This is not `std::process::Command` escaping: arguments are passed without a shell, and the same
/// mismatch shows up for curl, wget, OpenSSL, and PowerShell. Linux CI usually runs the Docker
/// `kennethreitz/httpbin` image; macOS/Windows use `pip install httpbin` + Waitress—different
/// httpbin/Werkzeug versions rebuild `request.url` for `/anything` JSON slightly differently.
fn assert_httpbin_url_field_allow_pct25_echo(body: &str, expected_url: &str, ctx: &str) {
    serde_json::from_str::<Value>(body).unwrap_or_else(|e| {
        panic!(
            "{ctx}: invalid JSON ({e}); prefix {:?}",
            body.chars().take(250).collect::<String>()
        )
    });
    if httpbin_url_field_eq(body, expected_url) {
        return;
    }
    if expected_url.contains("%25") {
        let alt = expected_url.replace("%25", "%");
        if httpbin_url_field_eq(body, &alt) {
            return;
        }
    }
    panic!(
        "{ctx}: url field; wanted {expected_url:?} (or %25→% echo); prefix {:?}",
        body.chars().take(250).collect::<String>()
    );
}

fn httpbin_response_url_matches(body: &str, want: &str) -> bool {
    httpbin_url_field_eq(body, want)
}

fn assert_httpbin_gzip_field(body: &str) {
    let expected: Value =
        serde_json::from_str(r#"{"gzipped":true}"#).expect("static gzip expectation literal");
    let expected = json_roundtrip(&expected);
    let actual = json_roundtrip(
        &serde_json::from_str(body).unwrap_or_else(|e| {
            panic!(
                "/gzip: invalid JSON ({e}); prefix {:?}",
                body.chars().take(250).collect::<String>()
            )
        }),
    );
    assert_eq!(
        actual.get("gzipped"),
        expected.get("gzipped"),
        "/gzip: gzipped field; prefix {:?}",
        body.chars().take(250).collect::<String>()
    );
}

fn unique_name(prefix: &str) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    PathBuf::from(format!("{prefix}-{}-{}.txt", std::process::id(), now))
}
