use std::path::{Path, PathBuf};
use std::time::Duration;

pub use self::canonical_url::CanonicalUrl;
pub use self::context::{ConfigValue, GlobalContext, homedir};
pub(crate) use self::counter::MetricsCounter;
pub use self::dependency_queue::DependencyQueue;
pub use self::diagnostic_server::RustfixDiagnosticServer;
pub use self::edit_distance::{closest, closest_msg, edit_distance};
pub use self::errors::CliError;
pub use self::errors::{CargoResult, CliResult, internal};
pub use self::flock::{FileLock, Filesystem};
pub use self::graph::Graph;
pub use self::hasher::StableHasher;
pub use self::hex::{hash_u64, short_hash, to_hex};
pub use self::into_url::IntoUrl;
pub use self::into_url_with_base::IntoUrlWithBase;
pub(crate) use self::io::LimitErrorReader;
pub use self::lockserver::{LockServer, LockServerClient, LockServerStarted};
pub use self::logger::BuildLogger;
pub use self::once::OnceExt;
pub use self::progress::{Progress, ProgressStyle};
pub use self::queue::Queue;
pub use self::rustc::Rustc;
pub use self::semver_ext::{OptVersionReq, VersionExt};
pub use self::unhashed::Unhashed;
pub use self::vcs::{FossilRepo, GitRepo, HgRepo, PijulRepo, existing_vcs_repo};
pub use self::workspace::{
    add_path_args, path_args, print_available_benches, print_available_binaries,
    print_available_examples, print_available_packages, print_available_tests,
};

pub mod auth;
pub mod cache_lock;
mod canonical_url;
pub mod command_prelude;
pub mod context;
mod counter;
pub mod cpu;
pub mod credential;
mod dependency_queue;
pub mod diagnostic_server;
pub mod edit_distance;
pub mod errors;
pub mod flock;
pub mod frontmatter;
pub mod graph;
mod hasher;
pub mod hex;
pub mod important_paths;
pub mod interning;
pub mod into_url;
mod into_url_with_base;
mod io;
pub mod job;
mod local_poll_adapter;
pub use local_poll_adapter::LocalPollAdapter;
mod lockserver;
pub mod log_message;
pub mod logger;
pub mod machine_message;
pub mod network;
mod once;
pub mod open;
mod progress;
mod queue;
pub mod restricted_names;
pub mod rustc;
mod semver_eval_ext;
mod semver_ext;
pub mod sqlite;
pub mod toml;
pub mod toml_mut;
mod unhashed;
mod vcs;
mod workspace;

pub use cargo_util_terminal::style;
#[cfg(not(target_os = "wasi"))]
pub(crate) use futures::executor::block_on;
#[cfg(not(target_os = "wasi"))]
pub(crate) use futures::executor::block_on_stream;

#[cfg(target_os = "wasi")]
pub(crate) use wasi_block_on as block_on;
#[cfg(target_os = "wasi")]
pub(crate) use wasi_block_on_stream as block_on_stream;

#[cfg(any(target_os = "wasi", test))]
struct WasiWake {
    notified: std::sync::atomic::AtomicBool,
}

#[cfg(any(target_os = "wasi", test))]
impl futures::task::ArcWake for WasiWake {
    fn wake_by_ref(arc_self: &std::sync::Arc<Self>) {
        arc_self
            .notified
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(any(target_os = "wasi", test))]
pub(crate) fn wasi_block_on<F: std::future::Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let wake = std::sync::Arc::new(WasiWake {
        notified: std::sync::atomic::AtomicBool::new(false),
    });
    let waker = futures::task::waker(wake.clone());
    let mut context = std::task::Context::from_waker(&waker);

    loop {
        wake.notified
            .swap(false, std::sync::atomic::Ordering::AcqRel);
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(output) => return output,
            std::task::Poll::Pending => {
                while !wake.notified.load(std::sync::atomic::Ordering::Acquire) {
                    std::thread::yield_now();
                }
            }
        }
    }
}

#[cfg(any(target_os = "wasi", test))]
pub(crate) struct WasiBlockingStream<S> {
    stream: S,
}

