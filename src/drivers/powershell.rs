use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{DownloadResult, DownloadSink, RequestBuilder, ResponseError, StartError, drivers::Driver, util};

#[derive(Debug, Clone, Copy)]
/// PowerShell (`pwsh`/`powershell`) backend.
pub(crate) struct PowerShellDriver;

impl Driver for PowerShellDriver {
    /// Start a download using PowerShell.
    fn start(
        &self,
        req: RequestBuilder,
        sink: DownloadSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<
        JoinHandle<Result<DownloadResult, ResponseError>>,
        (StartError, DownloadSink),
    > {
        start_inner(req, sink, cancel)
    }
}

const PS_STATUS_PREFIX: &str = "shell-download_status:";

/// PowerShell from `$response` through `$sc` (status line is spliced next — keep `WriteLine` args out of the raw block).
const PS_HTTP_TRY_HEAD: &str = r#"
$response=$null;
$client=$null;
$exitCode=1;
try {
  $handler=New-Object System.Net.Http.HttpClientHandler;
  if ($mr -gt 0) {
    $handler.AllowAutoRedirect=$true;
    try { $handler.MaxAutomaticRedirections=$mr } catch { }
  } else {
    $handler.AllowAutoRedirect=$false
  };
  $handler.AutomaticDecompression=[System.Net.DecompressionMethods]::GZip -bor [System.Net.DecompressionMethods]::Deflate;
  $client=New-Object System.Net.Http.HttpClient($handler);
  foreach ($e in $h.GetEnumerator()) { [void]$client.DefaultRequestHeaders.TryAddWithoutValidation([string]$e.Key,[string]$e.Value) };
  try {
    $uriOpts=[System.UriCreationOptions]::new();
    $uriOpts.DangerousDisablePathAndQueryCanonicalization=$true;
    $uri=[System.Uri]::new($u,$uriOpts)
  } catch {
    $uri=New-Object System.Uri($u)
  };
  $response=$client.GetAsync($uri,[System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult();
  $sc=[int]$response.StatusCode;
"#;

/// Remainder: body stream to stdout, catch/finally/exit.
const PS_HTTP_TRY_TAIL: &str = r#"
  $in=$response.Content.ReadAsStreamAsync().GetAwaiter().GetResult();
  $outFs=[System.Console]::OpenStandardOutput();
  try {
    $in.CopyTo($outFs);
    $outFs.Flush();
    $exitCode=0;
  } finally {
    if ($null -ne $in) { $in.Dispose() };
  }
} catch {
  [Console]::Error.WriteLine("shell-download(powershell): request failed");
  [Console]::Error.WriteLine($_.ToString());
} finally {
  if ($null -ne $response) { $response.Dispose() };
  if ($null -ne $client) { $client.Dispose() };
};
exit $exitCode;
"#;

/// Implementation for the PowerShell backend.
fn start_inner(
    req: RequestBuilder,
    sink: DownloadSink,
    cancel: Arc<AtomicBool>,
) -> Result<
    JoinHandle<Result<DownloadResult, ResponseError>>,
    (StartError, DownloadSink),
> {
    let candidates = find_powershell_candidates();
    if candidates.is_empty() {
        return Err((StartError::NoDriverFound, sink));
    }

    let mut ps_headers = String::new();
    for (k, v) in util::add_common_headers(&req) {
        ps_headers.push('\'');
        ps_headers.push_str(&escape_ps(&k));
        ps_headers.push_str("'='");
        ps_headers.push_str(&escape_ps(&v));
        ps_headers.push_str("';");
    }
    let mut headers_expr = String::with_capacity(ps_headers.len() + 2);
    headers_expr.push_str("@{");
    headers_expr.push_str(&ps_headers);
    headers_expr.push('}');
    let url = escape_ps(&req.url);
    let max_redir = if req.follow_redirects { 10 } else { 0 };

    let mut script = String::new();
    script.push_str("$ProgressPreference='SilentlyContinue';$h=");
    script.push_str(&headers_expr);
    script.push_str(";$u='");
    script.push_str(&url);
    script.push_str("';$mr=");
    script.push_str(&max_redir.to_string());
    script.push(';');
    script.push_str(PS_HTTP_TRY_HEAD);
    script.push_str("[Console]::Error.WriteLine(\"");
    script.push_str(PS_STATUS_PREFIX);
    script.push_str("$sc\");");
    script.push_str(PS_HTTP_TRY_TAIL);

    let mut last_io: Option<std::io::Error> = None;

    let (child, program_label, direct_stdout) = {
        let mut started: Option<(std::process::Child, &'static str, bool)> = None;
        for exe in candidates {
            let mut cmd = std::process::Command::new(exe);
            cmd.arg("-NoProfile")
                .arg("-NonInteractive")
                .arg("-ExecutionPolicy")
                .arg("Bypass")
                .arg("-Command")
                .arg(&script);

            match util::spawn_child_for_download(cmd, &sink, exe) {
                Ok((ch, dir)) => {
                    started = Some((ch, exe, dir));
                    break;
                }
                Err(StartError::NoDriverFound) => {}
                Err(StartError::IoError(e)) => {
                    if last_io.is_none() {
                        last_io = Some(e);
                    }
                }
                Err(StartError::Url(msg)) => return Err((StartError::Url(msg), sink)),
            };
        }

        if let Some(v) = started {
            v
        } else if let Some(e) = last_io {
            return Err((StartError::IoError(e), sink));
        } else {
            return Err((StartError::NoDriverFound, sink));
        }
    };

    Ok(util::spawn_download_thread(
        req,
        sink,
        cancel,
        move |req, sink, cancel| {
            let output = util::wait_child_into_sink(
                child,
                sink,
                direct_stdout,
                cancel,
                program_label,
                req.quiet,
            )?;
            let stderr_str = String::from_utf8_lossy(&output.stderr).to_string();
            let status_line = stderr_str
                .lines()
                .find_map(|line| line.trim().strip_prefix(PS_STATUS_PREFIX).map(str::trim));
            let code_str = status_line.unwrap_or("").to_string();
            let status_code: u16 = code_str
                .parse()
                .map_err(|_| ResponseError::BadStatusCode(code_str))?;

            Ok((status_code, None))
        },
    ))
}

/// Escape a value for a single-quoted PowerShell string.
fn escape_ps(s: &str) -> String {
    s.replace('\'', "''")
}

/// Find PowerShell executables in priority order.
fn find_powershell_candidates() -> Vec<&'static str> {
    let mut out = Vec::new();
    if !util::find_program_in_path("pwsh").is_empty() {
        out.push("pwsh");
    }
    if !util::find_program_in_path("powershell").is_empty() {
        out.push("powershell");
    }
    out
}
