/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Audio file decoding and OpenAL bindings.

mod caf_decoder;
mod ima4;
pub mod openal;
pub mod symphonia_formats;

pub use ima4::decode_ima4;

use crate::fs::{Fs, GuestPath};
use std::io::Cursor;

#[derive(Debug)]
pub enum AudioFileOpenError {
    FileReadError,
    FileDecodeError,
}

#[derive(Debug)]
pub enum AudioFormat {
    LinearPcm {
        is_float: bool,
        is_little_endian: bool,
    },
    /// MPEG-4 AAC. The packets exposed by [AudioFile::read_bytes] are ADTS
    /// frames (each packet carries its own 7-byte ADTS header), which is what
    /// `audio_queue::decode_buffer` knows how to decode.
    Mpeg4Aac,
}

#[derive(Debug)]
pub struct AudioDescription {
    pub sample_rate: f64,
    pub format: AudioFormat,
    pub bytes_per_packet: u32,
    pub frames_per_packet: u32,
    pub channels_per_frame: u32,
    pub bits_per_channel: u32,
}

pub struct AacPackets {
    pub sample_rate: f64,
    pub channels_per_frame: u32,
    pub packet_bytes: Vec<u8>,
    pub packet_offsets: Vec<usize>,
    pub magic_cookie: Vec<u8>,
}

impl AacPackets {
    pub fn packet_count(&self) -> u64 {
        self.packet_offsets.len().saturating_sub(1) as u64
    }

    pub fn byte_count(&self) -> u64 {
        self.packet_bytes.len() as u64
    }

    pub fn packet_size_upper_bound(&self) -> u32 {
        self.packet_offsets
            .windows(2)
            .map(|w| (w[1] - w[0]) as u32)
            .max()
            .unwrap_or(0)
    }

    /// Byte offset and size of the packet with the given index, or [None] if
    /// the index is out of range.
    pub fn packet_info(&self, index: u64) -> Option<(u64, u32)> {
        let index: usize = index.try_into().ok()?;
        if index + 1 >= self.packet_offsets.len() {
            return None;
        }
        let start = self.packet_offsets[index];
        let end = self.packet_offsets[index + 1];
        Some((start as u64, (end - start) as u32))
    }

    pub fn read_bytes(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, ()> {
        let start = offset as usize;
        let src = self.packet_bytes.get(start..).ok_or(())?;
        let n = buffer.len().min(src.len());
        buffer[..n].copy_from_slice(&src[..n]);
        Ok(n)
    }
}

pub struct AudioFile {
    raw_data: std::sync::Arc<Vec<u8>>,
    inner: AudioFileInner,
}

impl Clone for AudioFile {
    fn clone(&self) -> Self {
        let inner = Self::parse_inner(self.raw_data.as_ref().clone()).unwrap();
        AudioFile {
            raw_data: self.raw_data.clone(),
            inner,
        }
    }
}

enum AudioFileInner {
    Wave(hound::WavReader<Cursor<Vec<u8>>>),
    Symphonia(symphonia_formats::SymphoniaDecodedToPcm),
    Aac(AacPackets),
}

impl AudioFile {
    pub fn open_for_reading<P: AsRef<GuestPath>>(
        path: P,
        fs: &Fs,
    ) -> Result<Self, AudioFileOpenError> {
        let Ok(bytes) = fs.read(path.as_ref()) else {
            return Err(AudioFileOpenError::FileReadError);
        };
        if let Ok(file) = Self::read_from_vec(bytes) {
            Ok(file)
        } else {
            log!(
                "Could not decode audio file at path {:?}, likely an unimplemented file format.",
                path.as_ref()
            );
            Err(AudioFileOpenError::FileReadError)
        }
    }

    pub fn read_from_vec(bytes: Vec<u8>) -> Result<Self, AudioFileOpenError> {
        let inner = Self::parse_inner(bytes.clone())?;
        Ok(AudioFile {
            raw_data: std::sync::Arc::new(bytes),
            inner,
        })
    }

