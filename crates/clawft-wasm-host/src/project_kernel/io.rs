//! Nonblocking, deadline-bounded socket I/O for the runner's bridge.
use super::{SIGNALLED, MAX_FRAME};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use wasmtime::{Result, bail, ensure};

// All socket operations are nonblocking under a shared absolute deadline.
// Epochs cannot interrupt Rust host calls; check the same cancellation flag here.
pub(crate) fn io_tick(stop: &AtomicBool, deadline: Instant) -> Result<()> {
    ensure!(
        !stop.load(Ordering::Relaxed) && !SIGNALLED.load(Ordering::Relaxed),
        "runner cancelled"
    );
    ensure!(Instant::now() < deadline, "socket deadline exceeded");
    Ok(())
}
pub(crate) fn frame(mut stream: &UnixStream, stop: &AtomicBool, deadline: Instant) -> Result<Vec<u8>> {
    stream.set_nonblocking(true)?;
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        io_tick(stop, deadline)?;
        match stream.read(&mut buf) {
            Ok(0) => bail!("incomplete frame"),
            Ok(n) => {
                let end = buf[..n].iter().position(|b| *b == b'\n');
                out.extend_from_slice(&buf[..end.unwrap_or(n)]);
                ensure!(out.len() <= MAX_FRAME, "oversize frame");
                if end.is_some() {
                    return Ok(out);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
}
pub(crate) fn send(mut stream: &UnixStream, bytes: &[u8], stop: &AtomicBool, deadline: Instant) -> Result<()> {
    stream.set_nonblocking(true)?;
    let mut data = bytes.to_vec();
    data.push(b'\n');
    let mut offset = 0;
    while offset < data.len() {
        io_tick(stop, deadline)?;
        match stream.write(&data[offset..]) {
            Ok(0) => bail!("socket closed"),
            Ok(n) => offset += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
pub(crate) fn connect(path: &Path, stop: &AtomicBool, deadline: Instant) -> Result<UnixStream> {
    let bytes = path.as_os_str().as_bytes();
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    ensure!(
        bytes.len() < addr.sun_path.len() && !bytes.contains(&0),
        "invalid socket path"
    );
    addr.sun_family = libc::AF_UNIX as _;
    for (to, from) in addr.sun_path.iter_mut().zip(bytes) {
        *to = *from as _;
    }
    let len = std::mem::size_of_val(&addr) as libc::socklen_t;
    #[cfg(target_os = "macos")]
    {
        addr.sun_len = len as _;
    }
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    ensure!(fd >= 0, "socket: {}", std::io::Error::last_os_error());
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    stream.set_nonblocking(true)?;
    loop {
        io_tick(stop, deadline)?;
        let rc = unsafe {
            libc::connect(
                stream.as_raw_fd(),
                (&addr as *const libc::sockaddr_un).cast(),
                len,
            )
        };
        if rc == 0 {
            return Ok(stream);
        }
        let e = std::io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EISCONN) => return Ok(stream),
            Some(libc::EAGAIN)
            | Some(libc::EINPROGRESS)
            | Some(libc::EALREADY)
            | Some(libc::EINTR) => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => return Err(e.into()),
        }
    }
}
