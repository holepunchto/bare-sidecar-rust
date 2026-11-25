use std::ffi::OsStr;
use std::io;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::process::{self, Command};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_CONNECTED,
    ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, HANDLE_FLAG_INHERIT,
    INVALID_HANDLE_VALUE, SetHandleInformation,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_REJECT_REMOTE_CLIENTS, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::CreateEventW;

const BUFFER_SIZE: u32 = 65536;

// The server end of a named pipe, which stays open until dropped, so that
// disconnecting the process from it never invalidates a handle in use.
pub struct Ipc {
    pipe: OwnedHandle,
}

// The client end of the named pipe, which is handed to the process.
pub struct Remote {
    handle: OwnedHandle,
}

pub fn pair() -> io::Result<(Ipc, Remote)> {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());

    let name = wide(format!(
        r"\\.\pipe\bare-sidecar-{}-{}-{nanos}",
        process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    // A single instance that this process connects to before the child exists,
    // so that no other process can connect in its place.
    let pipe = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            BUFFER_SIZE,
            BUFFER_SIZE,
            0,
            ptr::null(),
        )
    };

    let pipe = owned(pipe)?;

    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            ptr::null_mut(),
        )
    };

    let handle = owned(handle)?;

    let ipc = Ipc { pipe };

    match ipc.overlapped(|overlapped| unsafe { ConnectNamedPipe(ipc.raw(), overlapped) }) {
        Err(err) if err.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) => return Err(err),
        _ => {}
    }

    Ok((ipc, Remote { handle }))
}

impl Remote {
    pub fn value(&self) -> String {
        (self.handle.as_raw_handle() as usize).to_string()
    }

    // Inheritable only for as long as it takes to spawn the process, as every
    // process spawned in the meantime inherits it too.
    pub fn pass(&self, _command: &mut Command) {
        unsafe {
            SetHandleInformation(
                self.handle.as_raw_handle(),
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            );
        }
    }
}

impl Ipc {
    fn raw(&self) -> HANDLE {
        self.pipe.as_raw_handle()
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let len = buf.len().min(u32::MAX as usize) as u32;

        let read = match self.overlapped(|overlapped| unsafe {
            ReadFile(
                self.raw(),
                buf.as_mut_ptr(),
                len,
                ptr::null_mut(),
                overlapped,
            )
        }) {
            Err(err) if is_disconnected(&err) => 0,
            result => result? as usize,
        };

        // Named pipes cannot be half closed, so the process only sees the
        // connection end once it is disconnected.
        if read == 0 && !buf.is_empty() {
            unsafe { DisconnectNamedPipe(self.raw()) };
        }

        Ok(read)
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let len = buf.len().min(u32::MAX as usize) as u32;

        match self.overlapped(|overlapped| unsafe {
            WriteFile(self.raw(), buf.as_ptr(), len, ptr::null_mut(), overlapped)
        }) {
            Err(err) if is_disconnected(&err) => Err(io::ErrorKind::BrokenPipe.into()),
            result => Ok(result? as usize),
        }
    }

    pub fn close(&self) {
        unsafe { DisconnectNamedPipe(self.raw()) };
    }

    // Overlapped, as reads and writes on a synchronous handle wait for each
    // other.
    fn overlapped(&self, op: impl FnOnce(*mut OVERLAPPED) -> i32) -> io::Result<u32> {
        let event = owned(unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) })?;

        let mut overlapped: OVERLAPPED = unsafe { mem::zeroed() };
        overlapped.hEvent = event.as_raw_handle();

        if op(&mut overlapped) == 0 {
            let err = io::Error::last_os_error();

            if err.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(err);
            }
        }

        let mut transferred = 0;

        if unsafe { GetOverlappedResult(self.raw(), &overlapped, &mut transferred, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(transferred)
    }
}

fn is_disconnected(err: &io::Error) -> bool {
    [ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED]
        .iter()
        .any(|code| err.raw_os_error() == Some(*code as i32))
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

fn wide(string: String) -> Vec<u16> {
    OsStr::new(&string).encode_wide().chain([0]).collect()
}