    // Extracted parse_inner to fix E0599 and removed duplicate read_from_vec
    fn parse_inner(bytes: Vec<u8>) -> Result<AudioFileInner, AudioFileOpenError> {
        // Only accept WAV files that the rest of the pipeline (`audio_description`
        // and the integer-sample `read_bytes` branch) can losslessly serve.
        // touchHLE's downstream WAVE consumers (Audio Queue / OpenAL decode in
        // `audio_toolbox::audio_queue::decode_buffer`) emit 8-bit or 16-bit LPCM
        // only, so anything else (24-bit Int, 32-bit Int, 32-bit Float per
        // Microsoft WAVE / IEEE-Float specification — see Apple's Core Audio
        // Format spec which mirrors the same bit depths,
        // https://developer.apple.com/library/archive/documentation/MusicAudio/Reference/CAFSpec/CAF_chunks/CAF_chunks.html)
        // must be transcoded. Symphonia handles those WAVE variants natively and
        // produces 16-bit interleaved PCM, which is what the
        // `AudioFileInner::Symphonia` branch expects. This replaces the previous
        // `assert!(matches!(bits_per_sample, 8 | 16))` panic that took down the
        // host whenever a guest opened, e.g., a 32-bit float WAV soundtrack
        // (`blocksPremium/game loop v3.wav` from log+2).
        if let Ok(reader) = hound::WavReader::new(Cursor::new(&bytes)) {
            let spec = reader.spec();
            if matches!(spec.bits_per_sample, 8 | 16)
                && spec.sample_format == hound::SampleFormat::Int
            {
                let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
                return Ok(AudioFileInner::Wave(reader));
            }
            log!(
                "AudioFile: WAVE with bits_per_sample={} sample_format={:?} cannot be served directly; \
                 transcoding via Symphonia.",
                spec.bits_per_sample,
                spec.sample_format,
            );
            // Fall through to the Symphonia path below — Symphonia's WAV
            // demuxer (`symphonia-bundle-flac`/`symphonia-format-wav`) supports
            // every PCM WAVE bit depth Apple's iPhone-OS Core Audio accepts
            // and produces the i16 stream the rest of the pipeline expects.
        }

        if is_adts_aac(&bytes) {
            if let Ok(aac) = parse_adts_aac(bytes.clone()) {
                return Ok(AudioFileInner::Aac(aac));
            }
        }

        // CAF (Apple Core Audio Format) handling is split between three
        // paths:
        //
        // 1. AAC inside CAF is exposed as AAC packets, like ADTS `.aac`
        //    files. `try_parse_caf_aac` extracts the packets with the `caf`
        //    crate (which, unlike `symphonia-format-caf 0.6.0-alpha.1`,
        //    handles the legal CAF layout where the Audio Data chunk's
        //    `mChunkSize` is `-1` — Apple's spec explicitly allows `-1` to
        //    mean "extends to the end of the file") and wraps each packet
        //    in an ADTS header so `audio_queue::decode_buffer` can decode
        //    the resulting stream. See the CAF spec, "The Audio Data Chunk":
        //    https://developer.apple.com/library/archive/documentation/MusicAudio/Reference/CAFSpec/CAF_chunks/CAF_chunks.html
        //
        // 2. For uncompressed LPCM and IMA4 ADPCM CAF files — which is what
        //    PvZ (`com.popcap.PvZ`) and most other iOS games ship for short
        //    sound effects — we decode them ourselves in
        //    `caf_decoder::decode_caf_to_pcm` (Symphonia's CAF demuxer fails
        //    on the `mChunkSize == -1` layout described above, which is what
        //    was leaving PvZ silent).
        //
        // 3. Anything else inside a CAF container (MP3, ALAC, …) and every
        //    other supported container falls through to Symphonia, which is
        //    already wired up with the right format / codec features in
        //    `Cargo.toml`. Paths 2 and 3 return the same
        //    `SymphoniaDecodedToPcm` shape (16-bit little-endian interleaved
        //    PCM) so the rest of the audio pipeline (AudioFile /
        //    ExtAudioFile / AudioServices / AudioQueue `decode_buffer`'s
        //    LPCM branch) handles them uniformly.
        if bytes.len() >= 4 && &bytes[..4] == b"caff" {
            if let Some(aac) = try_parse_caf_aac(&bytes) {
                return Ok(AudioFileInner::Aac(aac));
            }
            if let Ok(pcm) = caf_decoder::decode_caf_to_pcm(Cursor::new(bytes.clone())) {
                return Ok(AudioFileInner::Symphonia(pcm));
            }
        }

        // Attempt to decode WAV files with µ-law (WAVE_FORMAT_MULAW=7) or
        // A-law (WAVE_FORMAT_ALAW=6) encoding, which Symphonia's RIFF demuxer
        // does not support. These are common in older iPhone OS games.
        // WAV header layout (RIFF): bytes 20-21 = wFormatTag (little-endian u16).
        // Reference: Microsoft WAVE PCM soundfile format specification.
        if bytes.len() >= 22 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
            let format_tag = u16::from_le_bytes([bytes[20], bytes[21]]);
            if format_tag == 0x0006 || format_tag == 0x0007 {
                if let Ok(pcm) = decode_wav_companded(&bytes, format_tag) {
                    return Ok(AudioFileInner::Symphonia(pcm));
                }
            }
        }

