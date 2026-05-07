use std::io::{self, Write as _};
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "usage: shell-download [-q] <URL> [-o <FILE|->]\n\
         \n\
         -o <FILE>  Write to file (default: -)\n\
         -o -       Write to stdout\n\
         -q         Quiet (suppress redirect/status output)\n\
         -v         Verbose (show redirect/status output)"
    );
    std::process::exit(2);
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);

    let mut quiet = false;
    let mut verbose = false;
    let mut url: Option<String> = None;
    let mut out: Option<String> = None;

    while let Some(a) = args.next() {
        match a.as_str() {
            "-q" => quiet = true,
            "-v" => verbose = true,
            "-o" => out = args.next().or_else(|| usage()),
            "--help" | "-h" => usage(),
            _ if a.starts_with('-') => usage(),
            _ => {
                if url.is_none() {
                    url = Some(a);
                } else {
                    // unexpected extra positional
                    usage();
                }
            }
        }
    }

    let url = url.unwrap_or_else(|| usage());
    let out = out.unwrap_or_else(|| "-".to_string());

    if out == "-" {
        // Must keep child output quiet to avoid corrupting stdout.
        let bytes = shell_download::RequestBuilder::new(url)
            .fetch_bytes()
            .map_err(|e| format!("{e:?}"))?;
        io::stdout()
            .lock()
            .write_all(&bytes)
            .map_err(|e| format!("{e:?}"))?;
        Ok(())
    } else {
        // When writing to a file, it's safe to forward stderr which often contains redirect traces.
        let q = if !verbose {
            shell_download::Quiet::Always
        } else {
            shell_download::Quiet::Never
        };

        let out_path = PathBuf::from(out);
        let handle = shell_download::RequestBuilder::new(url)
            .quiet(q)
            .start(&out_path)
            .map_err(|e| format!("{e:?}"))?;
        let resp = handle.join().map_err(|e| format!("{e:?}"))?;

        if !quiet {
            eprintln!("shell-download: status_code={}", resp.status_code);
        }
        Ok(())
    }
}

