//! One native process per persistent project guest (ADR-103 D9).
#[cfg(not(unix))]
compile_error!("the project runner currently requires Unix domain sockets");

#[path = "../../../clawft-wasm/src/project_kernel/abi.rs"]
mod abi;

use abi::{Boot, MAX_FRAME};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use wasmtime::error::Context;
use wasmtime::{
    Caller, Config, Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder,
};
use wasmtime::{Result, bail, ensure};
use wasmtime_wasi::{FsPerms, WasiCtxBuilder, p1::WasiP1Ctx};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Launch {
    /// Must explicitly select this driver. A logical adapter never runs here.
    adapter: String,
    artifact: PathBuf,
    artifact_sha256: String,
    project_root: PathBuf,
    parent_socket: PathBuf,
    runtime_dir: PathBuf,
    project_id: String,
    user_pubkey: String,
    spawn_nonce: Option<String>,
    parent_policy: serde_json::Value,
    depth: u32,
    parent: String,
    lifetime_fuel: u64,
    memory_bytes: usize,
    lifetime_secs: u64,
}

struct Host {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
    boot: Vec<u8>,
    boot_taken: bool,
    parent: PathBuf,
    project: String,
    listener: UnixListener,
    pending: Option<UnixStream>,
    stop: Arc<AtomicBool>,
    revoked: PathBuf,
}

fn private(path: &Path, directory: bool) -> Result<()> {
    let m = fs::symlink_metadata(path)?;
    ensure!(
        !m.file_type().is_symlink(),
        "symlink refused: {}",
        path.display()
    );
    ensure!(
        m.uid() == unsafe { libc::geteuid() },
        "foreign owner: {}",
        path.display()
    );
    ensure!(
        m.mode() & 0o022 == 0,
        "writable by another user: {}",
        path.display()
    );
    if directory {
        ensure!(m.is_dir(), "not a directory");
    }
    Ok(())
}

fn lock(path: &Path) -> Result<File> {
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    ensure!(f.metadata()?.is_file(), "lock is not a regular file");
    // std's Unix implementation uses flock, matching the native chain lock.
    f.try_lock()
        .context("project runner or chain already in use")?;
    Ok(f)
}

