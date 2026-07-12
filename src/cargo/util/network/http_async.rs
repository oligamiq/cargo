//! Async wrapper around cURL for making managing HTTP requests.
//!
//! On non-WASI targets, requests are executed in parallel using cURL [`Multi`]
//! on a worker thread that is owned by the Client.

#[cfg(not(target_os = "wasi"))]
use std::collections::HashMap;
#[cfg(not(target_os = "wasi"))]
use std::io::Cursor;
#[cfg(not(target_os = "wasi"))]
use std::io::Read;
#[cfg(not(target_os = "wasi"))]
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
#[cfg(not(target_os = "wasi"))]
use std::sync::mpsc;
#[cfg(not(target_os = "wasi"))]
use std::sync::mpsc::Receiver;
#[cfg(not(target_os = "wasi"))]
use std::sync::mpsc::Sender;
#[cfg(not(target_os = "wasi"))]
use std::thread::JoinHandle;
use std::time::Duration;
#[cfg(not(target_os = "wasi"))]
use std::time::Instant;

#[cfg(not(target_os = "wasi"))]
use curl::easy::Easy2;
#[cfg(not(target_os = "wasi"))]
use curl::easy::Handler;
#[cfg(not(target_os = "wasi"))]
use curl::easy::InfoType;
#[cfg(not(target_os = "wasi"))]
use curl::easy::WriteError;
#[cfg(not(target_os = "wasi"))]
use curl::multi::Easy2Handle;
#[cfg(not(target_os = "wasi"))]
use curl::multi::Multi;
#[cfg(not(target_os = "wasi"))]
use futures::channel::oneshot;
use portable_atomic::AtomicI64;
#[cfg(not(target_os = "wasi"))]
use portable_atomic::AtomicU64;
#[cfg(not(target_os = "wasi"))]
use tracing::{debug, error, warn};

use crate::util::network::http::HandleConfiguration;
#[cfg(not(target_os = "wasi"))]
use crate::util::network::http::HttpTimeout;

type Response = http::Response<Vec<u8>>;
type Request = http::Request<Vec<u8>>;
type HttpResult<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[cfg(not(target_os = "wasi"))]
    #[error(transparent)]
    Multi(#[from] curl::MultiError),

    #[cfg(not(target_os = "wasi"))]
    #[error(transparent)]
    Easy(#[from] curl::Error),

    #[cfg(target_os = "wasi")]
    #[error("{0}")]
    Wasi(String),

    #[error(
        "transfer too slow: failed to transfer more than {low_speed_limit} bytes in {}s (transferred {transferred} bytes)",
        timeout_dur.as_secs()
    )]
    TooSlow {
        low_speed_limit: u32,
        timeout_dur: Duration,
        transferred: u64,
    },

    #[error("failed to convert header value of `{name}` to string: {bytes:?}")]
    BadHeader { name: String, bytes: Vec<u8> },
}

#[cfg(not(target_os = "wasi"))]
struct Message {
    easy: Easy2<Collector>,
    sender: oneshot::Sender<HttpResult<Response>>,
}

#[derive(Default)]
struct Stats {
    dl_remaining: AtomicI64,
    #[cfg(not(target_os = "wasi"))]
    dl_transferred: AtomicU64,
}

/// HTTP Client. On non-WASI targets, requests run on a worker thread owned by
/// the client.
pub struct Client {
    #[cfg(not(target_os = "wasi"))]
    channel: Option<Sender<Message>>,
    #[cfg(not(target_os = "wasi"))]
    thread_handle: Option<JoinHandle<()>>,
    #[cfg(not(target_os = "wasi"))]
    handle_config: HandleConfiguration,
    stats: Arc<Stats>,
}

