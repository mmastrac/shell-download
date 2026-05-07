use std::path::Path;
use std::process::Command;
use std::sync::{
    atomic::AtomicBool,
    Arc,
};

use crate::{drivers::Driver, util, Error, RequestBuilder};

#[derive(Debug, Clone, Copy)]
pub(crate) struct WgetDriver;

impl Driver for WgetDriver {
    fn download(
        &self,
        req: &RequestBuilder,
        out: &Path,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(u16, bool), Error> {
        let mut cmd = Command::new("wget");
        cmd.arg("-O").arg(out).arg("--server-response").arg(&req.url);
        if !req.follow_redirects {
            cmd.arg("--max-redirect=0");
        }
        for (k, v) in util::add_common_headers(req) {
            cmd.arg("--header").arg(format!("{k}: {v}"));
        }

        let output = util::run_cancellable_command(cmd, cancel, "wget", req.quiet)?;
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
        Ok((last_code.unwrap_or(200), false))
    }
}

