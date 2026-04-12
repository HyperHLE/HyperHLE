/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Audio file decoding and OpenAL bindings.
//!
//! The audio file decoding support is an abstraction over various libraries
//! (currently [caf], [hound], and [symphonia]), usage of which should be
//! confined to this module.
//!
//! Resources:
//! - [Apple Core Audio Format Specification 1.0](https://developer.apple.com/library/archive/documentation/MusicAudio/Reference/CAFSpec/CAF_intro/CAF_intro.html)

mod ima4;
pub mod openal;
mod symphonia_formats;

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
    AppleIma4,
    /// MPEG-4 Advanced Audio Coding (AAC-LC).
    ///
    /// Raw ADTS or CAF-packaged AAC frames. The emulator does not decode
    /// these itself; callers that need PCM must pass through an Audio
    /// Converter or use the Symphonia PCM path instead.
    Mpeg4Aac,
}

/// Fields have the same meanings as in the Core Audio Format's
/// Audio Description chunk, which is in turn similar to Core Audio Types'
/// `AudioStreamBasicDescription`.
#[derive(Debug)]
pub struct AudioDescription {
    /// Hz
    pub sample_rate: f64,
    pub format: AudioFormat,
    pub bytes_per_packet: u32,
    pub frames_per_packet: u32,
    pub channels_per_frame: u32,
    pub bits_per_channel: u32,
}

// ---------------------------------------------------------------------------
// AAC packet store
// ---------------------------------------------------------------------------

/// Raw (compressed) AAC packet data extracted from a CAF or ADTS container.
///
/// Each entry in `packets` is one complete AAC frame's worth of bytes.
/// `frames_per_packet` is always 1024 for AAC-LC.
pub struct AacPackets {
    pub sample_rate: f64,
    pub channels_per_frame: u32,
    /// Raw bytes for every packet, concatenated.
    pub packet_bytes: Vec<u8>,
    /// Byte offset of each packet inside `packet_bytes`.
    /// The sentinel at `packet_offsets[n]` is the end of the last packet (== packet_bytes.len()).
    pub packet_offsets: Vec<usize>,
    /// Magic cookie / decoder-specific config blob (AudioSpecificConfig).
    /// Empty when parsed from raw ADTS (the config is embedded in each frame).
    pub magic_cookie: Vec<u8>,
}

impl AacPackets {
    pub fn packet_count(&self) -> u64 {
        // packet_offsets has one sentinel past the end, so subtract 1.
        self.packet_offsets.len().saturating_sub(1) as u64
    }

    pub fn byte_count(&self) -> u64 {
        self.packet_bytes.len() as u64
    }

    /// Upper bound on bytes per packet; equal to the largest packet seen.
    pub fn packet_size_upper_bound(&self) -> u32 {
        self.packet_offsets
            .windows(2)
            .map(|w| (w[1] - w[0]) as u32)
            .max()
            .unwrap_or(0)
    }

