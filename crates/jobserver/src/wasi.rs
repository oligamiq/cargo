use libc::c_int;

use crate::FromEnvErrorInner;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek as _, Write};
use std::mem;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd as _, BorrowedFd, FromRawFd as _, IntoRawFd};
use std::path::Path;
use std::process::Command;
use std::ptr;
use std::sync::Mutex;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Once,
};
use std::thread::{self, Builder, JoinHandle};
use std::time::Duration;

#[derive(Debug)]
/// This preserves the `--jobserver-auth` type at creation time,
/// so auth type will be passed down to and inherit from sub-Make processes correctly.
///
/// See <https://github.com/rust-lang/jobserver-rs/issues/99> for details.
enum ClientCreationArg {
    Fds { read: c_int, write: c_int },
    Fifo(Box<Path>),
}

#[derive(Debug)]
pub struct Client {
    read_and_write: Arc<Mutex<ReadAndWrite>>,
    creation_arg: ClientCreationArg,
    /// It is set to `None` if the pipe is shared with other processes, so it
    /// cannot support non-blocking mode.
    ///
    /// If it is set to `Some`, then it can only go from
    /// `Some(false)` -> `Some(true)` but not the other way around,
    /// since that could cause a race condition.
    is_non_blocking: Option<AtomicBool>,
}

#[derive(Debug)]
pub struct ReadAndWrite {
    read: File,
    write: File,
}

impl ReadAndWrite {
    pub fn new(
        read: File,
        write: File,
    ) -> Self {
        Self {
            read,
            write,
        }
    }

    // read and clear only read buffer
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        println!("## read");

        // reset read cursor
        self.read.seek(io::SeekFrom::Start(0))?;

        // read
        let mut all_buffer = Vec::new();
        self.read.read_to_end(&mut all_buffer)?;

        println!("## read, all_buffer: {:?}", all_buffer);

        // copy to buf
        let len = buf.len().min(all_buffer.len());

        println!("## read, len: {}", len);

        // expand len size because copy_from_slice will panic if len is not equal to buf.len()
        let mut copied_buf = all_buffer.clone();
        copied_buf.resize(buf.len(), 0);

        println!("## read, copied_buf: {:?}", copied_buf);

        buf.copy_from_slice(&copied_buf);

        println!("## read, buf: {:?}", buf);

        // clear only read buffer
        self.read.seek(io::SeekFrom::Start(0))?;
        self.read.set_len(0)?;
        self.read.write_all(&all_buffer[len..])?;

        println!("## read, all_buffer: {:?}", all_buffer);

        Ok(len)
    }

    // append to file
    pub fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        // reset read cursor
        self.read.seek(io::SeekFrom::Start(0))?;

        // read
        let mut all_buffer = Vec::new();
        self.read.read_to_end(&mut all_buffer)?;

        println!("## write_all, all_buffer: {:?}", all_buffer);

        // append
        all_buffer.extend_from_slice(buf);

        println!("## write_all, all_buffer: {:?}", all_buffer);

        // clear
        self.read.seek(io::SeekFrom::Start(0))?;
        self.read.set_len(0)?;

        println!("## write_all, all_buffer: {:?}", all_buffer);

        // write
        self.read.write_all(&all_buffer)?;

        println!("## write_all, all_buffer: {:?}", all_buffer);

        Ok(())
    }

    // append to file
    pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // reset read cursor
        self.read.seek(io::SeekFrom::Start(0))?;

        // read
        let mut all_buffer = Vec::new();
        self.read.read_to_end(&mut all_buffer)?;

        // append
        all_buffer.extend_from_slice(buf);

        // clear
        self.read.seek(io::SeekFrom::Start(0))?;
        self.read.set_len(0)?;

        // write
        self.read.write_all(&all_buffer)?;

        Ok(buf.len())
    }

    pub fn get_raw_fd_read(&self) -> c_int {
        self.read.as_raw_fd()
    }

    pub fn get_raw_fd_write(&self) -> c_int {
        self.write.as_raw_fd()
    }
}

#[derive(Debug)]
pub struct Acquired {
    byte: u8,
}