        // Sun/NeXT ".au" (a.k.a. ".snd") audio files. Symphonia has no demuxer
        // for this format, so guest apps that ship `.au` sound effects (e.g.
        // "Digga", which loads `audio/stone.au`, `audio/step.au`, …) would get
        // silent/dummy sounds. The header magic is the ASCII ".snd"
        // (0x2E534E44) big-endian word. See the format description in Sun's
        // audio interfaces and the widely mirrored `au.h`:
        // <https://en.wikipedia.org/wiki/Au_file_format>.
        if bytes.len() >= 4 && &bytes[..4] == b".snd" {
            if let Ok(pcm) = decode_au_to_pcm(&bytes) {
                return Ok(AudioFileInner::Symphonia(pcm));
            }
        }

        if let Ok(pcm) = symphonia_formats::decode_symphonia_to_pcm(Cursor::new(bytes)) {
            Ok(AudioFileInner::Symphonia(pcm))
        } else {
            Err(AudioFileOpenError::FileDecodeError)
        }
    }

    pub fn audio_description(&self) -> AudioDescription {
        match self.inner {
            AudioFileInner::Wave(ref wave_reader) => {
                let hound::WavSpec {
                    channels,
                    sample_rate,
                    bits_per_sample,
                    sample_format,
                } = wave_reader.spec();
                // `parse_inner` already routes anything other than 8-/16-bit
                // PCM through Symphonia, so reaching this branch with another
                // spec would mean a regression in `parse_inner`. Keep these as
                // `debug_assert!` so a release build can still serve a sensible
                // description if invariants are ever loosened.
                debug_assert!(matches!(bits_per_sample, 8 | 16));
                debug_assert!(sample_format == hound::SampleFormat::Int);
                let _ = sample_format;

                AudioDescription {
                    sample_rate: sample_rate.into(),
                    format: AudioFormat::LinearPcm {
                        is_float: false,
                        is_little_endian: true,
                    },
                    bytes_per_packet: u32::from(channels * bits_per_sample / 8),
                    frames_per_packet: 1,
                    channels_per_frame: channels.into(),
                    bits_per_channel: bits_per_sample as u32,
                }
            }
            AudioFileInner::Symphonia(symphonia_formats::SymphoniaDecodedToPcm {
                sample_rate,
                channels,
                ..
            }) => AudioDescription {
                sample_rate: f64::from(sample_rate),
                format: AudioFormat::LinearPcm {
                    is_float: false,
                    is_little_endian: true,
                },
                bytes_per_packet: channels * 2,
                frames_per_packet: 1,
                channels_per_frame: channels,
                bits_per_channel: 16,
            },
            AudioFileInner::Aac(ref aac) => AudioDescription {
                sample_rate: aac.sample_rate,
                format: AudioFormat::Mpeg4Aac,
                bytes_per_packet: 0,
                frames_per_packet: 1024,
                channels_per_frame: aac.channels_per_frame,
                bits_per_channel: 0,
            },
        }
    }

