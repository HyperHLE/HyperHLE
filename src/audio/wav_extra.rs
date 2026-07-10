/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Decoders for compressed/companded RIFF WAVE formats that neither `hound`
//! nor Symphonia support:
//!
//! - `WAVE_FORMAT_ADPCM` (0x0002, Microsoft ADPCM)
//! - `WAVE_FORMAT_ALAW` (0x0006, ITU-T G.711 A-law)
//! - `WAVE_FORMAT_MULAW` (0x0007, ITU-T G.711 µ-law)
//! - `WAVE_FORMAT_DVI_ADPCM` / IMA ADPCM (0x0011)
//!
//! These formats were all natively playable by Core Audio on iPhone OS (see
//! Apple's "Core Audio Overview" and the `kAudioFormatALaw`,
//! `kAudioFormatULaw`, `kAudioFormatMicrosoftGSM` etc. constants in
//! `CoreAudioTypes.h` — A-law, µ-law and IMA ADPCM are listed among the iOS
//! supported codecs in Apple's Multimedia Programming Guide), so real apps
//! ship .wav sound effects in them (e.g. SnowCraft's `onetick2.wav`, which
//! previously failed with "wav: unsupported wave format" and left the game
//! without sound).
//!
//! References:
//! - Microsoft "Multimedia Programming Interface and Data Specifications 1.0"
//!   (RIFF/WAVE), and the WAVEFORMATEX documentation:
//!   <https://learn.microsoft.com/en-us/windows/win32/api/mmreg/ns-mmreg-waveformatex>
//! - MS ADPCM & IMA/DVI ADPCM block layouts:
//!   <https://wiki.multimedia.cx/index.php/Microsoft_ADPCM>
//!   <https://wiki.multimedia.cx/index.php/Microsoft_IMA_ADPCM>
//! - ITU-T G.711 (A-law/µ-law companding)
//! - Apple TN1081 "Understanding the Differences Between Apple and Windows
//!   IMA-ADPCM Compressed Sound Files":
//!   <https://developer.apple.com/library/archive/technotes/tn/tn1081.html>

use super::ima4::ImaAdpcmState;
use super::symphonia_formats::SymphoniaDecodedToPcm;

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_ADPCM: u16 = 0x0002;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_ALAW: u16 = 0x0006;
const WAVE_FORMAT_MULAW: u16 = 0x0007;
const WAVE_FORMAT_DVI_ADPCM: u16 = 0x0011;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

struct WavFmt {
    format_tag: u16,
    channels: u16,
    sample_rate: u32,
    block_align: u16,
    bits_per_sample: u16,
    /// Extra bytes after the fixed WAVEFORMATEX fields (`cbSize` payload).
    extra: Vec<u8>,
}

/// Attempt to decode a RIFF WAVE file in one of the compressed/companded
/// formats listed in the module docs to interleaved 16-bit little-endian PCM.
///
/// Returns [None] if the file is not a WAVE file, is in a format this module
/// doesn't handle (plain PCM/float are intentionally left to the callers'
/// other decoders), or is malformed.
pub fn decode_wav_compressed(bytes: &[u8]) -> Option<SymphoniaDecodedToPcm> {
    let (fmt, data, fact_samples) = parse_riff_wave(bytes)?;

    // Resolve WAVE_FORMAT_EXTENSIBLE to the real format tag: the SubFormat
    // GUID's first two bytes hold the format tag (see mmreg.h /
    // WAVEFORMATEXTENSIBLE documentation).
    let format_tag = if fmt.format_tag == WAVE_FORMAT_EXTENSIBLE {
        // extra = wValidBitsPerSample (2) + dwChannelMask (4) + SubFormat GUID
        if fmt.extra.len() < 8 {
            return None;
        }
        u16::from_le_bytes(fmt.extra[6..8].try_into().unwrap())
    } else {
        fmt.format_tag
    };

    if fmt.channels == 0 || fmt.sample_rate == 0 {
        return None;
    }

    let samples: Vec<i16> = match format_tag {
        // Leave the formats other decoders already handle alone.
        WAVE_FORMAT_PCM | WAVE_FORMAT_IEEE_FLOAT => return None,
        WAVE_FORMAT_MULAW => {
            if fmt.bits_per_sample != 8 {
                return None;
            }
            data.iter().map(|&b| mulaw_to_i16(b)).collect()
        }
        WAVE_FORMAT_ALAW => {
            if fmt.bits_per_sample != 8 {
                return None;
            }
            data.iter().map(|&b| alaw_to_i16(b)).collect()
        }
        WAVE_FORMAT_DVI_ADPCM => decode_ima_adpcm(&fmt, data)?,
        WAVE_FORMAT_ADPCM => decode_ms_adpcm(&fmt, data)?,
        other => {
            log!(
                "wav_extra: WAVE format tag {:#06x} is not supported by the compressed-WAV decoder.",
                other
            );
            return None;
        }
    };

    let mut samples = samples;
    // The `fact` chunk gives the true number of frames; ADPCM blocks are
    // padded so the raw decode can overshoot slightly. Trim if applicable.
    if let Some(fact) = fact_samples {
        let total = (fact as usize).saturating_mul(fmt.channels as usize);
        if total != 0 && total < samples.len() {
            samples.truncate(total);
        }
    }

    if samples.is_empty() {
        return None;
    }

    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }

    log!(
        "wav_extra: decoded WAVE format {:#06x} ({} ch, {} Hz) to 16-bit PCM.",
        format_tag,
        fmt.channels,
        fmt.sample_rate
    );

    Some(SymphoniaDecodedToPcm {
        bytes: out,
        sample_rate: fmt.sample_rate,
        channels: fmt.channels.into(),
    })
}

