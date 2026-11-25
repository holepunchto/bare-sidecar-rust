//! Start and manage Bare sidecar processes from Rust.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Mutex, PoisonError};

mod executable;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as ipc;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as ipc;

pub use executable::executable;

/// The versions of the components bundled with this crate, named as in
/// `Bare.versions`, such as `("bare", "1.38.0")` for the version of Bare. Read
/// from the `versions.json` manifest of the crate.
pub fn versions() -> &'static [(&'static str, &'static str)] {
    include!(concat!(env!("OUT_DIR"), "/versions.rs"))
}

// Read by the embedder, which opens `Bare.IPC` from the descriptor, or on
// Windows the handle, it names.
const IPC_ENV: &str = "BARE_SIDECAR_IPC";

/// Options for spawning a sidecar.
#[derive(Debug, Default)]
pub struct Builder {
    args: Vec<OsString>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
    executable: Option<PathBuf>,
}

impl Builder {
    /// Append an argument to `Bare.argv`, after the entry point.
    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    /// Append arguments to `Bare.argv`, after the entry point.
    pub fn args<I>(self, args: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<OsStr>,
    {
        args.into_iter().fold(self, Self::arg)
    }

    /// The standard input of the process, inherited by default.
    pub fn stdin(mut self, stdio: impl Into<Stdio>) -> Self {
        self.stdin = Some(stdio.into());
        self
    }

    /// The standard output of the process, inherited by default.
    pub fn stdout(mut self, stdio: impl Into<Stdio>) -> Self {
        self.stdout = Some(stdio.into());
        self
    }

    /// The standard error of the process, inherited by default.
    pub fn stderr(mut self, stdio: impl Into<Stdio>) -> Self {
        self.stderr = Some(stdio.into());
        self
    }

    /// The Bare executable to run, which defaults to the one embedded in this
    /// crate. It must be built with the embedder of this crate.
    pub fn executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }

    /// Start `entry` in a Bare process with an IPC connection to it.
    pub fn spawn(self, entry: impl AsRef<Path>) -> io::Result<Sidecar> {
        let executable = match self.executable {
            Some(executable) => executable,
            None => executable()?,
        };

        let (ipc, remote) = ipc::pair()?;

        let mut command = Command::new(executable);

        command
            .arg(entry.as_ref())
            .args(&self.args)
            .env(IPC_ENV, remote.value());

        if let Some(stdio) = self.stdin {
            command.stdin(stdio);
        }

        if let Some(stdio) = self.stdout {
            command.stdout(stdio);
        }

        if let Some(stdio) = self.stderr {
            command.stderr(stdio);
        }

        remote.pass(&mut command);

        let mut child = command.spawn()?;

        drop(remote);

        Ok(Sidecar {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
            id: child.id(),
            child: Mutex::new(child),
            ipc,
        })
    }
}

/// Start `entry` in a Bare process with an IPC connection to it. Use
/// [`Sidecar::builder()`] for more options.
pub fn spawn(entry: impl AsRef<Path>) -> io::Result<Sidecar> {
    Sidecar::builder().spawn(entry)
}

/// A Bare process connected to its `Bare.IPC`.
///
/// Reading yields the data that the process writes to `Bare.IPC` until it ends
/// the connection and exits, and writing sends data to it. `&Sidecar` reads and
/// writes too, so one thread may read while another writes. Use
/// [`Sidecar::close()`] to close the connection, terminate the process, and
/// wait for it. Dropping the sidecar does the same, but waits for the process
/// in the background.
pub struct Sidecar {
    /// The standard input of the process, if spawned with [`Stdio::piped()`].
    pub stdin: Option<ChildStdin>,

    /// The standard output of the process, if spawned with [`Stdio::piped()`].
    pub stdout: Option<ChildStdout>,

    /// The standard error of the process, if spawned with [`Stdio::piped()`].
    pub stderr: Option<ChildStderr>,

    id: u32,
    child: Mutex<Child>,
    ipc: ipc::Ipc,
}

impl Sidecar {
    /// Options for spawning a sidecar.
    pub fn builder() -> Builder {
        Builder::default()
    }

    /// The process ID.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Ask the process to exit, with `SIGTERM` or on Windows by terminating
    /// it, unless it has already exited.
    pub fn terminate(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            // The ID of a process that has been waited for may already belong
            // to another process.
            let child = self.child_mut();

            if child.try_wait()?.is_none()
                && unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) } == -1
            {
                return Err(io::Error::last_os_error());
            }

            Ok(())
        }

        #[cfg(windows)]
        self.kill()
    }

    /// Close the connection, ask the process to exit as
    /// [`Sidecar::terminate()`] does, and wait for it to exit. Returns the
    /// status of the process, which stays available from [`Sidecar::wait()`].
    pub fn close(&mut self) -> io::Result<ExitStatus> {
        self.ipc.close();

        let _ = self.terminate();

        self.wait()
    }

    /// Kill the process, with `SIGKILL` or on Windows by terminating it,
    /// unless it has already exited.
    pub fn kill(&mut self) -> io::Result<()> {
        self.child_mut().kill()
    }

    /// Wait for the process to exit and return its status.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child_mut().wait()
    }

    /// The status of the process if it has exited, without waiting.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child_mut().try_wait()
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child.get_mut().unwrap_or_else(PoisonError::into_inner)
    }
}

// The process only exits once both ends of the connection have ended, which
// reading to the end does for this one, so it is waited for only after that.
impl Read for &Sidecar {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.ipc.read(buf)?;

        if read == 0 && !buf.is_empty() {
            self.child
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .wait()?;
        }

        Ok(read)
    }
}

impl Read for Sidecar {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }
}

// A process that is never waited for lingers as a zombie on Unix, which has no
// counterpart on Windows.
impl Drop for Sidecar {
    fn drop(&mut self) {
        self.ipc.close();

        let _ = self.terminate();

        #[cfg(unix)]
        {
            if let Ok(None) = self.child_mut().try_wait() {
                let pid = self.id as libc::pid_t;

                std::thread::spawn(move || unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) });
            }
        }
    }
}

impl Write for &Sidecar {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.ipc.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Write for Sidecar {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
