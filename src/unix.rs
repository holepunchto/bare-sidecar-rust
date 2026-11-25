use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::Command;

// The descriptor that the embedder opens `Bare.IPC` from.
const FD: i32 = 3;

pub struct Ipc {
    stream: UnixStream,
}

// The end of the connection that is handed to the process.
pub struct Remote {
    fd: OwnedFd,
}

pub fn pair() -> io::Result<(Ipc, Remote)> {
    let (stream, remote) = UnixStream::pair()?;

    Ok((Ipc { stream }, Remote { fd: remote.into() }))
}

impl Remote {
    pub fn value(&self) -> String {
        FD.to_string()
    }

    pub fn pass(&self, command: &mut Command) {
        let fd = self.fd.as_raw_fd();

        // Moved into place in the child, where `dup2()` clears the
        // close-on-exec flag of the copy, or clearing it directly if the
        // descriptor already is in place.
        unsafe {
            command.pre_exec(move || {
                let err = if fd == FD {
                    libc::fcntl(fd, libc::F_SETFD, 0)
                } else {
                    libc::dup2(fd, FD)
                };

                if err == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
}

impl Ipc {
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        if !buf.is_empty() {
            self.wait_readable()?;
        }

        let read = match (&self.stream).read(buf) {
            // On Linux, a process that exits with data still unread resets the
            // connection rather than ending it.
            Err(err) if err.kind() == io::ErrorKind::ConnectionReset => 0,
            result => result?,
        };

        // The process exits once the connection has closed, which takes both
        // ends to have ended.
        if read == 0 && !buf.is_empty() {
            let _ = self.stream.shutdown(Shutdown::Write);
        }

        Ok(read)
    }

    // A blocking read on macOS may miss the end of the connection and wait
    // forever, which waiting with `poll()` first does not.
    fn wait_readable(&self) -> io::Result<()> {
        let mut fd = libc::pollfd {
            fd: self.stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };

        loop {
            if unsafe { libc::poll(&mut fd, 1, -1) } != -1 {
                return Ok(());
            }

            let err = io::Error::last_os_error();

            if err.kind() != io::ErrorKind::Interrupted {
                return Err(err);
            }
        }
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        (&self.stream).write(buf)
    }

    pub fn close(&self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}
