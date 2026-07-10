/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Decoder for the Apple IMA4 ADPCM format (FourCC: `ima4`)
//!
//! Resources on IMA ADPCM in general:
//! - MultimediaWiki's [IMA ADPCM](https://wiki.multimedia.cx/index.php?title=IMA_ADPCM) page
//! - The IMA's _Recommended Practices for Enhancing Digital Audio Compatibility
//!   in Multimedia Systems_, which includes a reference decoding algorithm in C
//!   on pages 31 and 32.
//!   - [OCR'd PDF](http://www.cs.columbia.edu/~hgs/audio/dvi/IMA_ADPCM.pdf)
//!   - [Untouched scans](http://www.cs.columbia.edu/~hgs/audio/dvi/)
//!
//! Resources on Apple IMA4:
//! - MultimediaWiki's [Apple QuickTime IMA ADPCM](https://wiki.multimedia.cx/index.php?title=Apple_QuickTime_IMA_ADPCM) page
//! - Apple's [Technical Note TN1081: Understanding the Differences Between Apple and Windows IMA-ADPCM Compressed Sound Files](https://web.archive.org/web/20080705145411/http://developer.apple.com/technotes/tn/tn1081.html) (also available [here](https://developer.apple.com/library/archive/technotes/tn/tn1081.html))
//!
//! The implementation here generally follows the naming from the IMA reference
//! algorithm.

const INDEX_TABLE: &[i8] = &[-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

const STEP_SIZE_TABLE: &[u16] = &[
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

/// Decoder state for one channel of an IMA ADPCM stream.
///
/// This implements the reference IMA decoding algorithm and is shared between
/// the Apple IMA4 packet decoder below and the Microsoft/DVI WAV IMA ADPCM
/// decoder in [`crate::audio::wav_extra`] (both use the same step-size and
/// index tables; only the container framing differs — see Apple's TN1081).
pub(crate) struct ImaAdpcmState {
    pub predicted_sample: i16,
    pub step_index: usize,
}

impl ImaAdpcmState {
    pub fn new(predicted_sample: i16, step_index: usize) -> Self {
        ImaAdpcmState {
            predicted_sample,
            step_index: step_index.min(STEP_SIZE_TABLE.len() - 1),
        }
    }

    /// Decode a single 4-bit IMA ADPCM code and return the new PCM sample.
    pub fn decode_nibble(&mut self, nibble: u8) -> i16 {
        let nibble = nibble & 0xf;
        let step_size = STEP_SIZE_TABLE[self.step_index];

        let mut difference = 0u16;
        if nibble & 4 != 0 {
            difference += step_size;
        }
        if nibble & 2 != 0 {
            difference += step_size >> 1;
        }
        if nibble & 1 != 0 {
            difference += step_size >> 2;
        }
        difference += step_size >> 3;

        self.predicted_sample = if nibble & 8 != 0 {
            self.predicted_sample.saturating_sub_unsigned(difference)
        } else {
            self.predicted_sample.saturating_add_unsigned(difference)
        };

        self.step_index = self
            .step_index
            .saturating_add_signed(INDEX_TABLE[nibble as usize].into())
            .min(STEP_SIZE_TABLE.len() - 1);

        self.predicted_sample
    }
}

/// Decode a 34-byte IMA4 ADPCM packet to 16-bit signed integer PCM.
///
/// The packet is always a single channel. For stereo, the packets alternate
/// between left and right, such that the first packet is for the left channel
/// and every other packet is for the right channel.
pub fn decode_ima4(in_packet: &[u8; 34]) -> [i16; 64] {
    let mut out_packet = [0i16; 64];

    let header = u16::from_be_bytes(in_packet[0..2].try_into().unwrap());
    let index = ((header & 0x7f) as usize).min(STEP_SIZE_TABLE.len() - 1);
    let predicted_sample = ((header >> 7) << 7) as i16;
    let mut state = ImaAdpcmState::new(predicted_sample, index);

    for (byte_idx, &byte) in in_packet[2..].iter().enumerate() {
        for nibble_idx in 0..2 {
            let nibble = (byte >> (nibble_idx * 4)) & 0xf;
            out_packet[byte_idx * 2 + nibble_idx] = state.decode_nibble(nibble);
        }
    }

    out_packet
}
