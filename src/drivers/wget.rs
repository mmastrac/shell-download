use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{DownloadResult, DownloadSink, RequestBuilder, ResponseError, StartError, drivers::Driver, util};

#[derive(Debug, Clone, Copy)]
/// `wget` backend.
pub(crate) struct WgetDriver;

impl Driver for WgetDriver {
    /// Start a download using `wget`.
    fn start(
        &self,
        req: RequestBuilder,
        sink: DownloadSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError> {
        let mut cmd = Command::new("wget");
        cmd.arg("-O")
            .arg("-")
            .arg("--server-response")
            // Avoid reusing a single TCP connection across redirects; ELBs (e.g. httpbin)
            // sometimes return 502 on a stale keep-alive after a redirect chain.
            .arg("--no-http-keep-alive")
            .arg(&req.url);
        if !req.follow_redirects {
            cmd.arg("--max-redirect=0");
        } else {
            // Some wget builds differ in default redirect behavior; set explicitly.
            cmd.arg("--max-redirect=10");
        }
        for (k, v) in util::add_common_headers(&req) {
            cmd.arg("--header").arg(format!("{k}: {v}"));
        }

        let (child, direct_stdout) = util::spawn_child_for_download(cmd, &sink, "wget")?;

        Ok(util::spawn_download_thread(
            req,
            sink,
            cancel,
            move |req, _sink, cancel| {
                let output = util::wait_child_into_sink(
                    child,
                    _sink,
                    direct_stdout,
                    cancel,
                    "wget",
                    req.quiet,
                )?;
                let stderr = String::from_utf8_lossy(&output.stderr);
                let mut last_code: Option<u16> = None;
                for line in stderr.lines() {
                    let line = line.trim();
                    if let Some(rest) = line.strip_prefix("HTTP/") {
                        let parts: Vec<&str> = rest.split_whitespace().collect();
                        if parts.len() >= 2 {
                            if let Ok(code) = parts[1].parse::<u16>() {
                                last_code = Some(code);
                            }
                        }
                    }
                }
                Ok((last_code.unwrap_or(200), None))
            },
        ))
    }
}
