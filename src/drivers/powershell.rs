use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{DownloadResult, RequestBuilder, ResponseError, StartError, drivers::Driver, util};

#[derive(Debug, Clone, Copy)]
pub(crate) struct PowerShellDriver;

impl Driver for PowerShellDriver {
    fn start(
        &self,
        req: RequestBuilder,
        out_path: std::path::PathBuf,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError> {
        start_inner(req, out_path, cancel)
    }
}

fn start_inner(
    req: RequestBuilder,
    out_path: std::path::PathBuf,
    cancel: Arc<AtomicBool>,
) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError> {
    let candidates = find_powershell_candidates();
    if candidates.is_empty() {
        return Err(StartError::NoDriverFound);
    }

    let mut ps_headers = String::new();
    for (k, v) in util::add_common_headers(&req) {
        ps_headers.push_str(&format!("'{}'='{}';", escape_ps(&k), escape_ps(&v)));
    }
    let headers_expr = format!("@{{{ps_headers}}}");
    let url = escape_ps(&req.url);
    let out_str = escape_ps(&out_path.to_string_lossy());
    let max_redir = if req.follow_redirects { 10 } else { 0 };

    let script = |use_basic_parsing: bool| {
        let basic = if use_basic_parsing {
            "-UseBasicParsing"
        } else {
            ""
        };
        format!(
            "$ProgressPreference='SilentlyContinue';\
         $h={headers_expr};\
         $u='{url}';\
         $o='{out_str}';\
         $mr={max_redir};\
         try {{\
           $r=Invoke-WebRequest -Uri $u -Headers $h -OutFile $o -PassThru -MaximumRedirection $mr -ErrorAction Stop {basic};\
           $sc=$r.StatusCode;\
           if ($null -eq $sc) {{ $sc=0 }};\
           if ($sc -is [int]) {{ [Console]::Out.Write($sc) }} else {{ [Console]::Out.Write($sc.value__) }};\
           exit 0;\
         }} catch {{\
           [Console]::Error.WriteLine(\"shell-download(powershell): request failed\");\
           [Console]::Error.WriteLine($_.ToString());\
           exit 1;\
         }}",
            basic = basic
        )
    };

    let mut last_io: Option<std::io::Error> = None;
    let mut saw_not_found = false;

    // Try pwsh first, then powershell; if the first fails to spawn for some reason,
    // try the next executable.
    let (child, program_label) = {
        let mut started: Option<(std::process::Child, &'static str)> = None;
        for c in candidates {
            let (exe, use_basic_parsing) = c;
            let mut cmd = Command::new(exe);
            cmd.arg("-NoProfile")
                .arg("-NonInteractive")
                .arg("-ExecutionPolicy")
                .arg("Bypass")
                .arg("-Command")
                .arg(script(use_basic_parsing));

            match util::spawn_child_for_output(cmd, exe) {
                Ok(ch) => {
                    started = Some((ch, exe));
                    break;
                }
                Err(StartError::NoDriverFound) => {
                    saw_not_found = true;
                    continue;
                }
                Err(StartError::IoError(e)) => {
                    if last_io.is_none() {
                        last_io = Some(e);
                    }
                    continue;
                }
                Err(StartError::Url(msg)) => return Err(StartError::Url(msg)),
            };
        }

        if let Some(v) = started {
            v
        } else if let Some(e) = last_io {
            return Err(StartError::IoError(e));
        } else if saw_not_found {
            return Err(StartError::NoDriverFound);
        } else {
            return Err(StartError::NoDriverFound);
        }
    };

    Ok(util::spawn_download_thread(
        req,
        out_path,
        cancel,
        move |req, _out, cancel| {
            let output = util::wait_child_with_output(child, cancel, program_label, req.quiet)?;
            let code_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let code: u16 = code_str
                .parse()
                .map_err(|_| ResponseError::BadStatusCode(code_str))?;
            Ok((code, false))
        },
    ))
}

fn escape_ps(s: &str) -> String {
    s.replace('\'', "''")
}

// Returns ordered (exe, use_basic_parsing) candidates.
fn find_powershell_candidates() -> Vec<(&'static str, bool)> {
    // Prefer pwsh if present.
    let mut out = Vec::new();
    if !util::find_program_in_path("pwsh").is_empty() {
        out.push(("pwsh", false));
    }
    if !util::find_program_in_path("powershell").is_empty() {
        // Windows PowerShell needs -UseBasicParsing for older versions.
        out.push(("powershell", true));
    }
    out
}
