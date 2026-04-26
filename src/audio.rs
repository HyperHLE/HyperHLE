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
    Mpeg4Aac,
    AppleIma4, // Added to fix E0599 in ext_audio_file.rs
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
    Caf(caf::CafPacketReader<Cursor<Vec<u8>>>),
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
        if hound::WavReader::new(Cursor::new(&bytes)).is_ok() {
            let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
            return Ok(AudioFileInner::Wave(reader));
        }

        if is_adts_aac(&bytes) {
            if let Ok(aac) = parse_adts_aac(bytes.clone()) {
                return Ok(AudioFileInner::Aac(aac));
            }
        }

        // CAF (Apple Core Audio Format) handling is split between two paths:
        //
        // 1. For uncompressed LPCM and IMA4 ADPCM CAF files — which is what
        //    PvZ (`com.popcap.PvZ`) and most other iOS games ship for short
        //    sound effects — we decode them ourselves in
        //    `caf_decoder::decode_caf_to_pcm`. The CAF demuxer in
        //    `symphonia-format-caf 0.6.0-alpha.1` does not handle the legal
        //    CAF layout where the Audio Data chunk's `mChunkSize` is `-1`
        //    (Apple's spec explicitly allows `-1` to mean "extends to the end
        //    of the file"); on those files Symphonia walks past the audio
        //    data trying to read a chunk header and fails with
        //    `IoError(UnexpectedEof)`, which is what was leaving PvZ silent.
        //    See the CAF spec, "The Audio Data Chunk":
        //    https://developer.apple.com/library/archive/documentation/MusicAudio/Reference/CAFSpec/CAF_chunks/CAF_chunks.html
        //
        // 2. Anything else inside a CAF container (AAC, MP3, ALAC, …) and
        //    every other supported container falls through to Symphonia,
        //    which is already wired up with the right format / codec
        //    features in `Cargo.toml`. Both paths return the same
        //    `SymphoniaDecodedToPcm` shape (16-bit little-endian interleaved
        //    PCM) so the rest of the audio pipeline (AudioFile /
        //    ExtAudioFile / AudioServices / AudioQueue `decode_buffer`'s
        //    LPCM branch) handles them uniformly.
        if bytes.len() >= 4 && &bytes[..4] == b"caff" {
            if let Ok(pcm) = caf_decoder::decode_caf_to_pcm(Cursor::new(bytes.clone())) {
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
            AudioFileInner::Caf(ref reader) => AudioDescription {
                sample_rate: reader.audio_desc.sample_rate,
                format: AudioFormat::LinearPcm {
                    is_float: false, 
                    is_little_endian: true
                }, // Note: update format appropriately if you process CAF AAC
                bytes_per_packet: reader.audio_desc.bytes_per_packet,
                frames_per_packet: reader.audio_desc.frames_per_packet,
                channels_per_frame: reader.audio_desc.channels_per_frame,
                bits_per_channel: reader.audio_desc.bits_per_channel,
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
                let sample_count = wave_reader.len();
                u64::from(sample_count) * self.bytes_per_sample()
            }
            AudioFileInner::Symphonia(symphonia_formats::SymphoniaDecodedToPcm {
                ref bytes,
                ..
            }) => bytes.len() as u64,
            AudioFileInner::Aac(ref aac) => aac.byte_count(),
            AudioFileInner::Caf(ref reader) => reader.audio_desc.bytes_per_packet as u64 * reader.get_packet_count().unwrap_or(0) as u64,
        }
    }

    pub fn packet_count(&self) -> u64 {
        match self.inner {
            AudioFileInner::Wave(_)
            | AudioFileInner::Symphonia(symphonia_formats::SymphoniaDecodedToPcm { .. }) => {
                self.byte_count() / u64::from(self.packet_size_fixed())
            }
            AudioFileInner::Aac(ref aac) => aac.packet_count(),
            AudioFileInner::Caf(ref reader) => reader.get_packet_count().unwrap_or(0) as u64,
        }
    }

    pub fn packet_size_fixed(&self) -> u32 {
        let AudioDescription { bytes_per_packet, .. } = self.audio_description();
        bytes_per_packet
    }

    pub fn packet_size_upper_bound(&self) -> u32 {
        self.packet_size_fixed() 
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
                        _ => todo!(),
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
            AudioFileInner::Caf(ref mut reader) => {
                let bytes_per_packet = reader.audio_desc.bytes_per_packet as u64;
                if bytes_per_packet == 0 {
                    return Err(());
                }

                let start_packet = (offset / bytes_per_packet) as usize;
                let offset_in_first_packet = (offset % bytes_per_packet) as usize;

                if reader.seek_to_packet(start_packet).is_err() {
                    return Err(());
                }

                let mut bytes_read = 0;
                while bytes_read < buffer.len() {
                    let pkt_size = match reader.next_packet_size() {
                        Some(size) => size,
                        None => break, // Достигнут конец файла
                    };

                    let mut packet_data = vec![0u8; pkt_size];
                    if reader.read_packet_into(&mut packet_data).is_err() {
                        break;
                    }

                    let start_idx = if bytes_read == 0 { offset_in_first_packet } else { 0 };
                    
                    if start_idx >= packet_data.len() {
                        continue;
                    }

                    let bytes_to_copy = (packet_data.len() - start_idx).min(buffer.len() - bytes_read);

                    buffer[bytes_read..bytes_read + bytes_to_copy]
                        .copy_from_slice(&packet_data[start_idx..start_idx + bytes_to_copy]);

                    bytes_read += bytes_to_copy;
                }

                Ok(bytes_read)
            }
        }
    }
}

const CAF_FORMAT_AAC_LC: &[u8; 4] = b"aac ";

fn try_parse_caf_aac(bytes: &[u8]) -> Option<AacPackets> {
    let format_id = caf_read_format_id(bytes)?;
    if &format_id != CAF_FORMAT_AAC_LC {
        return None;
    }

    let mut reader = caf::CafPacketReader::new(Cursor::new(bytes), vec![]).ok()?;

    let sample_rate = reader.audio_desc.sample_rate;
    let channels_per_frame = reader.audio_desc.channels_per_frame;
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
            return Some(data[8..12].try_into().ok()?);
        }
        pos = pos.checked_add(chunk_size.try_into().ok()?)? ;
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