/// Parse the RIFF/WAVE container: returns the `fmt ` chunk, the `data` chunk
/// contents and the `fact` chunk's sample-frame count if present.
fn parse_riff_wave(bytes: &[u8]) -> Option<(WavFmt, &[u8], Option<u32>)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }

    let mut fmt: Option<WavFmt> = None;
    let mut data: Option<&[u8]> = None;
    let mut fact_samples: Option<u32> = None;

    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let chunk_size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body_start = pos + 8;
        // Tolerate a truncated final chunk (some encoders write a short
        // `data` chunk size or truncate the file).
        let body_end = body_start.saturating_add(chunk_size).min(bytes.len());
        let body = &bytes[body_start..body_end];

        match chunk_id {
            b"fmt " => {
                if body.len() < 16 {
                    return None;
                }
                let format_tag = u16::from_le_bytes(body[0..2].try_into().unwrap());
                let channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                let sample_rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                // nAvgBytesPerSec at 8..12 (unused)
                let block_align = u16::from_le_bytes(body[12..14].try_into().unwrap());
                let bits_per_sample = u16::from_le_bytes(body[14..16].try_into().unwrap());
                let extra = if body.len() >= 18 {
                    let cb_size = u16::from_le_bytes(body[16..18].try_into().unwrap()) as usize;
                    body.get(18..18 + cb_size.min(body.len() - 18))
                        .unwrap_or(&[])
                        .to_vec()
                } else {
                    Vec::new()
                };
                fmt = Some(WavFmt {
                    format_tag,
                    channels,
                    sample_rate,
                    block_align,
                    bits_per_sample,
                    extra,
                });
            }
            b"data" => {
                data = Some(body);
            }
            b"fact" => {
                if body.len() >= 4 {
                    fact_samples = Some(u32::from_le_bytes(body[0..4].try_into().unwrap()));
                }
            }
            _ => {}
        }

        // Chunks are word-aligned: an odd-sized chunk is followed by a pad
        // byte (RIFF specification).
        pos = body_start + chunk_size + (chunk_size & 1);
    }

    Some((fmt?, data?, fact_samples))
}

/// ITU-T G.711 µ-law expansion.
fn mulaw_to_i16(mu: u8) -> i16 {
    const BIAS: i32 = 0x84;
    let mu = !mu;
    let sign = (mu & 0x80) != 0;
    let exponent = ((mu >> 4) & 0x07) as i32;
    let mantissa = (mu & 0x0F) as i32;
    let magnitude = (((mantissa << 3) + BIAS) << exponent) - BIAS;
    if sign {
        -magnitude as i16
    } else {
        magnitude as i16
    }
}

/// ITU-T G.711 A-law expansion.
fn alaw_to_i16(a: u8) -> i16 {
    let a = a ^ 0x55;
    let mut magnitude = ((a & 0x0F) as i32) << 4;
    let exponent = ((a >> 4) & 0x07) as i32;
    match exponent {
        0 => magnitude += 8,
        1 => magnitude += 0x108,
        _ => {
            magnitude += 0x108;
            magnitude <<= exponent - 1;
        }
    }
    if a & 0x80 != 0 {
        magnitude as i16
    } else {
        -magnitude as i16
    }
}