    /// The AAC packets of this file, if it is an AAC file.
    pub fn aac_packets(&self) -> Option<&AacPackets> {
        match self.inner {
            AudioFileInner::Aac(ref aac) => Some(aac),
            _ => None,
        }
    }

    fn bytes_per_sample(&self) -> u64 {
        let AudioDescription {
            format,
            bytes_per_packet,
            frames_per_packet,
            channels_per_frame,
            ..
        } = self.audio_description();
        if !matches!(format, AudioFormat::LinearPcm { .. }) {
            // Callers only invoke this for the Wave/PCM path where the
            // computation is meaningful. A compressed format here means
            // something else is calling us for a CAF/AAC track we shouldn't
            // be sampling; return a safe sentinel (1) and log so we don't
            // tear the host down.
            log!(
                "Warning: AudioFile::bytes_per_sample(): called on compressed format {:?}; returning 1.",
                format
            );
            return 1;
        }
        if frames_per_packet == 0 || channels_per_frame == 0 {
            log!(
                "Warning: AudioFile::bytes_per_sample(): degenerate description \
                 (frames_per_packet={}, channels_per_frame={}); returning 1.",
                frames_per_packet,
                channels_per_frame
            );
            return 1;
        }
        ((bytes_per_packet / frames_per_packet) / channels_per_frame).into()
    }

    pub fn byte_count(&self) -> u64 {
        match self.inner {
            AudioFileInner::Wave(ref wave_reader) => {
                let sample_count = wave_reader.len();
                u64::from(sample_count) * self.bytes_per_sample()
            }
            AudioFileInner::Symphonia(symphonia_formats::SymphoniaDecodedToPcm {
                ref bytes,
                ..
            }) => bytes.len() as u64,
            AudioFileInner::Aac(ref aac) => aac.byte_count(),
        }
    }

    pub fn packet_count(&self) -> u64 {
        match self.inner {
            AudioFileInner::Wave(_)
            | AudioFileInner::Symphonia(symphonia_formats::SymphoniaDecodedToPcm { .. }) => {
                self.byte_count() / u64::from(self.packet_size_fixed())
            }
            AudioFileInner::Aac(ref aac) => aac.packet_count(),
        }
    }

    pub fn packet_size_fixed(&self) -> u32 {
        let AudioDescription {
            bytes_per_packet, ..
        } = self.audio_description();
        bytes_per_packet
    }

    pub fn packet_size_upper_bound(&self) -> u32 {
        match self.inner {
            // Variable packet size: report the largest actual packet.
            AudioFileInner::Aac(ref aac) => aac.packet_size_upper_bound(),
            _ => self.packet_size_fixed(),
        }
    }

    pub fn magic_cookie(&self) -> &[u8] {
        match self.inner {
            AudioFileInner::Aac(ref aac) => &aac.magic_cookie,
            _ => &[],
        }
    }

    pub fn read_bytes(&mut self, offset: u64, buffer: &mut [u8]) -> Result<usize, ()> {
        match self.inner {
            AudioFileInner::Wave(_) => {
                let bytes_per_sample = self.bytes_per_sample();
                assert!(offset.is_multiple_of(bytes_per_sample));
                assert!(u64::try_from(buffer.len())
                    .unwrap()
                    .is_multiple_of(bytes_per_sample));
                let sample_count = u64::try_from(buffer.len()).unwrap() / bytes_per_sample;
                let sample_count: usize = sample_count.try_into().unwrap();
                let AudioFileInner::Wave(ref mut wave_reader) = self.inner else {
                    unreachable!()
                };
                let channels: u64 = wave_reader.spec().channels.into();
                wave_reader
                    .seek((offset / (bytes_per_sample * channels)).try_into().unwrap())
                    .map_err(|_| ())?;
                let mut byte_offset = 0;
                for sample in wave_reader.samples().take(sample_count) {
                    let sample: i16 = sample.map_err(|_| ())?;
                    match bytes_per_sample {
                        1 => buffer[byte_offset] = (sample + 128) as u8,
                        2 => buffer[byte_offset..][..2].copy_from_slice(&sample.to_le_bytes()),
                        other => {
                            // 8-bit and 16-bit PCM cover the vast majority of
                            // iOS WAV assets. Anything else means a broken/
                            // unusual WAVE header; surface an error to the
                            // caller rather than crashing the host.
                            log!(
                                "Warning: AudioFile::read_bytes(WAV): unsupported bytes_per_sample {}; aborting read.",
                                other
                            );
                            return Err(());
                        }
                    }
                    byte_offset += bytes_per_sample as usize;
                }
                Ok(byte_offset)
            }
            AudioFileInner::Symphonia(symphonia_formats::SymphoniaDecodedToPcm {
                ref bytes,
                ..
            }) => {
                let bytes = bytes.get(offset as usize..).ok_or(())?;
                let bytes_to_read = buffer.len().min(bytes.len());
                let bytes = &bytes[..bytes_to_read];
                buffer[..bytes_to_read].copy_from_slice(bytes);
                Ok(bytes_to_read)
            }
            AudioFileInner::Aac(ref aac) => aac.read_bytes(offset, buffer),
        }
    }
}

