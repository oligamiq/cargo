use std::{io::{Read as _, Write as _}, os::fd::AsRawFd as _};

#[derive(Debug)]
pub struct StdoutCapturer {
    original_stdout_fd: i32,
    capture_file: std::fs::File,
    capture_file_name: String,

    #[allow(dead_code)]
    read_buf: i64,
}

pub fn exchange_local_fd(from_fd: i32, to_fd: i32) -> Result<(), std::io::Error> {
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
    pub fn new_stdout() -> Result<StdoutCapturer, std::io::Error> {
        std::io::stdout().flush()?;

        let rand = rand::random::<u64>();

        let file_name = format!("/tmp/capture_stdout_{}", rand);

        let capture_file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_name)?;

        Ok(StdoutCapturer {
            read_buf: 0,
            original_stdout_fd: 1,
            capture_file,
            capture_file_name: file_name,
        })
    }

    pub fn new_stderr() -> Result<StdoutCapturer, std::io::Error> {
        std::io::stderr().flush()?;

        let rand = rand::random::<u64>();

        let file_name = format!("/tmp/capture_stderr_{}", rand);

        let capture_file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_name)?;

        Ok(StdoutCapturer {
            read_buf: 0,
            original_stdout_fd: 2,
            capture_file,
            capture_file_name: file_name,
        })
    }

    pub fn start_capture(&self) -> Result<(), std::io::Error> {
        let fd = self.capture_file.as_raw_fd();

        exchange_local_fd(1, fd)?;

        Ok(())
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

//     pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
//         let fd = self.original_stdout_fd;
//         let offset = self.read_buf;

//         let n = unsafe { libc::pread(fd, buf.as_mut_ptr() as *mut _, buf.len(), offset) };

//         if n < 0 {
//             return Err(std::io::Error::last_os_error());
//         }

//         self.read_buf += n as i64;

//         Ok(n as usize)
//     }
}

#[derive(Debug)]
pub struct StdinCapturer {
    original_stdin_fd: i32,
    capture_file: std::fs::File,
    capture_file_name: String,
}

impl StdinCapturer {
    pub fn new() -> Result<StdinCapturer, std::io::Error> {
        let rand = rand::random::<u64>();

        let file_name = format!("/tmp/capture_stdin_{}", rand);

        let capture_file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_name)?;

        Ok(StdinCapturer {
            original_stdin_fd: 0,
            capture_file,
            capture_file_name: file_name,
        })
    }

    pub fn set_stdin(&self, input: &[u8]) -> Result<(), std::io::Error> {
        let mut capture_file = self.capture_file.try_clone()?;

        capture_file.write_all(input)?;

        let fd = capture_file.as_raw_fd();

        exchange_local_fd(0, fd)?;

        Ok(())
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