impl Client {
    /// Creates a client, spawning a worker thread on non-WASI targets.
    pub fn new(handle_config: HandleConfiguration) -> Client {
        #[cfg(not(target_os = "wasi"))]
        {
            let (tx, rx) = mpsc::channel();
            let stats = Arc::new(Stats::default());
            let timeout = handle_config.timeout.clone();
            let worker_stats = stats.clone();
            let handle = std::thread::spawn(move || {
                WorkerServer::run(rx, handle_config.multiplexing, timeout, worker_stats)
            });
            Client {
                channel: Some(tx),
                thread_handle: Some(handle),
                handle_config,
                stats,
            }
        }
        #[cfg(target_os = "wasi")]
        {
            let _ = handle_config;
            Client {
                stats: Arc::new(Stats::default()),
            }
        }
    }

    /// Perform a blocking HTTP request using this client.
    /// Does not start an async executor.
    pub fn request_blocking(&self, request: Request) -> HttpResult<Response> {
        #[cfg(not(target_os = "wasi"))]
        {
            let mut handle = self.request_helper(request)?;
            self.handle_config.timeout.configure2(&mut handle)?;
            handle.perform()?;
            Ok(WorkerServer::process_response(handle))
        }
        #[cfg(target_os = "wasi")]
        {
            use crate::util::network::wasi_http::fetch_wasi;
            let method = request.method().to_string();
            let url = request.uri().to_string();
            let headers = collect_wasi_headers(request.headers())?;
            let body = if request.body().is_empty() {
                None
            } else {
                Some(request.body().clone())
            };

            let res = fetch_wasi(&method, &url, headers, body);
            match res {
                Ok((status, headers, body)) => {
                    let mut builder = http::Response::builder().status(status);
                    for (name, value) in headers {
                        builder = builder.header(name, value);
                    }
                    builder.body(body).map_err(|e| Error::Wasi(e.to_string()))
                }
                Err(e) => Err(Error::Wasi(e)),
            }
        }
    }

    /// Perform an HTTP request using this client.
    pub async fn request(&self, request: Request) -> HttpResult<Response> {
        #[cfg(not(target_os = "wasi"))]
        {
            let handle = self.request_helper(request)?;
            let (sender, receiver) = oneshot::channel();
            let req = Message {
                easy: handle,
                sender,
            };
            self.channel.as_ref().unwrap().send(req).unwrap();
            receiver.await.unwrap()
        }
        #[cfg(target_os = "wasi")]
        {
            self.request_blocking(request)
        }
    }

    #[cfg(not(target_os = "wasi"))]
    fn request_helper(&self, request: Request) -> HttpResult<Easy2<Collector>> {
        let url = request.uri().to_string();
        debug!(target: "network::fetch", url);
        let mut collector = Collector::new(self.stats.clone());
        let (parts, body) = request.into_parts();
        let body_len = body.len();
        collector.request_body = Cursor::new(body);
        collector.debug = self.handle_config.verbose;
        let mut handle = curl::easy::Easy2::new(collector);
        self.handle_config.configure2(&mut handle)?;

        handle.url(&url)?;
        handle.follow_location(true)?;
        handle.progress(true)?;

        match parts.method {
            http::Method::HEAD => handle.nobody(true)?,
            http::Method::GET => handle.get(true)?,
            http::Method::POST => {
                handle.post_field_size(body_len as u64)?;
                handle.post(true)?;
            }
            http::Method::PUT => {
                handle.in_filesize(body_len as u64)?;
                handle.put(true)?;
            }
            method => {
                if body_len > 0 {
                    handle.upload(true)?;
                    handle.in_filesize(body_len as u64)?;
                }
                handle.custom_request(method.as_str())?;
            }
        }

        let mut headers = curl::easy::List::new();
        for (name, value) in parts.headers {
            if let Some(name) = name {
                let value: &str = value.to_str().map_err(|_| Error::BadHeader {
                    name: name.to_string(),
                    bytes: value.as_bytes().to_owned(),
                })?;
                headers.append(&format!("{}: {}", name, value))?;
            }
        }
        handle.http_headers(headers)?;

        Ok(handle)
    }

    /// Returns the number pending bytes across all active transfers.
    pub fn bytes_pending(&self) -> u64 {
        self.stats
            .dl_remaining
            .load(Ordering::Acquire)
            .try_into()
            .unwrap()
    }
}