/// Decode Microsoft/DVI IMA ADPCM (WAVE format 0x0011).
///
/// Block layout (per <https://wiki.multimedia.cx/index.php/Microsoft_IMA_ADPCM>):
/// each block starts with a 4-byte header per channel (initial predictor as
/// i16 LE, step index as u8, reserved u8), followed by the nibble data in
/// 4-byte groups per channel (interleaved by channel for stereo). Each 4-byte
/// group holds 8 samples, low nibble first.
fn decode_ima_adpcm(fmt: &WavFmt, data: &[u8]) -> Option<Vec<i16>> {
    if fmt.bits_per_sample != 4 {
        return None;
    }
    let channels = fmt.channels as usize;
    let block_align = fmt.block_align as usize;
    if channels == 0 || block_align < 4 * channels || block_align % 4 != 0 {
        return None;
    }

    // Samples per block: 1 (from the header) + 2 per data byte per channel.
    let data_bytes_per_channel = (block_align - 4 * channels) / channels;
    let samples_per_block = 1 + data_bytes_per_channel * 2;

    let mut out: Vec<i16> = Vec::new();

    for block in data.chunks(block_align) {
        if block.len() < 4 * channels {
            break;
        }
        // Per-channel decoder state from the block header. The header's
        // predictor is itself the first output sample of the block.
        let mut states: Vec<ImaAdpcmState> = Vec::with_capacity(channels);
        for ch in 0..channels {
            let base = ch * 4;
            let predictor = i16::from_le_bytes(block[base..base + 2].try_into().unwrap());
            let step_index = block[base + 2] as usize;
            if step_index > 88 {
                return None;
            }
            states.push(ImaAdpcmState::new(predictor, step_index));
        }

        // Interleaved output frames for this block.
        let mut block_samples: Vec<i16> = vec![0; samples_per_block * channels];
        for (ch, state) in states.iter().enumerate() {
            block_samples[ch] = state.predicted_sample;
        }

        // Data area: 4-byte groups, cycling through channels.
        let body = &block[4 * channels..];
        let mut sample_index_per_channel = vec![1usize; channels];
        for (group_idx, group) in body.chunks(4).enumerate() {
            if group.len() < 4 {
                break;
            }
            let ch = group_idx % channels;
            let state = &mut states[ch];
            for &byte in group {
                for nibble in [byte & 0xf, byte >> 4] {
                    let sample = state.decode_nibble(nibble);
                    let idx = sample_index_per_channel[ch];
                    if idx < samples_per_block {
                        block_samples[idx * channels + ch] = sample;
                        sample_index_per_channel[ch] = idx + 1;
                    }
                }
            }
        }

        // Only emit as many frames as were actually decoded (handles a
        // truncated final block).
        let frames_decoded = sample_index_per_channel.iter().copied().min().unwrap_or(0);
        out.extend_from_slice(&block_samples[..frames_decoded * channels]);
    }

    Some(out)
}

/// Microsoft ADPCM predictor coefficient pairs (coef1, coef2), fixed-point
/// with 8 fractional bits.
const MS_ADPCM_COEFFS: [(i32, i32); 7] = [
    (256, 0),
    (512, -256),
    (0, 64),
    (192, 64),
    (240, 0),
    (460, -208),
    (392, -232),
];

/// Microsoft ADPCM delta adaptation table.
const MS_ADPCM_ADAPTATION: [i32; 16] = [
    230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230,
];

struct MsAdpcmChannel {
    coef1: i32,
    coef2: i32,
    delta: i32,
    sample1: i32,
    sample2: i32,
}

impl MsAdpcmChannel {
    fn decode_nibble(&mut self, nibble: u8) -> i16 {
        let signed: i32 = if nibble & 8 != 0 {
            (nibble as i32) - 16
        } else {
            nibble as i32
        };
        let predictor = ((self.sample1 * self.coef1 + self.sample2 * self.coef2) >> 8)
            + signed * self.delta;
        let predictor = predictor.clamp(i16::MIN as i32, i16::MAX as i32);
        self.sample2 = self.sample1;
        self.sample1 = predictor;
        self.delta = ((MS_ADPCM_ADAPTATION[(nibble & 0xf) as usize] * self.delta) >> 8).max(16);
        predictor as i16
    }
}

