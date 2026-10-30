//! A byte stream whose reads never block, built from a reader that does.
//!
//! The WebSocket I/O loop (`websocket.rs`) runs reads and writes on one
//! thread and needs `read()` to return `WouldBlock` when nothing has
//! arrived, as a non-blocking `TcpStream` does. A Java `SSLSocket` (the
//! Android `wss://` transport, `android_tls.rs`) has no non-blocking mode,
//! and a read timeout is no substitute: whether a TLS socket survives a
//! timeout in the middle of a record is up to the TLS implementation. So a
//! dedicated thread does plain blocking reads into a shared buffer, and
//! [`PumpedStream::read`] drains that buffer, reporting `WouldBlock` while
//! it is empty. Writes go straight to the writer on the caller's thread;
//! a TLS socket allows one reading and one writing thread at once.
//!
//! Platform-free so the host tests drive it over a real TCP socket.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};

/// What the reader thread has delivered.
#[derive(Default)]
struct Inbound {
    buf: VecDeque<u8>,
    /// Set once the reader stops: `Ok` at end of stream, `Err` with the
    /// read error. Reported after `buf` drains.
    end: Option<Result<(), (io::ErrorKind, String)>>,
}

/// Size of one blocking read on the reader thread.
const READ_CHUNK: usize = 16 * 1024;

/// Reads from the reader thread's buffer, writes to `W`.
pub(crate) struct PumpedStream<W> {
    inbound: Arc<Mutex<Inbound>>,
    writer: W,
}

/// Start a thread that reads `reader` until end of stream or an error, and
/// return the stream that drains it. Closing whatever `writer` writes to
/// (dropping it, for the Android socket) is what ends a reader blocked on
/// a quiet connection.
pub(crate) fn pumped<R, W>(mut reader: R, writer: W) -> io::Result<PumpedStream<W>>
where
    R: Read + Send + 'static,
{
    let inbound = Arc::new(Mutex::new(Inbound::default()));
    let shared = inbound.clone();
    std::thread::Builder::new()
        .name("net-ws-read".into())
        .spawn(move || {
            let mut chunk = vec![0u8; READ_CHUNK];
            let end = loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break Ok(()),
                    Ok(n) => lock(&shared).buf.extend(&chunk[..n]),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => break Err((e.kind(), e.to_string())),
                }
            };
            lock(&shared).end = Some(end);
        })?;
    Ok(PumpedStream { inbound, writer })
}

fn lock(m: &Mutex<Inbound>) -> std::sync::MutexGuard<'_, Inbound> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl<W> Read for PumpedStream<W> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut inbound = lock(&self.inbound);
        if !inbound.buf.is_empty() {
            let n = out.len().min(inbound.buf.len());
            for (dst, src) in out.iter_mut().zip(inbound.buf.drain(..n)) {
                *dst = src;
            }
            return Ok(n);
        }
        match &inbound.end {
            None => Err(io::ErrorKind::WouldBlock.into()),
            Some(Ok(())) => Ok(0),
            Some(Err((kind, msg))) => Err(io::Error::new(*kind, msg.clone())),
        }
    }
}

impl<W: Write> Write for PumpedStream<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A reader fed through a channel: each message is one `read`'s worth;
    /// `None` ends the stream, `Some(Err)` fails it.
    struct Scripted(std::sync::mpsc::Receiver<io::Result<Vec<u8>>>);

    impl Read for Scripted {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            match self.0.recv() {
                Ok(Ok(bytes)) => {
                    out[..bytes.len()].copy_from_slice(&bytes);
                    Ok(bytes.len())
                }
                Ok(Err(e)) => Err(e),
                Err(_) => Ok(0),
            }
        }
    }

    /// Poll `read` until it stops reporting `WouldBlock`.
    fn read_ready(s: &mut PumpedStream<Vec<u8>>, out: &mut [u8]) -> io::Result<usize> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match s.read(out) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "reader never delivered");
                    std::thread::sleep(Duration::from_millis(1));
                }
                other => return other,
            }
        }
    }

    #[test]
    fn empty_buffer_would_block_then_bytes_arrive_in_order() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut s = pumped(Scripted(rx), Vec::new()).unwrap();
        let mut out = [0u8; 8];
        assert_eq!(s.read(&mut out).unwrap_err().kind(), io::ErrorKind::WouldBlock);
        tx.send(Ok(b"hello".to_vec())).unwrap();
        tx.send(Ok(b" world".to_vec())).unwrap();
        let mut got: Vec<u8> = Vec::new();
        while got.len() < 11 {
            let n = read_ready(&mut s, &mut out).unwrap();
            got.extend(&out[..n]);
        }
        assert_eq!(got, b"hello world");
    }

    #[test]
    fn end_of_stream_reads_zero_after_the_buffer_drains() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut s = pumped(Scripted(rx), Vec::new()).unwrap();
        tx.send(Ok(b"ab".to_vec())).unwrap();
        drop(tx);
        let mut out = [0u8; 1];
        assert_eq!(read_ready(&mut s, &mut out).unwrap(), 1);
        assert_eq!(read_ready(&mut s, &mut out).unwrap(), 1);
        assert_eq!(read_ready(&mut s, &mut out).unwrap(), 0);
        assert_eq!(read_ready(&mut s, &mut out).unwrap(), 0, "stays ended");
    }

    #[test]
    fn a_read_error_surfaces_with_its_kind_after_buffered_bytes() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut s = pumped(Scripted(rx), Vec::new()).unwrap();
        tx.send(Ok(b"x".to_vec())).unwrap();
        tx.send(Err(io::Error::new(io::ErrorKind::ConnectionReset, "peer reset"))).unwrap();
        let mut out = [0u8; 4];
        assert_eq!(read_ready(&mut s, &mut out).unwrap(), 1);
        let e = read_ready(&mut s, &mut out).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionReset);
        assert!(e.to_string().contains("peer reset"));
    }

    #[test]
    fn writes_go_to_the_writer() {
        let (_tx, rx) = std::sync::mpsc::channel();
        let mut s = pumped(Scripted(rx), Vec::new()).unwrap();
        s.write_all(b"frame").unwrap();
        s.flush().unwrap();
        assert_eq!(s.writer, b"frame");
    }
}
