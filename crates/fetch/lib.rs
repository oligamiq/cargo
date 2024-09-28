use std::{fs::File, io::Read as _};
use std::os::fd::FromRawFd as _;

#[link(wasm_import_module = "extend_imports")]
extern "C" {
    // open fd
    fn fetch_open(url: *const u8, url_len: u32, method: *const u8, method_len: u32, serialized_headers: *const u8, serialized_headers_len: u32, body: *const u8, body_len: u32) -> i32;
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Fetch failed with status code {0}")]
    Fetch(i32),
    #[error("Timeout")]
    Timeout,
    #[error("Utf8 error: {0}")]
    Utf8(std::string::FromUtf8Error),
}

#[derive(Debug)]
pub struct Response {
    pub status: i32,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn fetch(url: String, method: &str, serialized_headers: Vec<(String, String)>, body: Vec<u8>) -> Result<Response> {
    let url_ptr = url.as_ptr();
    let url_len = url.len() as u32;
    let method_ptr = method.as_ptr();
    let method_len = method.len() as u32;
    let serialized_headers_str = serde_json::to_string(&serialized_headers).unwrap();
    let serialized_headers_ptr = serialized_headers_str.as_ptr();
    let serialized_headers_len = serialized_headers_str.len() as u32;
    let body_ptr = body.as_ptr();
    let body_len = body.len() as u32;

    let fd = unsafe { fetch_open(url_ptr, url_len, method_ptr, method_len, serialized_headers_ptr, serialized_headers_len, body_ptr, body_len) };
    let mut buf = Vec::new();
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.read_to_end(&mut buf).unwrap();
    // close fd
    drop(file);
    // first byte is status code
    let status = i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);

    // second byte is headers byte length
    let headers_len = i32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    let headers_str = String::from_utf8(buf[8..8 + headers_len].to_vec()).unwrap();
    let headers: Vec<(String, String)> = serde_json::from_str(&headers_str).unwrap();

    if status != 200 {
        return Err(Error::Fetch(status));
    }

    String::from_utf8(buf[8 + headers_len..].to_vec())
        .map_err(Error::Utf8)
        .map(|body| Response { status, headers, body: body.into_bytes() })
}