    /// Read raw packet bytes starting at byte offset `offset` into `buffer`.
    /// Packets are treated as an opaque byte stream (variable-size read).
    pub fn read_bytes(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, ()> {
        let start = offset as usize;
        let src = self.packet_bytes.get(start..).ok_or(())?;
        let n = buffer.len().min(src.len());
        buffer[..n].copy_from_slice(&src[..n]);
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
// AudioFile
// ---------------------------------------------------------------------------

// ИСПРАВЛЕНИЕ: Добавлен Arc с сырыми данными, чтобы файл можно было клонировать 
// с независимым курсором (ExtAudioFileWrapAudioFileID)
pub struct AudioFile {
    raw_data: std::sync::Arc<Vec<u8>>,
    inner: AudioFileInner,
}

impl Clone for AudioFile {
    fn clone(&self) -> Self {
        // Мы просто заново парсим сырые байты, чтобы получить новый независимый курсор чтения
        let inner = Self::parse_inner(self.raw_data.as_ref().clone()).unwrap();
        AudioFile {
            raw_data: self.raw_data.clone(),
            inner,
        }
    }
}

enum AudioFileInner {
    Wave(hound::WavReader<Cursor<Vec<u8>>>),
    Caf(caf::CafPacketReader<Cursor<Vec<u8>>>),
    Symphonia(symphonia_formats::SymphoniaDecodedToPcm),
    /// Raw AAC packets (CAF-AAC or ADTS AAC).
    Aac(AacPackets),
}

impl AudioFile {
    pub fn open_for_reading<P: AsRef<GuestPath>>(
        path: P,
        fs: &Fs,
    ) -> Result<Self, AudioFileOpenError> {
        // TODO: it would be better not to load the whole file at once
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

    fn parse_inner(bytes: Vec<u8>) -> Result<AudioFileInner, AudioFileOpenError> {
        // Both WavReader::new() and CafPacketReader::new() consume the reader,
        // so we use temporary readers for format detection and then recreate.
        if hound::WavReader::new(Cursor::new(&bytes)).is_ok() {
            let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
            Ok(AudioFileInner::Wave(reader))
        } else if caf::CafPacketReader::new(Cursor::new(&bytes), vec![]).is_ok() {
            // Distinguish CAF-AAC from CAF-PCM/IMA4 early so we can expose
            // the correct compressed format upstream.
            if let Some(aac) = try_parse_caf_aac(&bytes) {
                Ok(AudioFileInner::Aac(aac))
            } else {
                let reader = caf::CafPacketReader::new(Cursor::new(bytes), vec![]).unwrap();
                Ok(AudioFileInner::Caf(reader))
            }
        } else if is_adts_aac(&bytes) {
            // Raw ADTS AAC stream (e.g. a bare .aac file).
            match parse_adts_aac(bytes) {
                Ok(aac) => Ok(AudioFileInner::Aac(aac)),
                Err(_) => Err(AudioFileOpenError::FileDecodeError),
            }
        // TODO: Real MP3/MP4/Non-linear PCM container handling. Currently we
        // are immediately decoding the entire file to PCM and acting as if
        // it's a PCM file, simply because this is easier. Full MP3 support
        // would require a lot of changes in Audio Toolbox.
        } else if let Ok(pcm) = symphonia_formats::decode_symphonia_to_pcm(Cursor::new(bytes)) {
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
                // Hound supports unsigned 8-bit, signed 16-bit, signed 24-bit
                // and floating-point 32-bit linear PCM. We should expose all of
                // these eventually, but we should only expose formats we've tested.
                assert!(matches!(bits_per_sample, 8 | 16));
                assert!(sample_format == hound::SampleFormat::Int);

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
            AudioFileInner::Caf(ref caf_reader) => {
                let caf::chunks::AudioDescription {
                    sample_rate,
                    ref format_id,
                    format_flags,
                    bytes_per_packet,
                    frames_per_packet,
                    channels_per_frame,
                    bits_per_channel,
                } = caf_reader.audio_desc;
                AudioDescription {
                    sample_rate,
                    format: match format_id {
                        caf::FormatType::LinearPcm => {
                            assert!((format_flags & !3) == 0);
                            let is_float = (format_flags & 1) == 1;
                            let is_little_endian = (format_flags & 2) == 2;
                            AudioFormat::LinearPcm {
                                is_float,
                                is_little_endian,
                            }
                        }
                        caf::FormatType::AppleIma4 => {
                            assert!(format_flags == 0);
                            AudioFormat::AppleIma4
                        }
                        // We should expose all of the formats eventually, but
                        // the others haven't been tested yet.
                        _ => panic!("{format_id:?} not supported yet"),
                    },
                    bytes_per_packet,
                    frames_per_packet,
                    channels_per_frame,
                    bits_per_channel,
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
                // bytes_per_packet == 0 signals variable-size packets to
                // Core Audio callers (same convention as AAC in a CAF file).
                bytes_per_packet: 0,
                // AAC-LC always produces 1024 PCM frames per compressed packet.
                frames_per_packet: 1024,
                channels_per_frame: aac.channels_per_frame,
                // AAC is a compressed format; bits_per_channel is 0.
                bits_per_channel: 0,
            },
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
            panic!("{format:?} is a compressed format!");
        }
        ((bytes_per_packet / frames_per_packet) / channels_per_frame).into()
    }

    pub fn byte_count(&self) -> u64 {
        match self.inner {
            AudioFileInner::Wave(ref wave_reader) => {
                let sample_count = wave_reader.len(); // position-independent
                u64::from(sample_count) * self.bytes_per_sample()
            }
            AudioFileInner::Caf(_) => {
                // variable size not implemented
                u64::from(self.packet_size_fixed()) * self.packet_count()
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
                // never variable-size
                self.byte_count() / u64::from(self.packet_size_fixed())
            }
            AudioFileInner::Caf(ref caf_reader) => {
                caf_reader.get_packet_count().unwrap().try_into().unwrap()
            }
            AudioFileInner::Aac(ref aac) => aac.packet_count(),
        }
    }

    /// Returns the packet size if this audio format has a constant packet size,
    /// panics if not.
    pub fn packet_size_fixed(&self) -> u32 {
        let AudioDescription {
            bytes_per_packet, ..
        } = self.audio_description();
        // AAC has variable-size packets (bytes_per_packet == 0).
        // Callers that need fixed-size reads must not call this for AAC.
        // assert!(bytes_per_packet != 0, "packet_size_fixed() called on variable-size format");
        bytes_per_packet
    }

    pub fn packet_size_upper_bound(&self) -> u32 {
        self.packet_size_fixed() // variable size not implemented
    }

    /// Returns the magic cookie (AudioSpecificConfig) for AAC files, or an
    /// empty slice for all other formats.
    pub fn magic_cookie(&self) -> &[u8] {
        match self.inner {
            AudioFileInner::Aac(ref aac) => &aac.magic_cookie,
            _ => &[],
        }
    }

    /// Read `buffer.len()` bytes of audio data from byte offset `offset`.
    /// Returns the number of bytes read.
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
                // WavReader expects number of samples which are
                // independent of the number of channels here
                wave_reader
                    .seek((offset / (bytes_per_sample * channels)).try_into().unwrap())
                    .map_err(|_| ())?;
                let mut byte_offset = 0;
                for sample in wave_reader.samples().take(sample_count) {
                    let sample: i16 = sample.map_err(|_| ())?;
                    match bytes_per_sample {
                        // From the OpenAL docs: 8-bit PCM data is expressed as
                        // an unsigned value over the range 0 to 255, 128 being
                        // an audio output level of zero. Loaded wav samples
                        // must be converted to that from signed with 0 as
                        // output level 0.
                        1 => buffer[byte_offset] = (sample + 128) as u8,
                        2 => buffer[byte_offset..][..2].copy_from_slice(&sample.to_le_bytes()),
                        _ => todo!(),
                    }
                    byte_offset += bytes_per_sample as usize;
                }
                Ok(byte_offset)
            }
            AudioFileInner::Caf(_) => {
                // variable size not implemented
                let packet_size = self.packet_size_fixed();
                assert!(offset.is_multiple_of(packet_size.into()));
                assert!(u64::try_from(buffer.len())
                    .unwrap()
                    .is_multiple_of(packet_size.into()));
                let packet_count = u64::try_from(buffer.len()).unwrap() / u64::from(packet_size);

                let AudioFileInner::Caf(ref mut caf_reader) = self.inner else {
                    unreachable!()
                };
                caf_reader
                    .seek_to_packet(usize::try_from(offset / u64::from(packet_size)).unwrap())
                    .map_err(|_| ())?;
                let packet_size = usize::try_from(packet_size).unwrap();

                let mut i = 0;
                let mut byte_offset = 0;
                while i < packet_count && caf_reader.next_packet_size().is_some() {
                    caf_reader
                        .read_packet_into(&mut buffer[byte_offset..][..packet_size])
                        .map_err(|_| ())?;
                    byte_offset += packet_size;
                    i += 1;
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

// ---------------------------------------------------------------------------
// AAC parsing helpers
// ---------------------------------------------------------------------------

/// FourCC for AAC-LC in a CAF `audio description` chunk.
/// The CAF spec uses 'aac ' (with a trailing space) for AAC-LC.
const CAF_FORMAT_AAC_LC: &[u8; 4] = b"aac ";

/// Try to extract raw AAC packets from a CAF file that contains AAC-LC audio.
/// Returns `None` when the CAF file uses a non-AAC format.
fn try_parse_caf_aac(bytes: &[u8]) -> Option<AacPackets> {
    // We need a second parse pass for the audio description because
    // CafPacketReader consumes the reader. Use a minimal manual parse.
    let format_id = caf_read_format_id(bytes)?;
    if &format_id != CAF_FORMAT_AAC_LC {
        return None;
    }

    // Full parse to extract packets, sample rate, channels, and magic cookie.
    let mut reader = caf::CafPacketReader::new(Cursor::new(bytes), vec![]).ok()?;

    let sample_rate = reader.audio_desc.sample_rate;
    let channels_per_frame = reader.audio_desc.channels_per_frame;

    // ИСПРАВЛЕНИЕ: Исправлен доступ к MagicCookie из caf-crate
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

    // ИСПРАВЛЕНИЕ: Option<T> не имеет метода .ok() (в отличие от Result)
    let packet_count = reader.get_packet_count()?;
    
    let mut packet_bytes = Vec::new();
    let mut packet_offsets = Vec::with_capacity(packet_count as usize + 1);

    reader.seek_to_packet(0).ok()?;

    for _ in 0..packet_count {
        let pkt_size = reader.next_packet_size()?;
        packet_offsets.push(packet_bytes.len());
        let start = packet_bytes.len();
        packet_bytes.resize(start + pkt_size as usize, 0u8);

        reader
            .read_packet_into(&mut packet_bytes[start..])
            .ok()?;
    }
    packet_offsets.push(packet_bytes.len()); // sentinel

    log_dbg!(
        "try_parse_caf_aac: {} packets, {} bytes, {:.0} Hz, {} ch",
        packet_offsets.len().saturating_sub(1),
        packet_bytes.len(),
        sample_rate,
        channels_per_frame,
    );

    Some(AacPackets {
        sample_rate,
        channels_per_frame,
        packet_bytes,
        packet_offsets,
        magic_cookie,
    })
}

/// Minimal CAF header walk to read the `format_id` field from the `desc` chunk.
///
/// CAF file layout:
///   "caff" magic (4 bytes) + version (2) + flags (2)
///   Chunks: type (4) + size (i64) + data
///   `desc` chunk data starts with: sample_rate (f64) + format_id (4 bytes) + …
fn caf_read_format_id(bytes: &[u8]) -> Option<[u8; 4]> {
    if bytes.len() < 8 {
        return None;
    }
    if &bytes[..4] != b"caff" {
        return None;
    }
    let mut pos = 8usize; // skip magic + version + flags
    while pos + 12 <= bytes.len() {
        let chunk_type = &bytes[pos..pos + 4];
        let chunk_size = i64::from_be_bytes(bytes[pos + 4..pos + 12].try_into().ok()?);
        pos += 12;
        if chunk_type == b"desc" && chunk_size >= 12 {
            // format_id is at offset 8 within the desc chunk data (after f64 sample_rate).
            let data = bytes.get(pos..pos + chunk_size as usize)?;
            return Some(data[8..12].try_into().ok()?);
        }
        pos = pos.checked_add(chunk_size.try_into().ok()?)? ;
    }
    None
}

/// Detect a raw ADTS AAC stream by its sync word (0xFFF or 0xFFE).
fn is_adts_aac(bytes: &[u8]) -> bool {
    if bytes.len() < 2 {
        return false;
    }
    // ADTS sync word: 12 set bits (0xFFF) for MPEG-2/4 AAC.
    // The 13th bit being 0 distinguishes MPEG-4 (0) from MPEG-2 (1).
    (bytes[0] == 0xFF) && ((bytes[1] & 0xF0) == 0xF0)
}

/// Parse a raw ADTS bitstream into an `AacPackets`.
///
/// Each ADTS frame has a 7- or 9-byte header followed by the AAC payload.
/// Header layout (bits):
///   [0..12]  sync word (0xFFF)
///   [12]     ID (0 = MPEG-4, 1 = MPEG-2)
///   [13..14] layer (always 00)
///   [14]     protection absent (1 = no CRC, 0 = CRC present → 9-byte header)
///   [15..16] profile (0 = Main, 1 = LC, 2 = SSR, 3 = reserved) stored as (profile - 1)
///   [17..20] sampling frequency index
///   [20]     private bit
///   [21..23] channel configuration
///   [23]     originality / copy
///   [24]     home
///   [25]     copyright id bit
///   [26]     copyright id start
///   [27..40] frame length (includes header)
///   [40..53] buffer fullness (0x7FF = VBR)
///   [53..54] number of AAC frames in ADTS frame minus 1
fn parse_adts_aac(bytes: Vec<u8>) -> Result<AacPackets, ()> {
    // ADTS sampling frequency table (ISO 14496-3 §1.6.5.3.3)
    const SAMPLE_RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    let mut pos = 0usize;
    let mut sample_rate = 44100f64; // fallback
    let mut channels_per_frame = 2u32; // fallback
    let mut packet_bytes: Vec<u8> = Vec::new();
    let mut packet_offsets: Vec<usize> = Vec::new();
    let mut first = true;

    while pos + 7 <= bytes.len() {
        // Validate sync word.
        if bytes[pos] != 0xFF || (bytes[pos + 1] & 0xF0) != 0xF0 {
            if packet_offsets.is_empty() {
                return Err(()); // not ADTS
            }
            break; // trailing non-ADTS data; stop here
        }

        let protection_absent = (bytes[pos + 1] & 0x01) != 0;
        let header_size: usize = if protection_absent { 7 } else { 9 };
        if first {
            // Extract metadata from the first frame header.
            let sfi = ((bytes[pos + 2] & 0x3C) >> 2) as usize;
            if sfi < SAMPLE_RATES.len() {
                sample_rate = SAMPLE_RATES[sfi].into();
            }
            // Channel configuration:
            //   bits [21..23] span bytes[pos+2] (low 1 bit) and bytes[pos+3] (high 2 bits).
            let ch_cfg = ((bytes[pos + 2] & 0x01) << 2) | ((bytes[pos + 3] & 0xC0) >> 6);
            channels_per_frame = if ch_cfg == 0 { 2 } else { u32::from(ch_cfg) };
            first = false;
        }

        // Frame length is 13 bits: bytes[pos+3] bits [6..0] (7 bits) and
        // bytes[pos+4] (8 bits) and bytes[pos+5] bits [7] (1 bit) → 16 bits
        // stored at [27..40] of the header.
        let frame_length = (((bytes[pos + 3] & 0x03) as usize) << 11)
            | ((bytes[pos + 4] as usize) << 3)
            | ((bytes[pos + 5] as usize) >> 5);

        if frame_length < header_size || pos + frame_length > bytes.len() {
            break; // truncated frame
        }

        // Store the raw payload (header included — matches ADTS convention).
        packet_offsets.push(packet_bytes.len());
        packet_bytes.extend_from_slice(&bytes[pos..pos + frame_length]);

        pos += frame_length;
    }

    if packet_offsets.is_empty() {
        return Err(());
    }
    packet_offsets.push(packet_bytes.len()); // sentinel

    log_dbg!(
        "parse_adts_aac: {} packets, {} bytes, {:.0} Hz, {} ch",
        packet_offsets.len().saturating_sub(1),
        packet_bytes.len(),
        sample_rate,
        channels_per_frame,
    );

    Ok(AacPackets {
        sample_rate,
        channels_per_frame,
        packet_bytes,
        packet_offsets,
        // ADTS frames carry config inline; no separate magic cookie.
        magic_cookie: Vec::new(),
    })
}