const CAF_FORMAT_AAC_LC: &[u8; 4] = b"aac ";

/// ADTS header sampling-frequency-index table (ISO/IEC 14496-3).
const ADTS_SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

/// Builds the 7-byte ADTS header (protection absent) for an AAC-LC packet of
/// `payload_len` bytes.
fn adts_header(sample_rate_index: u8, channel_config: u8, payload_len: usize) -> [u8; 7] {
    let frame_len = payload_len + 7;
    [
        0xFF,
        0xF1, // MPEG-4, layer 0, no CRC
        // profile = AAC LC (Audio Object Type 2, encoded as 2 - 1 = 1)
        (0b01 << 6) | (sample_rate_index << 2) | (channel_config >> 2),
        ((channel_config & 0x3) << 6) | (((frame_len >> 11) & 0x3) as u8),
        ((frame_len >> 3) & 0xFF) as u8,
        (((frame_len & 0x7) << 5) as u8) | 0x1F, // buffer fullness (VBR)
        0xFC, // buffer fullness (cont.), 1 AAC frame per ADTS frame
    ]
}

/// Extracts the AAC packets of a CAF file, wrapping each one in an ADTS
/// header so that the result can be decoded as a plain ADTS stream
/// (which is what `audio_queue::decode_buffer` expects for AAC buffers).
fn try_parse_caf_aac(bytes: &[u8]) -> Option<AacPackets> {
    let format_id = caf_read_format_id(bytes)?;
    if &format_id != CAF_FORMAT_AAC_LC {
        return None;
    }

    let mut reader = caf::CafPacketReader::new(Cursor::new(bytes), vec![]).ok()?;

    let sample_rate = reader.audio_desc.sample_rate;
    let channels_per_frame = reader.audio_desc.channels_per_frame;

    let sample_rate_index = ADTS_SAMPLE_RATES
        .iter()
        .position(|&rate| f64::from(rate) == sample_rate)? as u8;
    if !(1..=7).contains(&channels_per_frame) {
        return None;
    }
    let channel_config = channels_per_frame as u8;

    let magic_cookie: Vec<u8> = reader
        .chunks
        .iter()
        .find_map(|chunk| {
            if let caf::chunks::CafChunk::MagicCookie(ref cookie) = chunk {
                Some(cookie.clone())
            } else {
                None
            }
        })
        .unwrap_or_default();
    let packet_count = reader.get_packet_count()?;
    let mut packet_bytes = Vec::new();
    let mut packet_offsets = Vec::with_capacity(packet_count + 1);

    reader.seek_to_packet(0).ok()?;
    for _ in 0..packet_count {
        let pkt_size = reader.next_packet_size()?;
        // The ADTS frame length field is 13 bits.
        if pkt_size + 7 > 0x1FFF {
            return None;
        }
        packet_offsets.push(packet_bytes.len());
        packet_bytes.extend_from_slice(&adts_header(sample_rate_index, channel_config, pkt_size));
        let start = packet_bytes.len();
        packet_bytes.resize(start + pkt_size, 0u8);
        reader.read_packet_into(&mut packet_bytes[start..]).ok()?;
    }
    packet_offsets.push(packet_bytes.len());

    Some(AacPackets {
        sample_rate,
        channels_per_frame,
        packet_bytes,
        packet_offsets,
        magic_cookie,
    })
}

