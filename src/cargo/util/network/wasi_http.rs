//! Bridge for WASI using wit-bindgen to provide HTTP and Git functionality.

#[cfg(target_os = "wasi")]
wit_bindgen::generate!({
    world: "cargo-bridge",
    path: "wit",
});

#[cfg(target_os = "wasi")]
pub use self::cargo::wasi::http::{Method, Request, fetch as wasi_http_fetch};
#[cfg(target_os = "wasi")]
pub use self::cargo::wasi::git::{CheckoutOptions, clone as wasi_git_clone, fetch as wasi_git_fetch, ls_remote as wasi_git_ls_remote};

#[cfg(target_os = "wasi")]
pub fn fetch_wasi(
    method: &str,
    url: &str,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
) -> Result<(u16, Vec<(String, String)>, Vec<u8>), String> {
    let method = match method.to_lowercase().as_str() {
        "get" => Method::Get,
        "post" => Method::Post,
        "put" => Method::Put,
        "delete" => Method::Delete,
        "head" => Method::Head,
        "patch" => Method::Patch,
        _ => return Err(format!("unsupported method: {}", method)),
    };

    let req = Request {
        method,
        url: url.to_string(),
        headers,
        body,
    };

    match wasi_http_fetch(&req) {
        Ok(res) => Ok((res.status, res.headers, res.body)),
        Err(e) => Err(e),
    }
}
