use std::{io::{Read as _, Write as _}, os::fd::AsRawFd as _};

pub struct StdoutCapturer {
    original_stdout_fd: i32,
    capture_file: std::fs::File,
    capture_file_name: String,
}

pub fn exchange_local_fd(from_fd: i32, to_fd: i32) -> Result<(), std::io::Error> {
    // #[link(wasm_import_module = "extend_imports")]
    // extern "C" {
    //     // This function is implemented in the `extend_imports` module.
    //     fn exchange_local_fd(from_fd: i32, to_fd: i32) -> i32;
    // }

    // let ret = unsafe { exchange_local_fd(from_fd, to_fd) };

    let rand = rand::random::<u64>();
    let tmp_file = format!("/tmp/exchange_local_fd_{}", rand);
    let tmp_file_fd = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp_file)?
        .as_raw_fd();

    if unsafe { libc::__wasilibc_fd_renumber(from_fd, tmp_file_fd) } != 0 {
        return Err(std::io::Error::last_os_error())
    }

    if unsafe { libc::__wasilibc_fd_renumber(to_fd, from_fd) } != 0 {
        return Err(std::io::Error::last_os_error())
    }

    if unsafe { libc::__wasilibc_fd_renumber(tmp_file_fd, to_fd) } != 0 {
        return Err(std::io::Error::last_os_error())
    }

    // rm tmp file
    std::fs::remove_file(&tmp_file)?;

    return Ok(());
}

impl StdoutCapturer {
    pub fn start_capture_stdout() -> Result<StdoutCapturer, std::io::Error> {
        std::io::stdout().flush()?;

        let rand = rand::random::<u64>();

        let file_name = format!("/tmp/capture_stdout_{}", rand);

        let capture_file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_name)?;

        let fd = capture_file.as_raw_fd();

        exchange_local_fd(1, fd)?;

        Ok(StdoutCapturer {
            original_stdout_fd: 1,
            capture_file,
            capture_file_name: file_name,
        })
    }

    pub fn start_capture_stderr() -> Result<StdoutCapturer, std::io::Error> {
        std::io::stderr().flush()?;

        let rand = rand::random::<u64>();

        let file_name = format!("/tmp/capture_stderr_{}", rand);

        let capture_file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_name)?;

        let fd = capture_file.as_raw_fd();

        exchange_local_fd(2, fd)?;

        Ok(StdoutCapturer {
            original_stdout_fd: 2,
            capture_file,
            capture_file_name: file_name,
        })
    }

    pub fn stop_capture(self) -> Result<Vec<u8>, std::io::Error> {
        exchange_local_fd(self.capture_file.as_raw_fd(), self.original_stdout_fd)?;

        // close fd
        drop(self.capture_file);

        let mut buf = Vec::new();
        std::fs::File::open(&self.capture_file_name)?.read_to_end(&mut buf)?;

        // remove file
        std::fs::remove_file(&self.capture_file_name)?;

        // don't close original stdout fd
        // but stdout is number so it won't be closed by drop

        Ok(buf)
    }
}


pub struct StdinCapturer {
    original_stdin_fd: i32,
    capture_file: std::fs::File,
    capture_file_name: String,
}

impl StdinCapturer {
    pub fn set_stdin(input: &[u8]) -> Result<StdinCapturer, std::io::Error> {
        let rand = rand::random::<u64>();

        let file_name = format!("/tmp/capture_stdin_{}", rand);

        let mut capture_file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_name)?;

        capture_file.write_all(input)?;

        let fd = capture_file.as_raw_fd();

        exchange_local_fd(0, fd)?;

        Ok(StdinCapturer {
            original_stdin_fd: 0,
            capture_file,
            capture_file_name: file_name,
        })
    }

    pub fn stop_capture(self) -> Result<(), std::io::Error> {
        exchange_local_fd(self.capture_file.as_raw_fd(), self.original_stdin_fd)?;

        // close fd
        drop(self.capture_file);

        // remove file
        std::fs::remove_file(&self.capture_file_name)?;

        // don't close original stdin fd
        // but stdin is number so it won't be closed by drop

        Ok(())
    }
}
