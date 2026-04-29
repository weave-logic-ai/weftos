//! Minimal RIFF/WAV header writer for 16 kHz mono `s16le` PCM.
//!
//! Just enough to produce the multipart body whisper.cpp's `/inference`
//! endpoint accepts. Lifted in spirit from `clawft-service-whisper::wav`
//! but intentionally re-implemented locally so the voice channel does
//! not pull `clawft-kernel` (whisper-service's dep tree) into its build.

/// Wrap a `s16le` PCM buffer in a RIFF/WAV header.
///
/// `samples` is interleaved `i16` mono. Returns the full WAV byte stream.
pub fn pcm_s16le_to_wav(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample) / 8;
    let block_align = channels * bits_per_sample / 8;
    let data_bytes = (samples.len() * 2) as u32;
    let riff_size = 36 + data_bytes;

    let mut buf = Vec::with_capacity(44 + samples.len() * 2);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&riff_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&bits_per_sample.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_bytes.to_le_bytes());
    for s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    buf
}

/// Decode an `s16le` mono WAV (44-byte canonical header) back to PCM.
///
/// Used by tests + by the playback path when the TTS daemon hands back a
/// WAV instead of raw PCM. Tolerates extra "fact"/"LIST" chunks by
/// scanning for the "data" tag. Returns `(samples, sample_rate)`.
pub fn wav_to_pcm_s16le(bytes: &[u8]) -> Result<(Vec<i16>, u32), &'static str> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file");
    }
    // fmt chunk should be at offset 12.
    if &bytes[12..16] != b"fmt " {
        return Err("missing fmt chunk");
    }
    let sample_rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    let bits_per_sample = u16::from_le_bytes(bytes[34..36].try_into().unwrap());
    if bits_per_sample != 16 {
        return Err("only 16-bit PCM supported");
    }
    // Find the "data" chunk.
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let tag = &bytes[i..i + 4];
        let size = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
        if tag == b"data" {
            let start = i + 8;
            let end = start + size;
            if end > bytes.len() {
                return Err("truncated data chunk");
            }
            let mut samples = Vec::with_capacity(size / 2);
            for chunk in bytes[start..end].chunks_exact(2) {
                samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
            }
            return Ok((samples, sample_rate));
        }
        i += 8 + size;
    }
    Err("no data chunk")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_pcm() {
        let pcm: Vec<i16> = (0..1_000).map(|i| (i % 1000) as i16).collect();
        let wav = pcm_s16le_to_wav(&pcm, 16_000);
        let (decoded, sr) = wav_to_pcm_s16le(&wav).unwrap();
        assert_eq!(sr, 16_000);
        assert_eq!(decoded, pcm);
    }

    #[test]
    fn rejects_non_riff() {
        let err = wav_to_pcm_s16le(&[0u8; 64]).unwrap_err();
        assert!(err.contains("RIFF"));
    }

    #[test]
    fn rejects_short_input() {
        assert!(wav_to_pcm_s16le(&[0u8; 8]).is_err());
    }

    #[test]
    fn header_size_for_empty_pcm() {
        let wav = pcm_s16le_to_wav(&[], 16_000);
        assert_eq!(wav.len(), 44);
        let (decoded, _sr) = wav_to_pcm_s16le(&wav).unwrap();
        assert!(decoded.is_empty());
    }
}
