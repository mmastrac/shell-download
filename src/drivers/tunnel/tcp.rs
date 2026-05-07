use std::io::Write as _;
use std::net::TcpStream;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

use crate::{
    ContentEncoding, DownloadResult, DownloadSink, RequestBuilder, ResponseError, StartError,
    drivers::Driver, url_parser::Url, util,
};

use super::http11;

#[derive(Debug, Clone, Copy)]
/// Plain HTTP/1.1 over a TCP socket (no TLS).
pub(crate) struct TcpDriver;

impl Driver for TcpDriver {
    fn start(
        &self,
        req: RequestBuilder,
        sink: DownloadSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<JoinHandle<Result<DownloadResult, ResponseError>>, StartError> {
        let url = match Url::new(&req.url) {
            Ok(u) => u,
            Err(_) => return Err(StartError::NoDriverFound),
        };
        if url.scheme != "http" {
            return Err(StartError::NoDriverFound);
        }

        Ok(util::spawn_download_thread(
            req,
            sink,
            cancel,
            download_http,
        ))
    }
}

fn download_http(
    req: &RequestBuilder,
    sink: &DownloadSink,
    cancel: &Arc<AtomicBool>,
) -> Result<(u16, Option<ContentEncoding>), ResponseError> {
    http11::redirect_download(req, sink, cancel, fetch_http_only)
}

fn fetch_http_only(
    url: &Url,
    req: &RequestBuilder,
    cancel: &Arc<AtomicBool>,
) -> Result<http11::HttpResponseParts, ResponseError> {
    if url.scheme != "http" {
        return Err(ResponseError::UnsupportedScheme);
    }

    let request = http11::build_get_request(url, req);
    if cancel.load(Ordering::SeqCst) {
        return Err(ResponseError::Cancelled);
    }

    let host = url.host.as_str();
    let port = url.port.unwrap_or(80);
    let mut stream = TcpStream::connect((host, port)).map_err(ResponseError::Io)?;
    stream
        .write_all(request.as_bytes())
        .map_err(ResponseError::Io)?;
    stream.flush().map_err(ResponseError::Io)?;

    let buf = http11::read_to_vec_cancelled(&mut stream, cancel)?;
    http11::parse_http_response(&buf)
}
