//! Minimal HTTP over TCP (`http`) and HTTPS via OpenSSL (`openssl s_client`).

use std::sync::{Arc, atomic::AtomicBool};
use std::thread::JoinHandle;

use crate::{
    DownloadResult, DownloadSink, RequestBuilder, ResponseError, StartError, url_parser::Url,
};
use crate::drivers::Driver;

mod http11;
mod openssl;
mod tcp;

pub(crate) use openssl::OpenSslDriver;
pub(crate) use tcp::TcpDriver;

#[derive(Debug, Clone, Copy)]
/// Dispatches to [`TcpDriver`] for `http:` and OpenSSL ([`OpenSslDriver`]) for `https:`.
pub(crate) struct TunnelDriver;

impl Driver for TunnelDriver {
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
        match url.scheme.as_str() {
            "http" => TcpDriver.start(req, sink, cancel),
            "https" => OpenSslDriver.start(req, sink, cancel),
            _ => Err(StartError::NoDriverFound),
        }
    }
}