// All socket operations are nonblocking under a shared absolute deadline.
// Epochs cannot interrupt Rust host calls; check the same cancellation flag here.
fn io_tick(stop: &AtomicBool, deadline: Instant) -> Result<()> {
    ensure!(
        !stop.load(Ordering::Relaxed) && !SIGNALLED.load(Ordering::Relaxed),
        "runner cancelled"
    );
    ensure!(Instant::now() < deadline, "socket deadline exceeded");
    Ok(())
}
fn frame(mut stream: &UnixStream, stop: &AtomicBool, deadline: Instant) -> Result<Vec<u8>> {
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
fn send(mut stream: &UnixStream, bytes: &[u8], stop: &AtomicBool, deadline: Instant) -> Result<()> {
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
fn connect(path: &Path, stop: &AtomicBool, deadline: Instant) -> Result<UnixStream> {
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

fn parent_call(host: &Host, input: &[u8]) -> Result<Vec<u8>> {
    let req: serde_json::Value = serde_json::from_slice(input)?;
    ensure!(
        req["project"].as_str() == Some(&host.project),
        "wrong parent scope"
    );
    ensure!(
        matches!(
            req["method"].as_str(),
            Some(
                "mesh.challenge"
                    | "mesh.register"
                    | "mesh.heartbeat"
                    | "mesh.unregister"
                    | "project.anchor.submit"
            )
        ),
        "parent method unavailable"
    );
    if let Some(project) = req["params"]["project_id"].as_str() {
        ensure!(
            project == host.project,
            "parent payload crosses project scope"
        );
    }
    // Recheck on every connection, including after parent replacement/restart.
    private(
        host.parent.parent().context("parent socket directory")?,
        true,
    )?;
    private(&host.parent, false)?;
    ensure!(
        fs::metadata(&host.parent)?.mode() & 0o077 == 0,
        "parent socket must be private"
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    let stream = connect(&host.parent, &host.stop, deadline)?;
    send(&stream, input, &host.stop, deadline)?;
    frame(&stream, &host.stop, deadline)
}

fn memory(caller: &mut Caller<'_, Host>) -> Result<Memory> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .context("guest memory missing")
}

fn exchange(
    mut c: Caller<'_, Host>,
    op: i32,
    ip: i32,
    il: i32,
    optr: i32,
    cap: i32,
) -> Result<i32> {
    ensure!(!c.data().stop.load(Ordering::Relaxed), "runner cancelled");
    ensure!(!c.data().revoked.try_exists()?, "project revoked");
    ensure!(
        ip >= 0
            && il >= 0
            && optr >= 0
            && cap >= 0
            && il as usize <= MAX_FRAME
            && cap as usize <= MAX_FRAME,
        "invalid ABI range"
    );
    let mem = memory(&mut c)?;
    // Check BOTH ranges before performing any external side effect.
    let size = mem.data_size(&c);
    ensure!(
        (ip as usize)
            .checked_add(il as usize)
            .is_some_and(|end| end <= size),
        "input outside memory"
    );
    ensure!(
        (optr as usize)
            .checked_add(cap as usize)
            .is_some_and(|end| end <= size),
        "output outside memory"
    );
    let mut input = vec![0; il as usize];
    mem.read(&c, ip as usize, &mut input)?;
    let out = match op {
        abi::BOOT => {
            ensure!(!c.data().boot_taken, "boot already consumed");
            c.data_mut().boot_taken = true;
            c.data().boot.clone()
        }
        abi::PARENT => match parent_call(c.data(), &input) {
            Ok(out) => out,
            Err(_) => serde_json::to_vec(
                &serde_json::json!({"ok":false,"error":"parent unavailable","error_kind":"parent_unavailable"}),
            )?,
        },
        abi::NEXT => {
            ensure!(c.data().pending.is_none(), "unanswered request");
            match c.data().listener.accept() {
                Ok((stream, _)) => match frame(
                    &stream,
                    &c.data().stop,
                    Instant::now() + Duration::from_millis(250),
                ) {
                    Ok(out) => {
                        c.data_mut().pending = Some(stream);
                        out
                    }
                    // A slow or malformed client must not kill the kernel.
                    Err(_) => Vec::new(),
                },
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                    Vec::new()
                }
                Err(e) => return Err(e.into()),
            }
        }
        abi::REPLY => {
            let stream = c.data_mut().pending.take().context("no pending request")?;
            // Disconnected callers do not end the guest's lifetime.
            let _ = send(
                &stream,
                &input,
                &c.data().stop,
                Instant::now() + Duration::from_millis(250),
            );
            Vec::new()
        }
        _ => bail!("unknown ABI operation"),
    };
    ensure!(out.len() <= cap as usize, "response exceeds guest buffer");
    mem.write(&mut c, optr as usize, &out)?;
    Ok(out.len() as i32)
}

struct Cleanup {
    socket: PathBuf,
    pid: PathBuf,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_file(&self.pid);
    }
}
static SIGNALLED: AtomicBool = AtomicBool::new(false);
extern "C" fn stop_signal(_: libc::c_int) {
    SIGNALLED.store(true, Ordering::Relaxed);
}

fn run(l: Launch) -> Result<()> {
    ensure!(
        l.adapter == "wasmtime-project-v1",
        "explicit wasmtime-project-v1 adapter required"
    );
    ensure!(
        (1..=10_000_000_000_000).contains(&l.lifetime_fuel),
        "invalid lifetime fuel"
    );
    ensure!(
        (16 * 1024 * 1024..=512 * 1024 * 1024).contains(&l.memory_bytes),
        "invalid memory bound"
    );
    ensure!(
        (1..=604800).contains(&l.lifetime_secs),
        "invalid lifetime deadline"
    );
    ensure!(
        l.project_root.is_absolute()
            && l.runtime_dir.is_absolute()
            && l.parent_socket.is_absolute(),
        "absolute paths required"
    );
    let root = fs::canonicalize(&l.project_root)?;
    ensure!(root == l.project_root, "project root must be canonical");
    private(&l.runtime_dir, true)?;
    ensure!(
        fs::canonicalize(&l.runtime_dir)? == l.runtime_dir && !l.runtime_dir.starts_with(&root),
        "runner runtime must be canonical and outside the guest project"
    );
    ensure!(
        fs::metadata(&l.runtime_dir)?.mode() & 0o077 == 0,
        "runtime directory must be 0700"
    );
    let state = root.join(".weftos");
    private(&state, true)?;
    // Fresh guest-created private files are never group/world-readable.
    unsafe {
        libc::umask(0o077);
    }
    let mut _runner_lock = lock(&l.runtime_dir.join("kernel.lock"))?;
    _runner_lock.set_len(0)?;
    writeln!(_runner_lock, "{}", std::process::id())?;
    _runner_lock.sync_all()?;
    fs::create_dir_all(state.join("chain"))?;
    private(&state.join("chain"), true)?;
    let _chain_lock = lock(&state.join("chain/chain.lock"))?;
    match fs::symlink_metadata(state.join("project.key")) {
        Ok(m) => {
            private(&state.join("project.key"), false)?;
            ensure!(
                m.is_file() && m.mode() & 0o077 == 0 && m.len() == 32,
                "invalid private project key"
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    ensure!(
        fs::metadata(&l.artifact)?.len() <= 128 * 1024 * 1024,
        "artifact too large"
    );
    let bytes = fs::read(&l.artifact)?;
    ensure!(bytes.len() <= 128 * 1024 * 1024, "artifact too large");
    let hash = format!("{:x}", Sha256::digest(&bytes));
    ensure!(
        hash == l.artifact_sha256,
        "guest artifact changed; refusing adoption"
    );
    // Compile the exact bytes just hashed, never deserialize a native code cache.
    let mut cfg = Config::new();
    cfg.consume_fuel(true).epoch_interruption(true);
    let engine = Engine::new(&cfg)?;
    let module = Module::new(&engine, &bytes)?;
    let socket = l.runtime_dir.join("kernel.sock");
    let listener =
        UnixListener::bind(&socket).context("socket exists or cannot bind; no stale unlink")?;
    let _cleanup = Cleanup {
        socket: socket.clone(),
        pid: l.runtime_dir.join("kernel.pid"),
    };
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    fs::write(
        l.runtime_dir.join("kernel.pid"),
        std::process::id().to_string(),
    )?;
    let boot = Boot {
        abi: abi::ABI,
        project_id: l.project_id.clone(),
        root_sha256: format!("{:x}", Sha256::digest(root.as_os_str().as_bytes())),
        user_pubkey: l.user_pubkey,
        spawn_nonce: l.spawn_nonce,
        parent_policy: l.parent_policy,
        pid: std::process::id(),
        socket: socket.to_string_lossy().into_owned(),
        runtime_dir: l.runtime_dir.to_string_lossy().into_owned(),
        artifact_sha256: hash,
        depth: l.depth,
        parent: l.parent,
    };
    let mut wasi = WasiCtxBuilder::new();
    // Deliberately no inherit_env, stdio, sockets, network or HOME preopen.
    wasi.preopened_dir(&state, "/project", FsPerms::ReadWrite)?;
    let stop = Arc::new(AtomicBool::new(false));
    let mut store = Store::new(
        &engine,
        Host {
            wasi: wasi.build_p1(),
            limits: StoreLimitsBuilder::new()
                .memory_size(l.memory_bytes)
                .table_elements(10_000)
                .instances(1)
                .memories(1)
                .tables(1)
                .build(),
            boot: serde_json::to_vec(&boot)?,
            boot_taken: false,
            parent: l.parent_socket,
            project: l.project_id,
            listener,
            pending: None,
            stop: stop.clone(),
            revoked: l.runtime_dir.join("revoked"),
        },
    );
    store.limiter(|h| &mut h.limits);
    store.set_fuel(l.lifetime_fuel)?; // Set exactly once, including instantiation/start.
    store.set_epoch_deadline(1);
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |h: &mut Host| &mut h.wasi)?;
    linker.func_wrap("weftos_project_v1", "exchange", exchange)?;
    unsafe {
        libc::signal(
            libc::SIGTERM,
            stop_signal as *const () as libc::sighandler_t,
        );
        libc::signal(libc::SIGINT, stop_signal as *const () as libc::sighandler_t);
    }
    let timer_engine = engine.clone();
    let timer_stop = stop.clone();
    let timer = std::thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(l.lifetime_secs);
        while !timer_stop.load(Ordering::Relaxed)
            && !SIGNALLED.load(Ordering::Relaxed)
            && Instant::now() < until
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        timer_stop.store(true, Ordering::Relaxed);
        timer_engine.increment_epoch();
    });
    let result = (|| {
        let instance = linker.instantiate(&mut store, &module)?;
        instance
            .get_typed_func::<(), ()>(&mut store, "_start")?
            .call(&mut store, ())
    })();
    stop.store(true, Ordering::Relaxed);
    let _ = timer.join();
    match result {
        Err(e)
            if e.downcast_ref::<wasmtime_wasi::I32Exit>()
                .is_some_and(|exit| exit.0 == 0) =>
        {
            Ok(())
        }
        result => result.context("persistent project guest stopped"),
    }
}

fn main() -> Result<()> {
    let config = std::env::args_os()
        .nth(1)
        .context("usage: weftos-wasm-project-runner /absolute/launch.json")?;
    let path = Path::new(&config);
    ensure!(path.is_absolute(), "absolute launch config required");
    private(path, false)?;
    ensure!(
        fs::metadata(path)?.mode() & 0o077 == 0,
        "launch config must be 0600"
    );
    ensure!(
        fs::metadata(path)?.len() <= MAX_FRAME as u64,
        "launch config too large"
    );
    run(serde_json::from_slice(&fs::read(path)?)?)
}

#[cfg(test)]
mod bounded_io_tests {
    use super::*;

    #[test]
    fn trickle_cannot_extend_absolute_read_deadline() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || {
            for _ in 0..100 {
                if writer.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
        });
        let start = Instant::now();
        assert!(
            frame(
                &reader,
                &AtomicBool::new(false),
                start + Duration::from_millis(80)
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_millis(500));
        drop(reader);
        t.join().unwrap();
    }

    #[test]
    fn cancellation_interrupts_host_read() {
        let (reader, _writer) = UnixStream::pair().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            signal.store(true, Ordering::Relaxed);
        });
        let start = Instant::now();
        assert!(frame(&reader, &stop, start + Duration::from_secs(2)).is_err());
        assert!(start.elapsed() < Duration::from_millis(500));
        t.join().unwrap();
    }

    #[test]
    fn stalled_writer_has_absolute_deadline() {
        let (writer, _reader) = UnixStream::pair().unwrap();
        let start = Instant::now();
        assert!(
            send(
                &writer,
                &vec![0; 8 * 1024 * 1024],
                &AtomicBool::new(false),
                start + Duration::from_millis(80)
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_millis(500));
    }
}
