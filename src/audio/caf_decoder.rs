/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Standalone CAF (Apple Core Audio Format) → 16-bit little-endian PCM decoder.
//!
//! This is used as a fallback when [`super::symphonia_formats`] cannot probe a
//! CAF file. The CAF demuxer in `symphonia-format-caf 0.6.0-alpha.1` does not
//! correctly handle CAF files where the Audio Data chunk's `mChunkSize` is set
//! to `-1` (which the CAF specification explicitly allows for the last chunk
//! to mean "extends to the end of the file"); on such files Symphonia walks
//! off the end of the audio data trying to read a chunk header and bails out
//! with `IoError(UnexpectedEof)`. Plants vs. Zombies (`com.popcap.PvZ`) ships
//! `sounds/*.caf` files in this layout, which is what was preventing its
//! sound effects from playing.
//!
//! References:
//! - Apple, *Apple Core Audio Format Specification 1.0*, "The Audio Data Chunk"
//!   <https://developer.apple.com/library/archive/documentation/MusicAudio/Reference/CAFSpec/CAF_chunks/CAF_chunks.html>
//! - Apple, *Apple Core Audio Format Specification 1.0*, "The Audio Description Chunk"
//!   <https://developer.apple.com/library/archive/documentation/MusicAudio/Reference/CAFSpec/CAF_chunks/CAF_chunks.html#//apple_ref/doc/uid/TP40001862-CH210-SW2>
//! - Apple, *AudioServicesCreateSystemSoundID*
//!   <https://developer.apple.com/documentation/audiotoolbox/audioservicescreatesystemsoundid(_:_:)>

use super::ima4::decode_ima4;
use super::symphonia_formats::SymphoniaDecodedToPcm;
use std::io::Cursor;
use std::panic::AssertUnwindSafe;

/// Decode the contents of a `.caf` file into 16-bit little-endian interleaved
/// PCM, returning the same in-memory shape that [`SymphoniaDecodedToPcm`]
/// uses so the rest of the audio pipeline can consume it uniformly.
///
/// Currently supported audio data formats inside the CAF container:
/// - `lpcm` — Linear PCM (8/16/24/32-bit signed integer, big- or little-endian).
///   Float PCM is rejected.
/// - `ima4` — Apple IMA 4:1 ADPCM (mono or stereo).
///
/// Anything else (MPEG-4 AAC, MP3, ALAC, …) is returned as `Err(())` so the
/// caller can fall through to a different decoder.
pub fn decode_caf_to_pcm(file: Cursor<Vec<u8>>) -> Result<SymphoniaDecodedToPcm, ()> {
    // The `caf` crate `panic!`s in a few corner cases that show up in real
    // iOS games (notably `to_next_chunk` / `read_chunk_body` on chunks whose
    // `mChunkSize == -1`, which Apple's CAF spec says is legal for the last
    // chunk to mean "extends to the end of the file"). Wrap the whole demux
    // in `catch_unwind` so a panic there is converted into a graceful
    // fall-through to the next decoder instead of taking the whole emulator
    // down. We log so we can see *why* it failed in the next bug report.
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| decode_caf_to_pcm_inner(file)));
    match result {
        Ok(Ok(pcm)) => Ok(pcm),
        Ok(Err(why)) => {
            log!("caf_decoder: refusing this CAF file: {}", why);
            Err(())
        }
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<&'static str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "(non-string panic payload)".to_string()
            };
            log!(
                "caf_decoder: panic from `caf` crate while demuxing CAF: {}",
                msg
            );
            Err(())
        }
    }
}

