use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::thread;
use std::time::Duration;

use bare_sidecar::{Sidecar, spawn};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn read_to_end(mut sidecar: &Sidecar) -> Vec<u8> {
    let mut data = Vec::new();
    sidecar.read_to_end(&mut data).unwrap();
    data
}

#[test]
fn echoes_ipc_data() {
    let mut sidecar = spawn(fixture("echo.js")).unwrap();

    sidecar.write_all(b"Hello world").unwrap();

    let mut buf = [0; 11];
    sidecar.read_exact(&mut buf).unwrap();

    assert_eq!(&buf, b"Hello world");
}

#[test]
fn passes_arguments() {
    let mut sidecar = Sidecar::builder()
        .args(["foo", "bar"])
        .spawn(fixture("argv.js"))
        .unwrap();

    assert_eq!(read_to_end(&sidecar), br#"["foo","bar"]"#);
    assert!(sidecar.wait().unwrap().success());
}

#[test]
fn reports_the_exit_code() {
    let mut sidecar = spawn(fixture("exit.js")).unwrap();

    assert_eq!(sidecar.wait().unwrap().code(), Some(3));
    assert_eq!(read_to_end(&sidecar), b"");
}

#[test]
fn exits_before_the_end_of_ipc() {
    let mut sidecar = spawn(fixture("ipc-end-exit.js")).unwrap();

    assert_eq!(read_to_end(&sidecar), b"");
    assert_eq!(
        sidecar.try_wait().unwrap().and_then(|status| status.code()),
        Some(3)
    );
}

#[test]
fn exits_after_ipc_destroy_while_writing() {
    let mut sidecar = spawn(fixture("ipc-destroy-exit.js")).unwrap();

    while sidecar.try_wait().unwrap().is_none() {
        let _ = sidecar.write(b"Hello world");

        thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(sidecar.wait().unwrap().code(), Some(3));
}

#[test]
fn pipes_stdout() {
    let mut sidecar = Sidecar::builder()
        .stdout(Stdio::piped())
        .spawn(fixture("log.js"))
        .unwrap();

    let mut stdout = String::new();
    sidecar
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();

    assert_eq!(stdout.lines().collect::<Vec<_>>(), ["Hello world"]);
}

#[test]
fn terminates() {
    let mut sidecar = spawn(fixture("sleep.js")).unwrap();

    sidecar.terminate().unwrap();

    let status = sidecar.wait().unwrap();

    #[cfg(unix)]
    assert_eq!(
        std::os::unix::process::ExitStatusExt::signal(&status),
        Some(libc::SIGTERM)
    );

    // Windows has no signals, and terminates the process with exit code 1.
    #[cfg(windows)]
    assert_eq!(status.code(), Some(1));
}

#[test]
fn closes() {
    let mut sidecar = spawn(fixture("linger.js")).unwrap();

    let status = sidecar.close().unwrap();

    #[cfg(unix)]
    assert_eq!(
        std::os::unix::process::ExitStatusExt::signal(&status),
        Some(libc::SIGTERM)
    );

    #[cfg(windows)]
    assert_eq!(status.code(), Some(1));

    assert_eq!(sidecar.try_wait().unwrap(), Some(status));
}

#[test]
fn kills() {
    let mut sidecar = spawn(fixture("sleep.js")).unwrap();

    sidecar.kill().unwrap();

    let status = sidecar.wait().unwrap();

    #[cfg(unix)]
    assert_eq!(
        std::os::unix::process::ExitStatusExt::signal(&status),
        Some(libc::SIGKILL)
    );

    #[cfg(windows)]
    assert_eq!(status.code(), Some(1));
}

#[test]
fn streams_large_data() {
    let data: Vec<u8> = (0..=255).cycle().take(256 * 4096).collect();

    let sidecar = spawn(fixture("echo.js")).unwrap();

    thread::scope(|scope| {
        scope.spawn(|| (&sidecar).write_all(&data).unwrap());

        let mut echoed = vec![0; data.len()];
        (&sidecar).read_exact(&mut echoed).unwrap();

        assert_eq!(echoed, data);
    });
}

#[test]
fn bundles_the_bare_version_of_the_manifest() {
    let (_, version) = bare_sidecar::versions()
        .iter()
        .find(|(name, _)| *name == "bare")
        .unwrap();

    let sidecar = spawn(fixture("version.js")).unwrap();

    assert_eq!(read_to_end(&sidecar), format!("v{version}").as_bytes());
}

#[cfg(unix)]
#[test]
fn terminates_and_waits_for_the_process_after_drop() {
    let sidecar = spawn(fixture("linger.js")).unwrap();
    let pid = sidecar.id();

    drop(sidecar);

    let unreaped = || {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };

        unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
            ) == 0
        }
    };

    for _ in 0..100 {
        if !unreaped() {
            return;
        }

        thread::sleep(Duration::from_millis(50));
    }

    panic!("Process {pid} was not waited for");
}