fn caf_read_format_id(bytes: &[u8]) -> Option<[u8; 4]> {
    if bytes.len() < 8 {
        return None;
    }
    if &bytes[..4] != b"caff" {
        return None;
    }
    let mut pos = 8usize;
    while pos + 12 <= bytes.len() {
        let chunk_type = &bytes[pos..pos + 4];
        let chunk_size = i64::from_be_bytes(bytes[pos + 4..pos + 12].try_into().ok()?);
        pos += 12;
        if chunk_type == b"desc" && chunk_size >= 12 {
            let data = bytes.get(pos..pos + chunk_size as usize)?;
            return data[8..12].try_into().ok();
        }
        pos = pos.checked_add(chunk_size.try_into().ok()?)?;
    }
    None
}

fn is_adts_aac(bytes: &[u8]) -> bool {
    if bytes.len() < 2 {
        return false;
    }
    (bytes[0] == 0xFF) && ((bytes[1] & 0xF0) == 0xF0)
}

fn parse_adts_aac(bytes: Vec<u8>) -> Result<AacPackets, ()> {
    const SAMPLE_RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    let mut pos = 0usize;
    let mut sample_rate = 44100f64;
    let mut channels_per_frame = 2u32;
    let mut packet_bytes: Vec<u8> = Vec::new();
    let mut packet_offsets: Vec<usize> = Vec::new();
    let mut first = true;
    while pos + 7 <= bytes.len() {
        if bytes[pos] != 0xFF || (bytes[pos + 1] & 0xF0) != 0xF0 {
            if packet_offsets.is_empty() {
                return Err(());
            }
            break;
        }

        let protection_absent = (bytes[pos + 1] & 0x01) != 0;
        let header_size: usize = if protection_absent { 7 } else { 9 };
        if first {
            let sfi = ((bytes[pos + 2] & 0x3C) >> 2) as usize;
            if sfi < SAMPLE_RATES.len() {
                sample_rate = SAMPLE_RATES[sfi].into();
            }
            let ch_cfg = ((bytes[pos + 2] & 0x01) << 2) | ((bytes[pos + 3] & 0xC0) >> 6);
            channels_per_frame = if ch_cfg == 0 { 2 } else { u32::from(ch_cfg) };
            first = false;
        }

        let frame_length = (((bytes[pos + 3] & 0x03) as usize) << 11)
            | ((bytes[pos + 4] as usize) << 3)
            | ((bytes[pos + 5] as usize) >> 5);

        if frame_length < header_size || pos + frame_length > bytes.len() {
            break;
        }

        packet_offsets.push(packet_bytes.len());
        packet_bytes.extend_from_slice(&bytes[pos..pos + frame_length]);
        pos += frame_length;
    }

    if packet_offsets.is_empty() {
        return Err(());
    }
    packet_offsets.push(packet_bytes.len());

    Ok(AacPackets {
        sample_rate,
        channels_per_frame,
        packet_bytes,
        packet_offsets,
        magic_cookie: Vec::new(),
    })
}

