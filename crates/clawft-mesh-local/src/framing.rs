//! Newline-delimited JSON framing with a bounded line reader and deadlines.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::proto::Frame;

/// Maximum bytes in one line, including the object but not the newline.
pub const MAX_LINE: usize = 1 << 20;
/// Deadline for the first message (`hello`) on a connection.
pub const HELLO_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("line exceeds {MAX_LINE} bytes")]
    LineTooLong,
    #[error("connection closed mid-line")]
    Truncated,
    #[error("deadline exceeded")]
    Deadline,
    #[error("malformed frame: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Reads one JSON object per line, never buffering more than [`MAX_LINE`].
pub struct FrameReader<R> {
    inner: BufReader<R>,
    buf: Vec<u8>,
    max: usize,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub fn new(r: R) -> Self {
        Self::with_limit(r, MAX_LINE)
    }

    pub fn with_limit(r: R, max: usize) -> Self {
        Self { inner: BufReader::new(r), buf: Vec::new(), max }
    }

    /// Next frame, or `None` on a clean EOF between lines. Blank lines are
    /// skipped. After any error the reader must be discarded.
    pub async fn read_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        loop {
            self.buf.clear();
            loop {
                let chunk = self.inner.fill_buf().await?;
                if chunk.is_empty() {
                    return if self.buf.is_empty() {
                        Ok(None)
                    } else {
                        Err(FrameError::Truncated)
                    };
                }
                let (take, done) = match chunk.iter().position(|&b| b == b'\n') {
                    Some(i) => (i, true),
                    None => (chunk.len(), false),
                };
                if self.buf.len() + take > self.max {
                    return Err(FrameError::LineTooLong);
                }
                self.buf.extend_from_slice(&chunk[..take]);
                self.inner.consume(take + usize::from(done));
                if done {
                    break;
                }
            }
            if self.buf.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            return Ok(Some(serde_json::from_slice(&self.buf)?));
        }
    }

    /// [`read_frame`](Self::read_frame) with a deadline.
    pub async fn read_frame_within(&mut self, d: Duration) -> Result<Option<Frame>, FrameError> {
        tokio::time::timeout(d, self.read_frame()).await.map_err(|_| FrameError::Deadline)?
    }
}

/// Serialize and write one frame followed by a newline.
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, f: &Frame) -> Result<(), FrameError> {
    let mut line = serde_json::to_vec(f)?;
    if line.len() > MAX_LINE {
        return Err(FrameError::LineTooLong);
    }
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await?;
    Ok(())
}