#[cfg(any(target_os = "wasi", test))]
impl<S: futures::Stream + Unpin> Iterator for WasiBlockingStream<S> {
    type Item = S::Item;

    fn next(&mut self) -> Option<Self::Item> {
        wasi_block_on(futures::StreamExt::next(&mut self.stream))
    }
}

#[cfg(any(target_os = "wasi", test))]
pub(crate) fn wasi_block_on_stream<S: futures::Stream + Unpin>(stream: S) -> WasiBlockingStream<S> {
    WasiBlockingStream { stream }
}

pub fn is_rustup() -> bool {
    #[expect(clippy::disallowed_methods, reason = "consistency with rustup")]
    std::env::var_os("RUSTUP_HOME").is_some()
}

pub fn elapsed(duration: Duration) -> String {
    let secs = duration.as_secs();

    if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}.{:02}s", secs, duration.subsec_nanos() / 10_000_000)
    }
}

/// Formats a number of bytes into a human readable SI-prefixed size.
pub struct HumanBytes(pub u64);

impl std::fmt::Display for HumanBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
        let bytes = self.0 as f32;
        let i = ((bytes.log2() / 10.0) as usize).min(UNITS.len() - 1);
        let unit = UNITS[i];
        let size = bytes / 1024_f32.powi(i as i32);

        // Don't show a fractional number of bytes.
        if i == 0 {
            return write!(f, "{size}{unit}");
        }

        let Some(precision) = f.precision() else {
            return write!(f, "{size}{unit}");
        };
        write!(f, "{size:.precision$}{unit}",)
    }
}

pub fn indented_lines(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                String::from("\n")
            } else {
                format!("  {}\n", line)
            }
        })
        .collect()
}

pub fn truncate_with_ellipsis(s: &str, max_width: usize) -> String {
    // We should truncate at grapheme-boundary and compute character-widths,
    // yet the dependencies on unicode-segmentation and unicode-width are
    // not worth it.
    let mut chars = s.chars();
    let mut prefix = (&mut chars).take(max_width - 1).collect::<String>();
    if chars.next().is_some() {
        prefix.push('…');
    }
    prefix
}

#[cfg(not(windows))]
#[inline]
pub fn try_canonicalize<P: AsRef<Path>>(path: P) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(&path)
}

#[cfg(windows)]
#[inline]
pub fn try_canonicalize<P: AsRef<Path>>(path: P) -> std::io::Result<PathBuf> {
    use std::io::Error;
    use std::io::ErrorKind;

    // On Windows `canonicalize` may fail, so we fall back to getting an absolute path.
    std::fs::canonicalize(&path).or_else(|_| {
        // Return an error if a file does not exist for better compatibility with `canonicalize`
        if !path.as_ref().try_exists()? {
            return Err(Error::new(ErrorKind::NotFound, "the path was not found"));
        }
        std::path::absolute(&path)
    })
}

/// Get the current [`umask`] value.
///
/// [`umask`]: https://man7.org/linux/man-pages/man2/umask.2.html
#[cfg(unix)]
pub fn get_umask() -> u32 {
    use std::sync::OnceLock;
    static UMASK: OnceLock<libc::mode_t> = OnceLock::new();
    // SAFETY: Syscalls are unsafe. Calling `umask` twice is even unsafer for
    // multithreading program, since it doesn't provide a way to retrieve the
    // value without modifications. We use a static `OnceLock` here to ensure
    // it only gets call once during the entire program lifetime.
    *UMASK.get_or_init(|| unsafe {
        let umask = libc::umask(0o022);
        libc::umask(umask);
        umask
    }) as u32 // it is u16 on macos
}

#[cfg(test)]
mod test {
    use super::*;

    #[track_caller]
    fn t(bytes: u64, expected: &str) {
        assert_eq!(&HumanBytes(bytes).to_string(), expected);
    }