/// Decode a WAV file with µ-law (format_tag=7) or A-law (format_tag=6)
/// encoding to 16-bit little-endian interleaved PCM.
///
/// These companded formats are not supported by Symphonia's RIFF demuxer but
/// are common in older iPhone OS game sound assets.
///
/// WAV header parsing follows the Microsoft WAVE specification:
/// <https://docs.microsoft.com/en-us/windows/win32/xaudio2/resource-interchange-file-format--riff->
/// and Apple's iPhone OS audio format documentation:
/// <https://developer.apple.com/library/archive/documentation/MusicAudio/Reference/CAFSpec/CAF_chunks/CAF_chunks.html>
fn decode_wav_companded(
    bytes: &[u8],
    format_tag: u16,
) -> Result<symphonia_formats::SymphoniaDecodedToPcm, ()> {
    // Minimum RIFF/WAV header: RIFF(4)+size(4)+WAVE(4)+fmt (4)+fmtsize(4)+
    // wFormatTag(2)+nChannels(2)+nSamplesPerSec(4)+... = at least 44 bytes.
    if bytes.len() < 44 {
        return Err(());
    }

    // Parse the fmt chunk fields.
    // nChannels is at offset 22 (2 bytes LE)
    let channels = u16::from_le_bytes([bytes[22], bytes[23]]) as u32;
    // nSamplesPerSec at offset 24 (4 bytes LE)
    let sample_rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);

    if channels == 0 || sample_rate == 0 {
        return Err(());
    }

    // Find the "data" sub-chunk by scanning past the fmt chunk.
    // fmt chunk size is at bytes[16..20] (LE u32).
    let fmt_size = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]) as usize;
    // fmt chunk body starts at offset 20; the next chunk starts after it.
    let mut pos = 12usize; // skip RIFF header (4 + 4 + WAVE fourcc = 12)
    let mut data_offset: Option<usize> = None;
    let mut data_size: usize = 0;

    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let chunk_size =
            u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]])
                as usize;
        if chunk_id == b"data" {
            data_offset = Some(pos + 8);
            data_size = chunk_size;
            break;
        }
        // Advance past this chunk (plus padding byte for odd-sized chunks).
        pos += 8 + chunk_size + (chunk_size & 1);
        let _ = fmt_size; // suppress unused warning
    }

    let data_start = data_offset.ok_or(())?;
    let data_end = (data_start + data_size).min(bytes.len());
    let raw = &bytes[data_start..data_end];

    if raw.is_empty() {
        return Err(());
    }

    let mut out_pcm: Vec<u8> = Vec::with_capacity(raw.len() * 2);

    match format_tag {
        0x0007 => {
            // WAVE_FORMAT_MULAW — ITU-T G.711 µ-law
            // Per Apple documentation on the µ-law WAVE variant and the
            // standard ITU-T G.711 specification (11/88).
            for &byte in raw {
                let s16 = ulaw_to_linear_wav(byte);
                out_pcm.extend_from_slice(&s16.to_le_bytes());
            }
        }
        0x0006 => {
            // WAVE_FORMAT_ALAW — ITU-T G.711 A-law
            for &byte in raw {
                let s16 = alaw_to_linear_wav(byte);
                out_pcm.extend_from_slice(&s16.to_le_bytes());
            }
        }
        _ => return Err(()),
    }

    log!(
        "decode_wav_companded: decoded {} bytes of WAV format={:#06x} ({} Hz, {} ch) \
         to {} bytes of 16-bit LE PCM",
        raw.len(),
        format_tag,
        sample_rate,
        channels,
        out_pcm.len()
    );

    Ok(symphonia_formats::SymphoniaDecodedToPcm {
        bytes: out_pcm,
        sample_rate,
        channels,
    })
}