impl Client {
    pub fn new(mut limit: usize) -> io::Result<Client> {
        println!("## new");
        let client = unsafe { Client::mk()? };

        // I don't think the character written here matters, but I could be
        // wrong!
        const BUFFER: [u8; 128] = [b'|'; 128];

        let holder = client.read_and_write.clone();
        let mut lock = holder.lock().unwrap();
        let write = lock.get_raw_fd_write();

        println!("## set_nonblocking");

        set_nonblocking(write, true)?;

        println!("## write_all");

        while limit > 0 {
            println!("## write_all loop, limit: {}", limit);
            let n = limit.min(BUFFER.len());

            println!("## write_all loop, n: {}", n);
            lock.write_all(&BUFFER[..n])?;

            println!("## write_all loop, end");

            limit -= n;
        }

        println!("## set_nonblocking");

        set_nonblocking(write, false)?;

        println!("## end");

        Ok(client)
    }

    unsafe fn mk() -> io::Result<Client> {
        println!("## mk");
        let rand = rand::random::<u64>();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(format!("/tmp/jobserver_{}", rand))?;
        let fd = file.into_raw_fd();
        drop(set_cloexec(fd, true));
        drop(set_cloexec(fd, true));
        let ret = Ok(Client::from_fds(fd, fd));
        println!("## mk end");
        ret
    }

    pub(crate) unsafe fn open(s: &str, check_pipe: bool) -> Result<Client, FromEnvErrorInner> {
        if let Some(client) = Self::from_fifo(s)? {
            return Ok(client);
        }
        if let Some(client) = Self::from_pipe(s, check_pipe)? {
            return Ok(client);
        }
        Err(FromEnvErrorInner::CannotParse(format!(
            "expected `fifo:PATH` or `R,W`, found `{s}`"
        )))
    }

    /// `--jobserver-auth=fifo:PATH`
    fn from_fifo(s: &str) -> Result<Option<Client>, FromEnvErrorInner> {
        println!("## jobserver from_fifo is not supported");
        return Err(FromEnvErrorInner::Unsupported);

        let mut parts = s.splitn(2, ':');
        if parts.next().unwrap() != "fifo" {
            return Ok(None);
        }
        let path_str = parts.next().ok_or_else(|| {
            FromEnvErrorInner::CannotParse("expected a path after `fifo:`".to_string())
        })?;
        let path = Path::new(path_str);

        let open_file = || {
            // Opening with read write is necessary, since opening with
            // read-only or write-only could block the thread until another
            // thread opens it with write-only or read-only (or RDWR)
            // correspondingly.
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .map_err(|err| FromEnvErrorInner::CannotOpenPath(path_str.to_string(), err))
        };

        Ok(Some(Client {
            read_and_write: Arc::new(Mutex::new(ReadAndWrite {
                read: open_file()?,
                write: open_file()?,
            })),
            creation_arg: ClientCreationArg::Fifo(path.into()),
            is_non_blocking: Some(AtomicBool::new(false)),
        }))
    }

