use std::{env::SplitPaths, ffi::OsStr, path::PathBuf};

pub fn split_paths<T: AsRef<OsStr> + ?Sized>(unparsed: &T) -> Vec<PathBuf> {
    let unparsed = unparsed.as_ref();
    let paths = unparsed.to_str().unwrap().split(':').map(|s| PathBuf::from(s)).collect();
    paths
}