    #[test]
    fn test_human_readable_bytes() {
        t(0, "0B");
        t(8, "8B");
        t(1000, "1000B");
        t(1024, "1KiB");
        t(1024 * 420 + 512, "420.5KiB");
        t(1024 * 1024, "1MiB");
        t(1024 * 1024 + 1024 * 256, "1.25MiB");
        t(1024 * 1024 * 1024, "1GiB");
        t((1024. * 1024. * 1024. * 1.2345) as u64, "1.2345GiB");
        t(1024 * 1024 * 1024 * 1024, "1TiB");
        t(1024 * 1024 * 1024 * 1024 * 1024, "1PiB");
        t(1024 * 1024 * 1024 * 1024 * 1024 * 1024, "1EiB");
        t(u64::MAX, "16EiB");

        assert_eq!(
            &format!("{:.3}", HumanBytes((1024. * 1.23456) as u64)),
            "1.234KiB"
        );
    }

    #[test]
    fn wasi_block_on_ready() {
        assert_eq!(wasi_block_on(async { 42 }), 42);
    }

    #[test]
    fn wasi_block_on_yields_once() {
        let mut pending = true;
        let future = futures::future::poll_fn(|cx| {
            if std::mem::take(&mut pending) {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            } else {
                std::task::Poll::Ready(42)
            }
        });

        assert_eq!(wasi_block_on(future), 42);
    }

    #[test]
    fn wasi_block_on_stream_drains_finite_stream() {
        let values = wasi_block_on_stream(futures::stream::iter([1, 2, 3])).collect::<Vec<_>>();

        assert_eq!(values, [1, 2, 3]);
    }

    #[test]
    fn wasi_block_on_stream_handles_empty_futures_unordered() {
        let futures = futures::stream::FuturesUnordered::<futures::future::Ready<usize>>::new();

        assert_eq!(wasi_block_on_stream(futures).next(), None);
    }

    #[test]
    fn wasi_block_on_stream_waits_for_pending_child_wake() {
        let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut futures = futures::stream::FuturesUnordered::new();
        futures.push(futures::future::poll_fn({
            let ready = ready.clone();
            move |cx| {
                if ready.load(std::sync::atomic::Ordering::Acquire) {
                    return std::task::Poll::Ready(42);
                }
                let ready = ready.clone();
                let waker = cx.waker().clone();
                std::thread::spawn(move || {
                    ready.store(true, std::sync::atomic::Ordering::Release);
                    waker.wake();
                });
                std::task::Poll::Pending
            }
        }));

        assert_eq!(wasi_block_on_stream(futures).collect::<Vec<_>>(), [42]);
    }

    #[test]
    fn wasi_block_on_stream_waits_for_multiple_pending_children() {
        fn pending_once(value: usize) -> impl std::future::Future<Output = usize> {
            let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            futures::future::poll_fn(move |cx| {
                if ready.load(std::sync::atomic::Ordering::Acquire) {
                    return std::task::Poll::Ready(value);
                }
                let ready = ready.clone();
                let waker = cx.waker().clone();
                std::thread::spawn(move || {
                    ready.store(true, std::sync::atomic::Ordering::Release);
                    waker.wake();
                });
                std::task::Poll::Pending
            })
        }

        let mut futures = futures::stream::FuturesUnordered::new();
        futures.push(pending_once(1));
        futures.push(pending_once(2));

        let mut values = wasi_block_on_stream(futures).collect::<Vec<_>>();
        values.sort_unstable();
        assert_eq!(values, [1, 2]);
    }

    #[test]
    fn wasi_block_on_waits_for_wake_from_another_thread() {
        let notified = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut first_poll = true;
        let future = futures::future::poll_fn({
            let notified = notified.clone();
            move |cx| {
                if std::mem::take(&mut first_poll) {
                    let notified = notified.clone();
                    let waker = cx.waker().clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        notified.store(true, std::sync::atomic::Ordering::Release);
                        waker.wake();
                    });
                    return std::task::Poll::Pending;
                }
                assert!(notified.load(std::sync::atomic::Ordering::Acquire));
                std::task::Poll::Ready(42)
            }
        });

        assert_eq!(wasi_block_on(future), 42);
    }
}
