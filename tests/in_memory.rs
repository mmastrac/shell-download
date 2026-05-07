#![cfg(feature = "in-memory")]

use shell_download::RequestBuilder;

#[test]
fn fetch_string() {
    let url = "https://httpbin.org/get";
    let body = RequestBuilder::new(url).fetch_string().expect("fetch failed");
    assert!(body.contains("\"url\": \"https://httpbin.org/get\""));
}

#[test]
fn fetch_bytes() {
    let url = "https://httpbin.org/get";
    let body = RequestBuilder::new(url).fetch_bytes().expect("fetch failed");
    assert!(
        body.windows(b"\"url\": \"https://httpbin.org/get\"".len())
            .any(|w| w == b"\"url\": \"https://httpbin.org/get\"")
    );
}
