# shell-download

| crate          | docs                                                                               | version                                                                                                 |
| -------------- | ---------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| `shell-download`     | [![docs.rs](https://docs.rs/shell-download/badge.svg)](https://docs.rs/shell-download)           | [![crates.io](https://img.shields.io/crates/v/shell-download.svg)](https://crates.io/crates/shell-download)       |

A zero-dependency Rust library for downloading a URL to a file by delegating to
whatever download tools are available on the current system.

It hunts for (in order): `curl`, `wget`, `pwsh`/`powershell`, and finally
`openssl` (HTTPS via `openssl s_client`, HTTP via a raw TCP GET).

- The caller provides a target path.
- That file is unlinked before the request starts.
- The response body is written to that path (decompressed if the response is gzip).
- The request runs in a background thread and returns a cancellable handle.

## Features

By default, these features are not enabled, making the crate zero-dependency.

 - `url`: Parse URLs using the `url` crate (adds a number of dependencies, but
   makes URL parsing more accurate).
 - `in-memory`: Receive the response body as a String or Vec<u8> instead of
   requiring a path. Uses the `tempfile` crate.

## Usage

```rust
use shell_download::{Downloader, RequestBuilder};

fn main() -> Result<(), shell_download::Error> {
    let out = std::path::Path::new("downloaded.json");

    let handle = RequestBuilder::new("https://httpbin.org/redirect/5")
        .header("User-Agent", "shell-download/0.1")
        .follow_redirects(true)
        // Optional: force a specific backend (useful for tests)
        .preferred_downloader(Downloader::Curl)
        .start(out)?;

    // Optional: cancel from another thread if you need to abort
    // handle.cancel();

    let response = handle.join()?;
    println!("status={}", response.status_code);

    let body = std::fs::read_to_string(out).unwrap();
    assert!(body.contains("httpbin.org/get"));

    Ok(())
}
