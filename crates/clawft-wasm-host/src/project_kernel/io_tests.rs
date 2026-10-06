//! Absolute-deadline and cancellation behaviour of the bridge socket I/O.
use super::*;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use std::io::Write;
use std::os::unix::net::UnixStream;

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
