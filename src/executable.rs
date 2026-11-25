use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Mutex;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

static EXECUTABLE: &[u8] = include_bytes!(env!("BARE_SIDECAR_EXECUTABLE"));

const NAME: &str = if cfg!(windows) { "bare.exe" } else { "bare" };

/// The path of the Bare executable embedded in this crate, which is extracted
/// to the cache directory of the user on first use.
pub fn executable() -> io::Result<PathBuf> {
    static PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

    let mut path = PATH.lock().unwrap_or_else(|err| err.into_inner());

    if let Some(path) = &*path {
        return Ok(path.clone());
    }

    let extracted = extract()?;

    *path = Some(extracted.clone());

    Ok(extracted)
}

// Checked against the embedded executable once per process, so that a
// truncated or modified copy is replaced rather than run.
fn extract() -> io::Result<PathBuf> {
    let dir = cache_dir()?
        .join("bare-sidecar")
        .join(env!("BARE_SIDECAR_DIGEST"));

    let path = dir.join(NAME);

    if is_extracted(&path) {
        return Ok(path);
    }

    fs::create_dir_all(&dir)?;

    // Written aside and renamed into place, so that processes extracting at
    // the same time never run a partial copy.
    let temporary = dir.join(format!("{NAME}.{}", process::id()));

    let result = write(&temporary).and_then(|()| fs::rename(&temporary, &path));

    if let Err(err) = result {
        let _ = fs::remove_file(&temporary);

        // Windows refuses to replace an executable that is running, which
        // another process may have extracted in the meantime.
        if !is_extracted(&path) {
            return Err(err);
        }
    }

    Ok(path)
}

fn write(path: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();

    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    options.mode(0o755);

    options.open(path)?.write_all(EXECUTABLE)
}

fn is_extracted(path: &Path) -> bool {
    fs::read(path).is_ok_and(|data| data == EXECUTABLE)
}

fn cache_dir() -> io::Result<PathBuf> {
    let dir = if cfg!(windows) {
        env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches"))
    } else {
        env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    };

    dir.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No cache directory"))
}
