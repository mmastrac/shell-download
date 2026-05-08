# shell-download

| crate          | docs                                                                               | version                                                                                                 |
| -------------- | ---------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| `shell-download`     | [![docs.rs](https://docs.rs/shell-download/badge.svg)](https://docs.rs/shell-download)           | [![crates.io](https://img.shields.io/crates/v/shell-download.svg)](https://crates.io/crates/shell-download)       |

A zero-dependency Rust library for downloading a URL to a file by delegating to
whatever download tools are available on the current system. <sup>❡</sup>

<sup>❡</sup> By default, `tempfile` is enabled for secure temporary file creation. Disable it with `default-features = false`.

It hunts for (in order): `curl`, `wget`, `pwsh`/`powershell`, `python3`, then the
built-in **tunnel** (HTTP over TCP, HTTPS via OpenSSL / `openssl s_client`). You can still
force only the TCP or only the OpenSSL stack with [`Downloader::Tcp`] and
[`Downloader::OpenSSL`].

- The caller provides a target path.
- That file is unlinked before the request starts.
- The response body is written to that path (decompressed if the response is gzip).
- The request runs in a background thread and returns a cancellable handle.

## When to use this crate

If you can afford the compile-time cost and binary size of a much
fuller-featured crate such as `reqwest`, that's probably a better choice. But if
you need a small, zero-dependency library for just downloading a file, and you'd
prefer not to add code to call into `curl` or `wget` or `pwsh` or whatever
happens to be installed on a random machine, this crate is a good choice.

## Features

By default, these features are not enabled, making the crate zero-dependency.

 - `url`: Parse URLs using the `url` crate (adds a number of dependencies, but
   makes URL parsing more accurate).

## Usage

```rust
use shell_download::{Downloader, RequestBuilder};

fn main() -> Result<(), shell_download::ResponseError> {
    // NOTE: This should use a secure temporary file name!
    let out = std::env::temp_dir().join("downloaded.json");

    let handle = RequestBuilder::new("https://httpbin.org/redirect/5")
        .header("User-Agent", "shell-download/0.1")
        .follow_redirects(true)
        // Optional: force a specific backend (useful for tests)
        .preferred_downloader(Downloader::Curl)
        .start(&out)
        .map_err(shell_download::ResponseError::Start)?;

    // Optional: cancel from another thread if you need to abort
    // handle.cancel();

    let response = handle.join()?;
    println!("status={}", response.status_code);

    let body = std::fs::read_to_string(out).unwrap();
    assert!(body.contains("httpbin.org/get"));

    Ok(())
}
```