#[cfg(not(target_os = "wasi"))]
impl Drop for Client {
    fn drop(&mut self) {
        // Close the channel
        drop(self.channel.take().unwrap());
        // Join the thread
        let _ = self.thread_handle.take().unwrap().join();
    }
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("http_async::Client").finish()
    }
}

/// Manages HTTP requests.
#[cfg(not(target_os = "wasi"))]
struct WorkerServer {
    /// Channel to receive new work
    incoming_work: Receiver<Message>,
    /// curl multi interface
    multi: Multi,
    /// Map of token to curl handle and response channel
    handles: HashMap<
        usize,
        (
            Easy2Handle<Collector>,
            oneshot::Sender<HttpResult<Response>>,
        ),
    >,
    /// Next token to use
    token: usize,
    /// Global timeout configuration
    timeout: HttpTimeout,
    /// Global transfer statistics
    stats: Arc<Stats>,
    /// Instant when the current low speed window started
    low_speed_window_start: Instant,
    /// Amount of total bytes transferred when the current low speed window started
    low_speed_window_initial: u64,
}

#[cfg(not(target_os = "wasi"))]
impl WorkerServer {
    pub fn run(
        incoming_work: Receiver<Message>,
        multiplex: bool,
        timeout: HttpTimeout,
        stats: Arc<Stats>,
    ) {
        let mut multi = Multi::new();
        if let Err(e) = multi.set_max_host_connections(2) {
            error!("failed to set max host connections in curl: {e}");
        }
        if let Err(e) = multi.pipelining(false, multiplex) {
            error!("failed to enable multiplexing/pipelining in curl: {e}");
        }

        let mut worker = Self {
            incoming_work,
            multi,
            handles: HashMap::new(),
            token: 0,
            timeout,
            stats,
            low_speed_window_start: Instant::now(),
            low_speed_window_initial: 0,
        };
        worker.worker_loop();
    }

    fn fail_and_drain(&mut self, e: &Error) {
        warn!(
            target: "network",
            "failing all outstanding HTTP requests: {e}"
        );
        for (_token, (_handle, sender)) in self.handles.drain() {
            let _ = sender.send(Err(e.clone()));
        }
    }

    fn process_response(mut easy: Easy2<Collector>) -> Response {
        let mut response =
            std::mem::replace(&mut easy.get_mut().response, Response::new(Vec::new()));
        if let Ok(status) = easy.response_code()
            && status != 0
            && let Ok(status) = http::StatusCode::from_u16(status as u16)
        {
            *response.status_mut() = status;
        }
        let extensions = Extensions {
            client_ip: easy.primary_ip().ok().flatten().map(str::to_string),
            effective_url: easy.effective_url().ok().flatten().map(str::to_string),
        };
        response.extensions_mut().insert(extensions);
        response
    }

    fn reset_low_speed_timeout(&mut self) {
        self.low_speed_window_start = Instant::now();
        self.low_speed_window_initial = self.stats.dl_transferred.load(Ordering::Acquire);
    }

    fn check_low_speed_timeout(&mut self) -> Option<Error> {
        if Instant::now().duration_since(self.low_speed_window_start) < self.timeout.dur {
            return None;
        }

        let current = self.stats.dl_transferred.load(Ordering::Acquire);
        let transferred = current.saturating_sub(self.low_speed_window_initial);
        self.reset_low_speed_timeout();
        if transferred < self.timeout.low_speed_limit.into() {
            Some(Error::TooSlow {
                low_speed_limit: self.timeout.low_speed_limit,
                timeout_dur: self.timeout.dur,
                transferred,
            })
        } else {
            None
        }
    }