    /// `--jobserver-auth=R,W`
    unsafe fn from_pipe(s: &str, check_pipe: bool) -> Result<Option<Client>, FromEnvErrorInner> {
        println!("## jobserver from_pipe is not supported");
        return Err(FromEnvErrorInner::Unsupported);

        let mut parts = s.splitn(2, ',');
        let read = parts.next().unwrap();
        let write = match parts.next() {
            Some(w) => w,
            None => return Ok(None),
        };
        let read = read
            .parse()
            .map_err(|e| FromEnvErrorInner::CannotParse(format!("cannot parse `read` fd: {e}")))?;
        let write = write
            .parse()
            .map_err(|e| FromEnvErrorInner::CannotParse(format!("cannot parse `write` fd: {e}")))?;

        // If either or both of these file descriptors are negative,
        // it means the jobserver is disabled for this process.
        if read < 0 {
            return Err(FromEnvErrorInner::NegativeFd(read));
        }
        if write < 0 {
            return Err(FromEnvErrorInner::NegativeFd(write));
        }

        let creation_arg = ClientCreationArg::Fds { read, write };

        // Ok so we've got two integers that look like file descriptors, but
        // for extra sanity checking let's see if they actually look like
        // valid files and instances of a pipe if feature enabled before we
        // return the client.
        //
        // If we're called from `make` *without* the leading + on our rule
        // then we'll have `MAKEFLAGS` env vars but won't actually have
        // access to the file descriptors.
        //
        // `NotAPipe` is a worse error, return it if it's reported for any of the two fds.
        match (fd_check(read, check_pipe), fd_check(write, check_pipe)) {
            (read_err @ Err(FromEnvErrorInner::NotAPipe(..)), _) => read_err?,
            (_, write_err @ Err(FromEnvErrorInner::NotAPipe(..))) => write_err?,
            (read_err, write_err) => {
                read_err?;
                write_err?;

                // Optimization: Try converting it to a fifo by using /dev/fd
                //
                // On linux, opening `/dev/fd/$fd` returns a fd with a new file description,
                // so we can set `O_NONBLOCK` on it without affecting other processes.
                //
                // On macOS, opening `/dev/fd/$fd` seems to be the same as `File::try_clone`.
                //
                // I tested this on macOS 14 and Linux 6.5.13
                #[cfg(target_os = "linux")]
                if let (Ok(read), Ok(write)) = (
                    File::open(format!("/dev/fd/{}", read)),
                    OpenOptions::new()
                        .write(true)
                        .open(format!("/dev/fd/{}", write)),
                ) {
                    return Ok(Some(Client {
                        read,
                        write,
                        creation_arg,
                        is_non_blocking: Some(AtomicBool::new(false)),
                    }));
                }
            }
        }

        Ok(Some(Client {
            read_and_write: Arc::new(Mutex::new(ReadAndWrite::new(
                clone_fd_and_set_cloexec(read)?,
                clone_fd_and_set_cloexec(write)?,
            ))),
            creation_arg,
            is_non_blocking: None,
        }))
    }


    unsafe fn from_fds(read: c_int, write: c_int) -> Client {
        Client {
            read_and_write: Arc::new(Mutex::new(ReadAndWrite {
                read: File::from_raw_fd(read),
                write: File::from_raw_fd(write),
            })),
            creation_arg: ClientCreationArg::Fds { read, write },
            is_non_blocking: None,
        }
    }

    pub fn acquire(&self) -> io::Result<Acquired> {
        println!("## acquire");
        // Ignore interrupts and keep trying if that happens
        loop {
            if let Some(token) = self.acquire_allow_interrupts()? {
                return Ok(token);
            }
        }
    }

    /// Block waiting for a token, returning `None` if we're interrupted with
    /// EINTR.
    fn acquire_allow_interrupts(&self) -> io::Result<Option<Acquired>> {
        println!("## acquire_allow_interrupts");
        // We don't actually know if the file descriptor here is set in
        // blocking or nonblocking mode. AFAIK all released versions of
        // `make` use blocking fds for the jobserver, but the unreleased
        // version of `make` doesn't. In the unreleased version jobserver
        // fds are set to nonblocking and combined with `pselect`
        // internally.
        //
        // Here we try to be compatible with both strategies. We optimistically
        // try to read from the file descriptor which then may block, return
        // a token or indicate that polling is needed.
        // Blocking reads (if possible) allows the kernel to be more selective
        // about which readers to wake up when a token is written to the pipe.
        //
        // We use `poll` here to block this thread waiting for read
        // readiness, and then afterwards we perform the `read` itself. If
        // the `read` returns that it would block then we start over and try
        // again.
        //
        // Also note that we explicitly don't handle EINTR here. That's used
        // to shut us down, so we otherwise punt all errors upwards.
        unsafe {
            let mut fd: libc::pollfd = mem::zeroed();
            let holder = self.read_and_write.clone();
            let mut lock = holder.lock().unwrap();
            fd.fd = lock.get_raw_fd_read();
            fd.events = libc::POLLIN;
            println!("## pol 1l");
            loop {
                let mut buf = [0];
                println!("## read");
                match lock.read(&mut buf) {
                    Ok(1) => return Ok(Some(Acquired { byte: buf[0] })),
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "early EOF on jobserver pipe",
                        ));
                    }
                    Err(e) => match e.kind() {
                        io::ErrorKind::WouldBlock => { /* fall through to polling */ }
                        io::ErrorKind::Interrupted => return Ok(None),
                        _ => return Err(e),
                    },
                }