/// Decode Microsoft ADPCM (WAVE format 0x0002).
///
/// Block layout (per <https://wiki.multimedia.cx/index.php/Microsoft_ADPCM>):
/// per channel: u8 predictor index; then per channel: i16 initial delta; then
/// per channel: i16 sample1; then per channel: i16 sample2. sample2 is the
/// older sample and is output first. The remaining bytes hold two 4-bit codes
/// each (high nibble first), cycling through channels per nibble.
fn decode_ms_adpcm(fmt: &WavFmt, data: &[u8]) -> Option<Vec<i16>> {
    if fmt.bits_per_sample != 4 {
        return None;
    }
    let channels = fmt.channels as usize;
    let block_align = fmt.block_align as usize;
    let header_size = 7 * channels;
    if channels == 0 || block_align <= header_size {
        return None;
    }

    let mut out: Vec<i16> = Vec::new();

    for block in data.chunks(block_align) {
        if block.len() < header_size {
            break;
        }

        let mut chans: Vec<MsAdpcmChannel> = Vec::with_capacity(channels);
        for ch in 0..channels {
            let predictor_index = block[ch] as usize;
            if predictor_index >= MS_ADPCM_COEFFS.len() {
                return None;
            }
            let (coef1, coef2) = MS_ADPCM_COEFFS[predictor_index];
            let delta_base = channels + ch * 2;
            let sample1_base = channels * 3 + ch * 2;
            let sample2_base = channels * 5 + ch * 2;
            let delta =
                i16::from_le_bytes(block[delta_base..delta_base + 2].try_into().unwrap()) as i32;
            let sample1 =
                i16::from_le_bytes(block[sample1_base..sample1_base + 2].try_into().unwrap())
                    as i32;
            let sample2 =
                i16::from_le_bytes(block[sample2_base..sample2_base + 2].try_into().unwrap())
                    as i32;
            chans.push(MsAdpcmChannel {
                coef1,
                coef2,
                delta,
                sample1,
                sample2,
            });
        }

        // The two header samples are emitted first, oldest (sample2) first.
        for ch in &chans {
            out.push(ch.sample2 as i16);
        }
        for ch in &chans {
            out.push(ch.sample1 as i16);
        }

        // Nibbles cycle through channels; high nibble first within a byte.
        let mut ch_index = 0usize;
        for &byte in &block[header_size..] {
            for nibble in [byte >> 4, byte & 0xf] {
                let sample = chans[ch_index].decode_nibble(nibble);
                out.push(sample);
                ch_index = (ch_index + 1) % channels;
            }
        }
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mulaw_extremes() {
        // 0xFF encodes 0 (positive), 0x7F encodes 0 (negative zero → 0).
        assert_eq!(mulaw_to_i16(0xFF), 0);
        assert_eq!(mulaw_to_i16(0x7F), 0);
        // 0x80 is the largest positive magnitude, 0x00 the largest negative.
        assert_eq!(mulaw_to_i16(0x80), 32124);
        assert_eq!(mulaw_to_i16(0x00), -32124);
    }

    #[test]
    fn test_alaw_extremes() {
        // 0xD5 encodes +8, 0x55 encodes -8 (A-law has no true zero).
        assert_eq!(alaw_to_i16(0xD5), 8);
        assert_eq!(alaw_to_i16(0x55), -8);
        // 0xAA / 0x2A are the max magnitude codes.
        assert_eq!(alaw_to_i16(0xAA), 32256);
        assert_eq!(alaw_to_i16(0x2A), -32256);
    }

    /// Round-trip a simple mono IMA ADPCM block through the decoder.
    #[test]
    fn test_ima_adpcm_block_header_sample() {
        // Block: predictor = 1000, step index = 0, one 4-byte data group of
        // zero nibbles (each zero nibble adds step_size >> 3).
        let fmt = WavFmt {
            format_tag: WAVE_FORMAT_DVI_ADPCM,
            channels: 1,
            sample_rate: 22050,
            block_align: 8,
            bits_per_sample: 4,
            extra: vec![],
        };
        let mut data = Vec::new();
        data.extend_from_slice(&1000i16.to_le_bytes());
        data.push(0); // step index
        data.push(0); // reserved
        data.extend_from_slice(&[0u8; 4]);
        let samples = decode_ima_adpcm(&fmt, &data).unwrap();
        // 1 header sample + 8 nibble samples
        assert_eq!(samples.len(), 9);
        assert_eq!(samples[0], 1000);
        // Zero nibbles with step index 0: each adds 7 >> 3 = 0, index drops
        // to 0 (clamped), so the signal stays flat.
        assert!(samples.iter().all(|&s| s == 1000));
    }

    #[test]
    fn test_ms_adpcm_header_samples() {
        // Mono block with predictor 0, delta 16, sample1=100, sample2=50,
        // no data bytes beyond header + 1 byte of zero nibbles.
        let fmt = WavFmt {
            format_tag: WAVE_FORMAT_ADPCM,
            channels: 1,
            sample_rate: 22050,
            block_align: 8,
            bits_per_sample: 4,
            extra: vec![],
        };
        let mut data = Vec::new();
        data.push(0); // predictor index
        data.extend_from_slice(&16i16.to_le_bytes()); // delta
        data.extend_from_slice(&100i16.to_le_bytes()); // sample1
        data.extend_from_slice(&50i16.to_le_bytes()); // sample2
        data.push(0); // one byte → two zero nibbles
        let samples = decode_ms_adpcm(&fmt, &data).unwrap();
        assert_eq!(samples.len(), 4);
        assert_eq!(samples[0], 50); // sample2 first (older)
        assert_eq!(samples[1], 100);
        // Zero nibble: predictor = (100*256 + 50*0) >> 8 = 100.
        assert_eq!(samples[2], 100);
    }
}