    fn worker_loop(&mut self) {
        const INITIAL_DELAY: Duration = Duration::from_millis(1);
        let mut wait_backoff = INITIAL_DELAY;
        loop {
            while let Ok(msg) = self.incoming_work.try_recv() {
                self.enqueue_request(msg);
                wait_backoff = INITIAL_DELAY;
            }

            match self.multi.perform() {
                Err(e) if e.is_call_perform() => {}
                Err(e) => {
                    self.fail_and_drain(&Error::Multi(e));
                }
                Ok(running) => {
                    self.multi.messages(|msg| {
                        let t = msg.token().expect("all handles have tokens");
                        let Some((handle, sender)) = self.handles.remove(&t) else {
                            error!("missing entry {t} in handle table");
                            return;
                        };
                        let result = msg.result_for2(&handle).expect("handle must have a result");
                        let easy = self.multi.remove2(handle).expect("handle must be in multi");
                        let response = Self::process_response(easy);
                        let _ = sender.send(result.map(|()| response).map_err(Into::into));
                    });

                    if running > 0 {
                        if let Some(timeout_error) = self.check_low_speed_timeout() {
                            self.fail_and_drain(&timeout_error);
                            continue;
                        }

                        let max_timeout = Duration::from_millis(1000);
                        let mut timeout = self
                            .multi
                            .get_timeout()
                            .ok()
                            .flatten()
                            .unwrap_or(max_timeout)
                            .min(max_timeout);
                        if timeout.is_zero() {
                            continue;
                        }
                        if wait_backoff < timeout {
                            wait_backoff *= 2;
                            timeout = wait_backoff
                        }
                        if let Err(e) = self.multi.wait(&mut [], timeout) {
                            self.fail_and_drain(&Error::Multi(e));
                        }
                    } else {
                        match self.incoming_work.recv() {
                            Ok(msg) => {
                                self.reset_low_speed_timeout();
                                self.enqueue_request(msg);
                                wait_backoff = INITIAL_DELAY;
                            }
                            Err(_) => {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    fn enqueue_request(&mut self, message: Message) {
        match self.multi.add2(message.easy) {
            Ok(mut handle) => {
                self.token = self.token.wrapping_add(1);
                handle.set_token(self.token).ok();
                self.handles.insert(self.token, (handle, message.sender));
            }
            Err(e) => {
                let _ = message.sender.send(Err(e.into()));
            }
        }
    }
}

#[cfg(not(target_os = "wasi"))]
struct Collector {
    response: Response,
    request_body: Cursor<Vec<u8>>,
    debug: bool,
    global_stats: Arc<Stats>,
    dl_remaining_delta: i64,
}

#[cfg(not(target_os = "wasi"))]
impl Collector {
    fn new(stats: Arc<Stats>) -> Self {
        Collector {
            response: Response::new(Vec::new()),
            request_body: Cursor::new(Vec::new()),
            debug: false,
            global_stats: stats,
            dl_remaining_delta: 0,
        }
    }
}

#[cfg(not(target_os = "wasi"))]
impl Handler for Collector {
    fn write(&mut self, data: &[u8]) -> Result<usize, WriteError> {
        self.response.body_mut().extend_from_slice(data);
        self.global_stats
            .dl_transferred
            .fetch_add(data.len() as u64, Ordering::Release);
        Ok(data.len())
    }

    fn header(&mut self, data: &[u8]) -> bool {
        if let Some((name, value)) = handle_http_header(data)
            && let Ok(name) = http::HeaderName::from_str(name)
            && let Ok(value) = http::HeaderValue::from_str(value)
        {
            self.response.headers_mut().append(name, value);
        }
        true
    }

    fn read(&mut self, data: &mut [u8]) -> Result<usize, curl::easy::ReadError> {
        Ok(self.request_body.read(data).unwrap())
    }

    fn debug(&mut self, kind: InfoType, data: &[u8]) {
        if self.debug {
            super::http::debug(kind, data);
        }
    }

    fn progress(&mut self, dltotal: f64, dlnow: f64, _ultotal: f64, _ulnow: f64) -> bool {
        if dlnow > dltotal {
            return true;
        }
        let dl_total = dltotal as i64;
        let dl_current = dlnow as i64;

        let remaining = dl_total - dl_current;

        self.global_stats
            .dl_remaining
            .fetch_add(remaining - self.dl_remaining_delta, Ordering::Release);
        self.dl_remaining_delta = remaining;
        true
    }
}

#[cfg(not(target_os = "wasi"))]
impl Drop for Collector {
    fn drop(&mut self) {
        self.global_stats
            .dl_remaining
            .fetch_add(-self.dl_remaining_delta, Ordering::Release);
    }
}

#[derive(Clone)]
struct Extensions {
    client_ip: Option<String>,
    effective_url: Option<String>,
}

pub trait ResponsePartsExtensions {
    fn client_ip(&self) -> Option<&str>;
    fn effective_url(&self) -> Option<&str>;
}

impl ResponsePartsExtensions for http::response::Parts {
    fn client_ip(&self) -> Option<&str> {
        self.extensions
            .get::<Extensions>()
            .and_then(|extensions| extensions.client_ip.as_deref())
    }

    fn effective_url(&self) -> Option<&str> {
        self.extensions
            .get::<Extensions>()
            .and_then(|extensions| extensions.effective_url.as_deref())
    }
}

impl ResponsePartsExtensions for Response {
    fn client_ip(&self) -> Option<&str> {
        self.extensions()
            .get::<Extensions>()
            .and_then(|extensions| extensions.client_ip.as_deref())
    }

    fn effective_url(&self) -> Option<&str> {
        self.extensions()
            .get::<Extensions>()
            .and_then(|extensions| extensions.effective_url.as_deref())
    }
}

#[cfg(any(target_os = "wasi", test))]
fn collect_wasi_headers(headers: &http::HeaderMap) -> HttpResult<Vec<(String, String)>> {
    headers
        .iter()
        .map(|(name, value)| {
            let value = value.to_str().map_err(|_| Error::BadHeader {
                name: name.to_string(),
                bytes: value.as_bytes().to_owned(),
            })?;
            Ok((name.to_string(), value.to_string()))
        })
        .collect()
}

#[cfg(any(target_os = "wasi", test))]
#[cfg_attr(target_os = "wasi", allow(dead_code))]
fn target_uses_http_worker() -> bool {
    cfg!(not(target_os = "wasi"))
}

#[cfg(test)]
mod tests {
    use super::{Error, collect_wasi_headers, target_uses_http_worker};

    #[test]
    fn wasi_headers_reject_non_utf8_values() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::HeaderName::from_static("x-invalid"),
            http::HeaderValue::from_bytes(b"\xff").unwrap(),
        );

        match collect_wasi_headers(&headers) {
            Err(Error::BadHeader { name, bytes }) => {
                assert_eq!(name, "x-invalid");
                assert_eq!(bytes, b"\xff");
            }
            result => panic!("expected BadHeader, got {result:?}"),
        }
    }

    #[test]
    fn current_target_selects_expected_http_worker_mode() {
        #[cfg(target_os = "wasi")]
        assert!(!target_uses_http_worker());
        #[cfg(not(target_os = "wasi"))]
        assert!(target_uses_http_worker());
    }

    #[test]
    fn wasi_request_dispatches_directly() {
        let source = include_str!("http_async.rs").replace("\r\n", "\n");
        let request = source
            .split_once("pub async fn request")
            .unwrap()
            .1
            .split_once("fn request_helper")
            .unwrap()
            .0;
        let wasi = request
            .split_once("#[cfg(target_os = \"wasi\")]")
            .unwrap()
            .1;

        assert!(wasi.contains("self.request_blocking(request)"));
        assert!(!wasi.contains("self.channel"));
    }
}

#[cfg(not(target_os = "wasi"))]
fn handle_http_header(buf: &[u8]) -> Option<(&str, &str)> {
    if buf.is_empty() {
        return None;
    }
    let buf = std::str::from_utf8(buf).ok()?.trim_end();
    if buf.contains('\n') {
        return None;
    }
    let (tag, value) = buf.split_once(':')?;
    let value = value.trim();
    Some((tag, value))
}