fn decode_caf_to_pcm_inner(file: Cursor<Vec<u8>>) -> Result<SymphoniaDecodedToPcm, &'static str> {
    use caf::FormatType;

    let mut reader =
        caf::CafPacketReader::new(file, vec![]).map_err(|_| "CafPacketReader::new failed")?;
    let desc = reader.audio_desc.clone();
    log!(
        "caf_decoder: format_id={:?}, sample_rate={}, channels={}, bytes_per_packet={}, frames_per_packet={}, bits_per_channel={}, format_flags=0x{:x}",
        desc.format_id,
        desc.sample_rate,
        desc.channels_per_frame,
        desc.bytes_per_packet,
        desc.frames_per_packet,
        desc.bits_per_channel,
        desc.format_flags,
    );

    let sample_rate: u32 = desc.sample_rate.round() as u32;
    let channels: u32 = desc.channels_per_frame;
    if sample_rate == 0 {
        return Err("sample_rate == 0");
    }
    if channels == 0 {
        return Err("channels == 0");
    }

    let mut out_pcm: Vec<u8> = Vec::new();

    match desc.format_id {
        FormatType::AppleIma4 => {
            // CAF IMA4: each packet covers `frames_per_packet` (= 64) frames,
            // and one packet's worth of bytes is `34 * channels_per_frame`
            // (per Apple's spec). For stereo, the packet data is the left
            // channel's 34-byte sub-packet immediately followed by the right
            // channel's 34-byte sub-packet.
            //
            // We decode each 34-byte sub-packet through `decode_ima4` exactly
            // like `audio_queue::decode_buffer` does, but eagerly for the
            // whole file.
            let sub_packet_bytes: usize = 34;
            let expected_packet_bytes: usize = sub_packet_bytes * channels as usize;
            if desc.bytes_per_packet as usize != expected_packet_bytes && desc.bytes_per_packet != 0
            {
                // Not actually a 34-bytes-per-channel packet layout — refuse
                // rather than producing garbage.
                return Err("IMA4 bytes_per_packet != 34 * channels");
            }

            // Greedily collect every CAF packet, then chunk into 34-byte
            // sub-packets and decode in (channel, channel, …) order.
            //
            // For PvZ-style CAF files where the Audio Data chunk has
            // `mChunkSize == -1`, `caf::CafPacketReader::next_packet`
            // ultimately calls `read_exact` on the underlying cursor and will
            // return an `UnexpectedEof` `Err` once we walk off the end of the
            // file. That's the natural termination signal here.
            let mut all_bytes: Vec<u8> = Vec::new();
            loop {
                match reader.next_packet() {
                    Ok(Some(pkt)) => all_bytes.extend_from_slice(&pkt),
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            log!(
                "caf_decoder: IMA4 collected {} bytes of packet data from CAF",
                all_bytes.len()
            );

            // Trim any tail that doesn't form a complete 34-byte sub-packet.
            let aligned_len = (all_bytes.len() / sub_packet_bytes) * sub_packet_bytes;
            all_bytes.truncate(aligned_len);
            if all_bytes.is_empty() {
                return Err("IMA4 file had no decodable packet data");
            }

            let mut sub_packets = all_bytes.chunks_exact(sub_packet_bytes);
            match channels {
                1 => {
                    for sub in sub_packets.by_ref() {
                        let pcm: [i16; 64] = decode_ima4(sub.try_into().unwrap());
                        for s in &pcm {
                            out_pcm.extend_from_slice(&s.to_le_bytes());
                        }
                    }
                }
                2 => {
                    while let Some(left) = sub_packets.next() {
                        let Some(right) = sub_packets.next() else {
                            break;
                        };
                        let l_pcm: [i16; 64] = decode_ima4(left.try_into().unwrap());
                        let r_pcm: [i16; 64] = decode_ima4(right.try_into().unwrap());
                        for (l, r) in l_pcm.iter().zip(r_pcm.iter()) {
                            out_pcm.extend_from_slice(&l.to_le_bytes());
                            out_pcm.extend_from_slice(&r.to_le_bytes());
                        }
                    }
                }
                _ => return Err("IMA4 with >2 channels is not supported"),
            }
        }
        FormatType::LinearPcm => {
            // CAF audio-description format flags (Apple CAF spec):
            //   bit 0 — kCAFLinearPCMFormatFlagIsFloat
            //   bit 1 — kCAFLinearPCMFormatFlagIsLittleEndian
            // Float PCM is rejected here because the rest of the pipeline only
            // accepts 16-bit signed integer little-endian PCM.
            let is_float = (desc.format_flags & 0b01) != 0;
            let is_little_endian = (desc.format_flags & 0b10) != 0;
            if is_float {
                return Err("LPCM float is not supported by this decoder");
            }

            let bits = desc.bits_per_channel;
            if !matches!(bits, 8 | 16 | 24 | 32) {
                return Err("LPCM bits_per_channel must be 8, 16, 24, or 32");
            }

            let bytes_per_sample = (bits / 8) as usize;
            loop {
                let pkt = match reader.next_packet() {
                    Ok(Some(pkt)) => pkt,
                    Ok(None) => break,
                    Err(_) => break,
                };
                for sample in pkt.chunks_exact(bytes_per_sample) {
                    let mut buf = [0u8; 8];
                    buf[..sample.len()].copy_from_slice(sample);
                    let s16 = match (bits, is_little_endian) {
                        (8, _) => {
                            // CAF 8-bit LPCM is signed (per spec); convert to
                            // signed 16-bit by sign-extending.
                            let v = buf[0] as i8;
                            (v as i16) << 8
                        }
                        (16, true) => i16::from_le_bytes([buf[0], buf[1]]),
                        (16, false) => i16::from_be_bytes([buf[0], buf[1]]),
                        (24, true) => {
                            let v = (buf[0] as i32)
                                | ((buf[1] as i32) << 8)
                                | (((buf[2] as i8) as i32) << 16);
                            (v >> 8) as i16
                        }
                        (24, false) => {
                            let v = (buf[2] as i32)
                                | ((buf[1] as i32) << 8)
                                | (((buf[0] as i8) as i32) << 16);
                            (v >> 8) as i16
                        }
                        (32, true) => {
                            let v = i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                            (v >> 16) as i16
                        }
                        (32, false) => {
                            let v = i32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
                            (v >> 16) as i16
                        }
                        _ => unreachable!(),
                    };
                    out_pcm.extend_from_slice(&s16.to_le_bytes());
                }
            }
        }
        // Compressed formats other than IMA4 (AAC, MP3, ALAC, …) — leave them
        // for Symphonia to handle.
        _ => return Err("format_id is not LPCM or IMA4 — leaving for Symphonia"),
    }

    if out_pcm.is_empty() {
        return Err("decoded output is empty");
    }
    log!(
        "caf_decoder: produced {} bytes of 16-bit LE PCM ({} Hz, {} ch)",
        out_pcm.len(),
        sample_rate,
        channels
    );

    Ok(SymphoniaDecodedToPcm {
        bytes: out_pcm,
        sample_rate,
        channels,
    })
}