/// Decode a Sun/NeXT ".au" (".snd") audio file to 16-bit little-endian
/// interleaved PCM.
///
/// The `.au` header is six big-endian 32-bit words:
///   0: magic number, always the ASCII ".snd" (0x2E534E44)
///   1: data offset (bytes from start of file to the audio data)
///   2: data size in bytes (0xFFFFFFFF means "unknown / until EOF")
///   3: encoding
///   4: sample rate (Hz)
///   5: channel count
/// The audio samples themselves are stored big-endian.
///
/// Reference: the Sun/NeXT audio file format, as documented in Sun's
/// `multimedia/audio_filehdr.h` and summarised at
/// <https://en.wikipedia.org/wiki/Au_file_format>. The encoding constants
/// match Sun's `AUDIO_FILE_ENCODING_*` values.
fn decode_au_to_pcm(bytes: &[u8]) -> Result<symphonia_formats::SymphoniaDecodedToPcm, ()> {
    if bytes.len() < 24 || &bytes[..4] != b".snd" {
        return Err(());
    }
    let read_u32_be = |off: usize| u32::from_be_bytes(bytes[off..off + 4].try_into().unwrap());

    let mut data_offset = read_u32_be(4) as usize;
    let data_size = read_u32_be(8);
    let encoding = read_u32_be(12);
    let sample_rate = read_u32_be(16);
    let channels = read_u32_be(20);

    // The header (with optional annotation/info field) must be at least 24
    // bytes; some writers set a smaller value, in which case clamp it.
    if data_offset < 24 {
        data_offset = 24;
    }
    if data_offset > bytes.len() || channels == 0 || sample_rate == 0 {
        return Err(());
    }

    // A data size of 0xFFFFFFFF (or one that overruns the file) means "read to
    // the end of the file".
    let available = bytes.len() - data_offset;
    let data_len = if data_size == 0xFFFF_FFFF || data_size as usize > available {
        available
    } else {
        data_size as usize
    };
    let raw = &bytes[data_offset..data_offset + data_len];
    if raw.is_empty() {
        return Err(());
    }

    let mut out_pcm: Vec<u8> = Vec::with_capacity(raw.len() * 2);
    match encoding {
        // 1: 8-bit G.711 µ-law
        1 => {
            for &byte in raw {
                out_pcm.extend_from_slice(&ulaw_to_linear_wav(byte).to_le_bytes());
            }
        }
        // 2: 8-bit signed linear PCM
        2 => {
            for &byte in raw {
                let s16 = (byte as i8 as i16) << 8;
                out_pcm.extend_from_slice(&s16.to_le_bytes());
            }
        }
        // 3: 16-bit signed linear PCM, big-endian
        3 => {
            for frame in raw.chunks_exact(2) {
                let s16 = i16::from_be_bytes([frame[0], frame[1]]);
                out_pcm.extend_from_slice(&s16.to_le_bytes());
            }
        }
        // 6: 32-bit IEEE floating point, big-endian
        6 => {
            for frame in raw.chunks_exact(4) {
                let f = f32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
                let s16 = (f.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                out_pcm.extend_from_slice(&s16.to_le_bytes());
            }
        }
        // 27: 8-bit G.711 A-law
        27 => {
            for &byte in raw {
                out_pcm.extend_from_slice(&alaw_to_linear_wav(byte).to_le_bytes());
            }
        }
        other => {
            log!(
                "decode_au_to_pcm: unsupported .au encoding {}; cannot decode.",
                other
            );
            return Err(());
        }
    }

    log!(
        "decode_au_to_pcm: decoded {} bytes of .au encoding={} ({} Hz, {} ch) \
         to {} bytes of 16-bit LE PCM",
        raw.len(),
        encoding,
        sample_rate,
        channels,
        out_pcm.len()
    );

    Ok(symphonia_formats::SymphoniaDecodedToPcm {
        bytes: out_pcm,
        sample_rate,
        channels,
    })
}

/// Decode a single 8-bit µ-law (G.711) sample to 16-bit signed linear PCM.
///
/// This mirrors `caf_decoder::ulaw_to_linear` but is a standalone copy so
/// the WAV decoder does not depend on the CAF module's private API.
///
/// Reference: ITU-T Rec. G.711 (11/88); Sun Microsystems g711.c (public domain).
fn ulaw_to_linear_wav(u_val: u8) -> i16 {
    let u_val = !u_val;
    let segment = ((u_val & 0x70) >> 4) as i32;
    let quantization = (u_val & 0x0F) as i32;
    const BIAS: i32 = 0x84;
    let mut t = (quantization << 3) + BIAS;
    t <<= segment;
    if (u_val & 0x80) != 0 {
        (t - BIAS) as i16
    } else {
        (BIAS - t) as i16
    }
}

/// Decode a single 8-bit A-law (G.711) sample to 16-bit signed linear PCM.
///
/// Reference: ITU-T Rec. G.711 (11/88).
fn alaw_to_linear_wav(a_val: u8) -> i16 {
    let a_val = a_val ^ 0x55;
    let segment = ((a_val & 0x70) >> 4) as i32;
    let quantization = (a_val & 0x0F) as i32;
    let t = if segment == 0 {
        (quantization << 4) + 8
    } else {
        ((quantization << 4) + 0x108) << (segment - 1)
    };
    if (a_val & 0x80) != 0 {
        t as i16
    } else {
        -(t as i16)
    }
}
