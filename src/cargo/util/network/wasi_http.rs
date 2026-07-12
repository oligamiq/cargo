//! Bridge for WASI using `extern "C"` imports to provide HTTP and Git functionality.

#[cfg(any(target_os = "wasi", test))]
fn empty_stdin() -> &'static [u8] {
    &b"\0"[..0]
}

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
#[link(wasm_import_module = "__wasip1_vfs-host")]
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

    fn wasi_ext_git_fetch(path_ptr: *const u8, path_len: usize) -> i32;

    fn wasi_ext_spawn(
        program_ptr: *const u8,
        program_len: usize,
        args_ptr: *const u8,
        args_len: usize,
        env_ptr: *const u8,
        env_len: usize,
        cwd_ptr: *const u8,
        cwd_len: usize,
        stdin_ptr: *const u8,
        stdin_len: usize,
        out_exit_code: *mut i32,
        out_stdout_ptr: *mut *mut u8,
        out_stdout_len: *mut usize,
        out_stderr_ptr: *mut *mut u8,
        out_stderr_len: *mut usize,
    ) -> i32;
}

#[cfg(target_os = "wasi")]
pub fn wasi_spawn(
    program: &std::ffi::OsStr,
    args: &[std::ffi::OsString],
    env: &std::collections::BTreeMap<String, Option<std::ffi::OsString>>,
    cwd: Option<&std::ffi::OsStr>,
) -> Result<(i32, Vec<u8>, Vec<u8>), String> {
    use std::io::Write;

    let program_s = program.to_string_lossy();

    let mut args_buf = Vec::new();
    for arg in args {
        write!(args_buf, "{}\0", arg.to_string_lossy()).unwrap();
    }

    let mut env_buf = Vec::new();
    for (k, v) in env {
        if let Some(v) = v {
            write!(env_buf, "{}={}\0", k, v.to_string_lossy()).unwrap();
        }
    }

    let cwd_s = cwd.map(|c| c.to_string_lossy()).unwrap_or_default();
    let stdin = empty_stdin();

    let mut out_exit_code: i32 = 0;
    let mut out_stdout_ptr: *mut u8 = std::ptr::null_mut();
    let mut out_stdout_len: usize = 0;
    let mut out_stderr_ptr: *mut u8 = std::ptr::null_mut();
    let mut out_stderr_len: usize = 0;

    let res = unsafe {
        wasi_ext_spawn(
            program_s.as_ptr(),
            program_s.len(),
            args_buf.as_ptr(),
            args_buf.len(),
            env_buf.as_ptr(),
            env_buf.len(),
            cwd_s.as_ptr(),
            cwd_s.len(),
            stdin.as_ptr(),
            stdin.len(),
            &mut out_exit_code,
            &mut out_stdout_ptr,
            &mut out_stdout_len,
            &mut out_stderr_ptr,
            &mut out_stderr_len,
        )
    };

    if res != 0 {
        return Err(format!("wasi_ext_spawn failed with code {}", res));
    }

    let stdout = unsafe { Vec::from_raw_parts(out_stdout_ptr, out_stdout_len, out_stdout_len) };
    let stderr = unsafe { Vec::from_raw_parts(out_stderr_ptr, out_stderr_len, out_stderr_len) };

    Ok((out_exit_code, stdout, stderr))
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
    let res = unsafe { wasi_ext_git_clone(url.as_ptr(), url.len(), dest.as_ptr(), dest.len()) };
    if res == 0 {
        Ok(())
    } else {
        Err(format!("wasi_ext_git_clone failed with code {}", res))
    }
}

#[cfg(target_os = "wasi")]
pub fn wasi_git_fetch(repo_path: &str) -> Result<(), String> {
    let res = unsafe { wasi_ext_git_fetch(repo_path.as_ptr(), repo_path.len()) };
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

#[cfg(any(target_os = "wasi", test))]
fn allocate_owned_bytes(size: usize) -> *mut u8 {
    Box::into_raw(vec![0; size].into_boxed_slice()) as *mut u8
}

#[cfg(target_os = "wasi")]
#[unsafe(no_mangle)]
pub extern "C" fn wasi_ext_allocate(size: usize) -> *mut u8 {
    allocate_owned_bytes(size)
}

#[cfg(test)]
mod tests {
    use super::{allocate_owned_bytes, empty_stdin};

    #[test]
    fn allocated_bytes_can_be_written_reconstructed_and_dropped() {
        for (expected, size) in [(Vec::new(), 0), (vec![11, 22, 33, 44], 4)] {
            let ptr = allocate_owned_bytes(size);
            assert!(!ptr.is_null());

            unsafe {
                std::ptr::copy_nonoverlapping(expected.as_ptr(), ptr, size);
                let actual = Vec::from_raw_parts(ptr, size, size);
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn wasi_spawn_empty_stdin_has_valid_pointer() {
        let stdin = empty_stdin();
        assert_eq!(stdin.len(), 0);
        assert_ne!(stdin.as_ptr() as usize, 0);
        assert!(stdin.as_ptr().is_aligned());
    }
}
