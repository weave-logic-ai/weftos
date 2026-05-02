//! Minimal WAV-wrapping for PCM chunks.
//!
//! The whisper HTTP service (per `whisper-service-api.md` §6 step 2)
//! reads the WAV header to determine sample rate; it rejects raw PCM.
//! We hand-write a 44-byte RIFF header rather than pulling `hound` for
//! what amounts to 12 `u32`s.
//!
//! Format produced is always:
//!
//! - RIFF/WAVE container
//! - `fmt ` chunk of size 16 (PCM, no extension)
//! - AudioFormat = 1 (PCM)
//! - signed 16-bit little-endian samples
//! - mono or stereo per `channels`
//!
//! The server's native rate is 16 kHz mono; we always ship that shape
//! today, but [`write_wav`] is parameterised so a future multi-channel
//! or resampled-upstream path doesn't need a rewrite.

/// Bytes per sample for s16le.
const BYTES_PER_SAMPLE: u16 = 2;
/// PCM AudioFormat marker.
const AUDIO_FORMAT_PCM: u16 = 1;
/// Bit depth of each sample.
const BITS_PER_SAMPLE: u16 = 16;

/// Wrap raw s16le PCM bytes in a WAV container.
///
/// # Arguments
/// * `pcm_s16le` — raw PCM payload; length must be a multiple of
///   `channels * 2` bytes, otherwise the trailing odd byte is dropped
///   from the declared data-chunk size (but kept in the output for
///   whisper to deal with — matches the permissive Python snippet in
///   the service-API doc §6).
/// * `sample_rate` — Hz (e.g. 16000).
/// * `channels` — channel count (1 for mono, 2 for stereo).
///
/// # Returns
/// A `Vec<u8>` holding a complete WAV file suitable for POSTing as the
/// `file` multipart field to `POST /inference`.
pub fn write_wav(pcm_s16le: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_len = pcm_s16le.len() as u32;
    // RIFF size = 4 ("WAVE") + (8 + 16 fmt) + (8 + data_len). Total
    // file size = 8 + RIFF size.
    let riff_size: u32 = 4 + (8 + 16) + (8 + data_len);
    let byte_rate: u32 = sample_rate * channels as u32 * BYTES_PER_SAMPLE as u32;
    let block_align: u16 = channels * BYTES_PER_SAMPLE;

    let mut buf = Vec::with_capacity(44 + pcm_s16le.len());
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&riff_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // `fmt ` subchunk.
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // subchunk size
    buf.extend_from_slice(&AUDIO_FORMAT_PCM.to_le_bytes());
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());

    // `data` subchunk.
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    buf.extend_from_slice(pcm_s16le);

    buf
}

/// Parse a WAV file and return its raw PCM body + (sample_rate, channels).
///
/// Forgiving enough to accept the common subset: RIFF/WAVE, PCM
/// (AudioFormat=1), s16le. Rejects anything else with an `Err(reason)`.
///
/// Used only by the [`crate`]'s test harness (`examples/publish_wav.rs`
/// feeds a real WAV file into substrate), not by the runtime pipeline.
pub fn parse_wav(bytes: &[u8]) -> Result<(Vec<u8>, u32, u16), &'static str> {
    if bytes.len() < 44 {
        return Err("wav: too short for header");
    }
    if &bytes[0..4] != b"RIFF" {
        return Err("wav: missing RIFF marker");
    }
    if &bytes[8..12] != b"WAVE" {
        return Err("wav: missing WAVE marker");
    }

    // Walk sub-chunks looking for `fmt ` + `data`. Not every producer
    // puts them back-to-back starting at offset 12 (fmt chunks can have
    // extensions, producers may insert LIST/INFO), so do a proper scan.
    let mut pos = 12usize;
    let mut fmt: Option<(u32, u16, u16)> = None; // (sample_rate, channels, bits)
    let mut audio_format: u16 = 0;
    let mut data: Option<Vec<u8>> = None;

    while pos + 8 <= bytes.len() {
        let tag = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        pos += 8;
        if pos + size > bytes.len() {
            return Err("wav: chunk size exceeds file");
        }
        match tag {
            b"fmt " => {
                if size < 16 {
                    return Err("wav: fmt chunk too small");
                }
                audio_format = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]);
                let channels = u16::from_le_bytes([bytes[pos + 2], bytes[pos + 3]]);
                let sample_rate = u32::from_le_bytes([
                    bytes[pos + 4],
                    bytes[pos + 5],
                    bytes[pos + 6],
                    bytes[pos + 7],
                ]);
                let bits =
                    u16::from_le_bytes([bytes[pos + 14], bytes[pos + 15]]);
                fmt = Some((sample_rate, channels, bits));
            }
            b"data" => {
                data = Some(bytes[pos..pos + size].to_vec());
            }
            _ => {
                // Skip unknown chunks (LIST, INFO, bext, …).
            }
        }
        // Chunks are word-aligned; if size is odd the spec pads to even.
        pos += size + (size & 1);
        if data.is_some() && fmt.is_some() {
            break;
        }
    }

    let (sr, ch, bits) = fmt.ok_or("wav: missing fmt chunk")?;
    let pcm = data.ok_or("wav: missing data chunk")?;
    if audio_format != AUDIO_FORMAT_PCM {
        return Err("wav: only PCM (format=1) supported");
    }
    if bits != BITS_PER_SAMPLE {
        return Err("wav: only 16-bit PCM supported");
    }
    Ok((pcm, sr, ch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_parse_roundtrips() {
        let pcm: Vec<u8> = (0..1000u16).flat_map(|s| s.to_le_bytes()).collect();
        let wav = write_wav(&pcm, 16000, 1);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        let (body, sr, ch) = parse_wav(&wav).unwrap();
        assert_eq!(sr, 16000);
        assert_eq!(ch, 1);
        assert_eq!(body, pcm);
    }

    #[test]
    fn header_is_exactly_44_bytes() {
        // For PCM, fmt subchunk size = 16, so the header is 12 (RIFF) +
        // 24 (fmt) + 8 (data header) = 44 bytes before the payload.
        let wav = write_wav(&[], 16000, 1);
        assert_eq!(wav.len(), 44);
    }

    #[test]
    fn parse_rejects_short_input() {
        assert!(parse_wav(&[0u8; 10]).is_err());
    }

    #[test]
    fn parse_rejects_non_riff() {
        let mut bogus = vec![0u8; 44];
        bogus[0..4].copy_from_slice(b"FUZZ");
        assert!(parse_wav(&bogus).is_err());
    }

    #[test]
    fn stereo_declares_correct_byte_rate() {
        // Stereo @ 16 kHz = 16000 * 2ch * 2B = 64000 B/s. Byte rate is
        // at offset 28..32. Smoke test to catch arithmetic drift.
        let wav = write_wav(&[0u8; 4], 16000, 2);
        let br = u32::from_le_bytes([wav[28], wav[29], wav[30], wav[31]]);
        assert_eq!(br, 64_000);
    }
}
