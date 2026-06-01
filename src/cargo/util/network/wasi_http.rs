//! Bridge for WASI using `extern "C"` imports to provide HTTP and Git functionality.

#[cfg(target_os = "wasi")]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Patch,
}

#[cfg(target_os = "wasi")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    /// Fetches a URL.
    ///
    /// The host allocates the response buffer and returns a pointer to it.
    /// The format of the returned data needs to be parsed by the guest.
    /// Returns 0 on success, non-zero on error.
    fn wasi_ext_fetch(
        method_ptr: *const u8,
        method_len: usize,
        url_ptr: *const u8,
        url_len: usize,
        headers_ptr: *const u8,
        headers_len: usize,
        body_ptr: *const u8,
        body_len: usize,
        out_status: *mut u16,
        out_resp_ptr: *mut *mut u8,
        out_resp_len: *mut usize,
    ) -> i32;

    fn wasi_ext_git_clone(
        url_ptr: *const u8,
        url_len: usize,
        dest_ptr: *const u8,
        dest_len: usize,
    ) -> i32;

    fn wasi_ext_git_fetch(
        path_ptr: *const u8,
        path_len: usize,
    ) -> i32;
}

#[cfg(target_os = "wasi")]
pub fn fetch_wasi(
    method: &str,
    url: &str,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
) -> Result<(u16, Vec<(String, String)>, Vec<u8>), String> {
    use std::io::Write;

    // Serialize headers into a simple key:value\n format
    let mut header_buf = Vec::new();
    for (k, v) in headers {
        write!(header_buf, "{}:{}\n", k, v).unwrap();
    }

    let (body_ptr, body_len) = match &body {
        Some(b) => (b.as_ptr(), b.len()),
        None => (std::ptr::null(), 0),
    };

    let mut out_status: u16 = 0;
    let mut out_resp_ptr: *mut u8 = std::ptr::null_mut();
    let mut out_resp_len: usize = 0;

    let res = unsafe {
        wasi_ext_fetch(
            method.as_ptr(),
            method.len(),
            url.as_ptr(),
            url.len(),
            header_buf.as_ptr(),
            header_buf.len(),
            body_ptr,
            body_len,
            &mut out_status,
            &mut out_resp_ptr,
            &mut out_resp_len,
        )
    };

    if res != 0 {
        return Err(format!("wasi_ext_fetch failed with code {}", res));
    }

    let resp_bytes = unsafe { Vec::from_raw_parts(out_resp_ptr, out_resp_len, out_resp_len) };

    // Parse the response back: status \n header:val\n \n body
    // (A very naive parser just for the bridge)
    let mut parts = resp_bytes.splitn(2, |&b| b == b'\n');
    let _status_line = parts.next().unwrap_or(&[]); // We already have out_status
    let rest = parts.next().unwrap_or(&[]);

    let mut parts = rest.splitn(2, |&b| b == b'\n'); // Empty line separates headers from body
    // Let's do a slightly better parse
    let mut parsed_headers = Vec::new();
    let mut parsed_body = Vec::new();

    let mut lines = rest.split(|&b| b == b'\n');
    let mut in_body = false;
    for line in lines {
        if in_body {
            parsed_body.extend_from_slice(line);
            parsed_body.push(b'\n'); // Keep newlines in body
        } else if line.is_empty() {
            in_body = true;
        } else {
            if let Ok(s) = std::str::from_utf8(line) {
                if let Some((k, v)) = s.split_once(':') {
                    parsed_headers.push((k.to_string(), v.to_string()));
                }
            }
        }
    }
    // Remove the trailing newline added by the loop if body is present
    if in_body && !parsed_body.is_empty() {
        parsed_body.pop();
    }

    Ok((out_status, parsed_headers, parsed_body))
}

#[cfg(target_os = "wasi")]
pub fn wasi_git_clone(url: &str, dest: &str, _options: Option<()>) -> Result<(), String> {
    let res = unsafe {
        wasi_ext_git_clone(
            url.as_ptr(),
            url.len(),
            dest.as_ptr(),
            dest.len(),
        )
    };
    if res == 0 {
        Ok(())
    } else {
        Err(format!("wasi_ext_git_clone failed with code {}", res))
    }
}

#[cfg(target_os = "wasi")]
pub fn wasi_git_fetch(repo_path: &str) -> Result<(), String> {
    let res = unsafe {
        wasi_ext_git_fetch(
            repo_path.as_ptr(),
            repo_path.len(),
        )
    };
    if res == 0 {
        Ok(())
    } else {
        Err(format!("wasi_ext_git_fetch failed with code {}", res))
    }
}

#[cfg(target_os = "wasi")]
pub fn wasi_git_ls_remote(_url: &str) -> Result<Vec<(String, String)>, String> {
    // Stub for now, can be implemented similarly if needed
    Ok(Vec::new())
}

#[cfg(target_os = "wasi")]
#[unsafe(no_mangle)]
pub extern "C" fn wasi_ext_allocate(size: usize) -> *mut u8 {
    let mut buf = Vec::with_capacity(size);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}