                println!("## poll 2");

                loop {
                    fd.revents = 0;
                    if libc::poll(&mut fd, 1, -1) == -1 {
                        let e = io::Error::last_os_error();
                        return match e.kind() {
                            io::ErrorKind::Interrupted => Ok(None),
                            _ => Err(e),
                        };
                    }
                    if fd.revents != 0 {
                        break;
                    }
                }
            }
        }
    }

    pub fn try_acquire(&self) -> io::Result<Option<Acquired>> {
        let mut buf = [0];
        let holder = self.read_and_write.clone();
        let mut fifo = holder.lock().unwrap();

        if let Some(is_non_blocking) = self.is_non_blocking.as_ref() {
            if !is_non_blocking.load(Ordering::Relaxed) {
                set_nonblocking(fifo.get_raw_fd_read(), true)?;
                is_non_blocking.store(true, Ordering::Relaxed);
            }
        } else {
            return Err(io::ErrorKind::Unsupported.into());
        }

        loop {
            match fifo.read(&mut buf) {
                Ok(1) => break Ok(Some(Acquired { byte: buf[0] })),
                Ok(_) => {
                    break Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "early EOF on jobserver pipe",
                    ))
                }

                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break Ok(None),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,

                Err(err) => break Err(err),
            }
        }
    }

    pub fn release(&self, data: Option<&Acquired>) -> io::Result<()> {
        // Note that the fd may be nonblocking but we're going to go ahead
        // and assume that the writes here are always nonblocking (we can
        // always quickly release a token). If that turns out to not be the
        // case we'll get an error anyway!
        let byte = data.map(|d| d.byte).unwrap_or(b'+');
        let holder = self.read_and_write.clone();
        let mut lock = holder.lock().unwrap();
        match lock.write(&[byte])? {
            1 => Ok(()),
            _ => Err(io::Error::new(
                io::ErrorKind::Other,
                "failed to write token back to jobserver",
            )),
        }
    }

    pub fn string_arg(&self) -> String {
        match &self.creation_arg {
            ClientCreationArg::Fifo(path) => format!("fifo:{}", path.display()),
            ClientCreationArg::Fds { read, write } => format!("{},{}", read, write),
        }
    }

    pub fn available(&self) -> io::Result<usize> {
        let mut len = MaybeUninit::<c_int>::uninit();
        let holder = self.read_and_write.clone();
        let lock = holder.lock().unwrap();
        cvt(unsafe { libc::ioctl(lock.get_raw_fd_read(), libc::FIONREAD, len.as_mut_ptr()) })?;
        Ok(unsafe { len.assume_init() } as usize)
    }

    pub fn configure(&self, cmd: &mut Command) {
        if matches!(self.creation_arg, ClientCreationArg::Fifo { .. }) {
            // We `File::open`ed it when inheriting from environment,
            // so no need to set cloexec for fifo.
            return;
        }
        // Here we basically just want to say that in the child process
        // we'll configure the read/write file descriptors to *not* be
        // cloexec, so they're inherited across the exec and specified as
        // integers through `string_arg` above.
        let holder = self.read_and_write.clone();
        let lock = holder.lock().unwrap();
        let read = lock.get_raw_fd_read();
        let write = lock.get_raw_fd_write();
        // unsafe {
        //     cmd.pre_exec(move || {
                set_cloexec(read, false).unwrap();
                set_cloexec(write, false).unwrap();
        //         Ok(())
        //     });
        // }
    }
}

#[derive(Debug)]
pub struct Helper {
    thread: JoinHandle<()>,
    state: Arc<super::HelperState>,
}

pub(crate) fn spawn_helper(
    client: crate::Client,
    state: Arc<super::HelperState>,
    mut f: Box<dyn FnMut(io::Result<crate::Acquired>) + Send>,
) -> io::Result<Helper> {
    let state2 = state.clone();
    let thread = Builder::new().spawn(move || {
        state2.for_each_request(|helper| loop {
            match client.inner.acquire_allow_interrupts() {
                Ok(Some(data)) => {
                    break f(Ok(crate::Acquired {
                        client: client.inner.clone(),
                        data,
                        disabled: false,
                    }));
                }
                Err(e) => break f(Err(e)),
                Ok(None) if helper.lock().producer_done => break,
                Ok(None) => {}
            }
        });
    })?;

    Ok(Helper { thread, state })
}

impl Helper {
    pub fn join(self) {
        let dur = Duration::from_millis(10);
        let mut state = self.state.lock();
        debug_assert!(state.producer_done);

        // We need to join our helper thread, and it could be blocked in one
        // of two locations. First is the wait for a request, but the
        // initial drop of `HelperState` will take care of that. Otherwise
        // it may be blocked in `client.acquire()`. We actually have no way
        // of interrupting that, so resort to `pthread_kill` as a fallback.
        // This signal should interrupt any blocking `read` call with
        // `io::ErrorKind::Interrupt` and cause the thread to cleanly exit.
        //
        // Note that we don't do this forever though since there's a chance
        // of bugs, so only do this opportunistically to make a best effort
        // at clearing ourselves up.
        for _ in 0..100 {
            if state.consumer_done {
                break;
            }
            state = self
                .state
                .cvar
                .wait_timeout(state, dur)
                .unwrap_or_else(|e| e.into_inner())
                .0;
            thread::yield_now(); // we really want the other thread to run
        }

        // If we managed to actually see the consumer get done, then we can
        // definitely wait for the thread. Otherwise it's... off in the ether
        // I guess?
        if state.consumer_done {
            drop(self.thread.join());
        }
    }
}

unsafe fn fcntl_check(fd: c_int) -> Result<(), FromEnvErrorInner> {
    match libc::fcntl(fd, libc::F_GETFD) {
        -1 => Err(FromEnvErrorInner::CannotOpenFd(
            fd,
            io::Error::last_os_error(),
        )),
        _ => Ok(()),
    }
}

unsafe fn fd_check(fd: c_int, check_pipe: bool) -> Result<(), FromEnvErrorInner> {
    if check_pipe {
        let mut stat = mem::zeroed();
        if libc::fstat(fd, &mut stat) == -1 {
            let last_os_error = io::Error::last_os_error();
            fcntl_check(fd)?;
            Err(FromEnvErrorInner::NotAPipe(fd, Some(last_os_error)))
        } else {
            // On android arm and i686 mode_t is u16 and st_mode is u32,
            // this generates a type mismatch when S_IFIFO (declared as mode_t)
            // is used in operations with st_mode, so we use this workaround
            // to get the value of S_IFIFO with the same type of st_mode.
            #[allow(unused_assignments)]
            let mut s_ififo = stat.st_mode;
            s_ififo = libc::S_IFIFO as _;
            if stat.st_mode & s_ififo == s_ififo {
                return Ok(());
            }
            Err(FromEnvErrorInner::NotAPipe(fd, None))
        }
    } else {
        fcntl_check(fd)
    }
}

fn clone_fd_and_set_cloexec(fd: c_int) -> Result<File, FromEnvErrorInner> {
    // Safety: fd is a valid fd dand it remains open until returns
    unsafe { BorrowedFd::borrow_raw(fd) }
        .try_clone_to_owned()
        .map(File::from)
        .map_err(|err| FromEnvErrorInner::CannotOpenFd(fd, err))
}

fn set_cloexec(fd: c_int, set: bool) -> io::Result<()> {
    unsafe {
        let previous = cvt(libc::fcntl(fd, libc::F_GETFD))?;
        let new = if set {
            previous | libc::FD_CLOEXEC
        } else {
            previous & !libc::FD_CLOEXEC
        };
        if new != previous {
            cvt(libc::fcntl(fd, libc::F_SETFD, new))?;
        }
        Ok(())
    }
}

fn set_nonblocking(fd: c_int, set: bool) -> io::Result<()> {
    // let status_flag = if set { libc::O_NONBLOCK } else { 0 };

    // unsafe {
    //     cvt(libc::fcntl(fd, libc::F_SETFL, status_flag))?;
    // }

    Ok(())
}
fn cvt(t: c_int) -> io::Result<c_int> {
    if t == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(t)
    }
}
