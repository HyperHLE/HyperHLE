/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! The AVFoundation framework.

mod av_audio_player;
pub mod av_audio_session;
pub mod av_capture;
pub mod av_player;
pub mod av_speech;

use crate::dyld::{ConstantExports, HostConstant};
use crate::objc::id;
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    pub av_audio_session: av_audio_session::State,
    pub av_capture: av_capture::State,
    /// Side-table for `AVCaptureVideoPreviewLayer` instances. The base
    /// `CALayerHostObject` is allocated by `+[CALayer allocWithZone:]`, so
    /// subclass-specific state (the AVCaptureSession the layer is bound to,
    /// the videoGravity string, etc.) lives here keyed by layer `id`.
    pub av_capture_preview_extras: HashMap<id, av_capture::AVCapturePreviewLayerExtra>,
    /// Side-table mapping each `AVPlayerLayer` instance to the `AVPlayer*`
    /// (retained) it was assigned. Like the capture preview layer above, the
    /// base `CALayerHostObject` is allocated by `+[CALayer allocWithZone:]`,
    /// so this subclass-specific association can't live on the host object.
    pub av_player_layer_players: HashMap<id, id>,
}

/// Constants commonly referenced from iOS 5/6 binaries that don't yet have a
/// dedicated host implementation. Their actual string values are observable
/// only through identity comparisons / dictionary lookups, so spelling them
/// canonically (matching Apple's headers) is sufficient.
pub const STUB_CONSTANTS: ConstantExports = &[
    // AVPlayerItem notification names.
    (
        "_AVPlayerItemDidPlayToEndTimeNotification",
        HostConstant::NSString("AVPlayerItemDidPlayToEndTimeNotification"),
    ),
    (
        "_AVPlayerItemFailedToPlayToEndTimeNotification",
        HostConstant::NSString("AVPlayerItemFailedToPlayToEndTimeNotification"),
    ),
    // userInfo key for `AVPlayerItemFailedToPlayToEndTimeNotification`,
    // pointing at the `NSError` describing the failure. Apple
    // `AVPlayerItem.h` declares it as `AVF_EXPORT NSString *const`; the
    // literal value matches the symbol name.
    // <https://developer.apple.com/documentation/avfoundation/avplayeritemfailedtoplaytoendtimeerrorkey>
    (
        "_AVPlayerItemFailedToPlayToEndTimeErrorKey",
        HostConstant::NSString("AVPlayerItemFailedToPlayToEndTimeErrorKey"),
    ),
    (
        "_AVPlayerItemPlaybackStalledNotification",
        HostConstant::NSString("AVPlayerItemPlaybackStalledNotification"),
    ),
    (
        "_AVPlayerItemNewErrorLogEntryNotification",
        HostConstant::NSString("AVPlayerItemNewErrorLogEntryNotification"),
    ),
    (
        "_AVPlayerItemNewAccessLogEntryNotification",
        HostConstant::NSString("AVPlayerItemNewAccessLogEntryNotification"),
    ),
    // AVAudioRecorder settings dictionary keys (apps reach them through
    // Mach-O symbol lookup, so we expose them as NSString constants whose
    // identity matches Apple's value).
    (
        "_AVFormatIDKey",
        HostConstant::NSString("AVFormatIDKey"),
    ),
    (
        "_AVSampleRateKey",
        HostConstant::NSString("AVSampleRateKey"),
    ),
    (
        "_AVNumberOfChannelsKey",
        HostConstant::NSString("AVNumberOfChannelsKey"),
    ),
    (
        "_AVEncoderAudioQualityKey",
        HostConstant::NSString("AVEncoderAudioQualityKey"),
    ),
    (
        "_AVEncoderBitRateKey",
        HostConstant::NSString("AVEncoderBitRateKey"),
    ),
    (
        "_AVEncoderBitDepthHintKey",
        HostConstant::NSString("AVEncoderBitDepthHintKey"),
    ),
    (
        "_AVLinearPCMBitDepthKey",
        HostConstant::NSString("AVLinearPCMBitDepthKey"),
    ),
    (
        "_AVLinearPCMIsBigEndianKey",
        HostConstant::NSString("AVLinearPCMIsBigEndianKey"),
    ),
    (
        "_AVLinearPCMIsFloatKey",
        HostConstant::NSString("AVLinearPCMIsFloatKey"),
    ),
    (
        "_AVLinearPCMIsNonInterleaved",
        HostConstant::NSString("AVLinearPCMIsNonInterleaved"),
    ),
    // -----------------------------------------------------------------
    // AVAssetWriter / AVAssetExportSession video output keys & preset
    // identifiers (Apple `AVVideoSettings.h`, `AVAssetExportSession.h`).
    // String literals match Apple's; the comparison sites are
    // dictionary lookups (`outputSettings[AVVideoCodecKey]`) and
    // identity checks against the documented preset names.
    // -----------------------------------------------------------------
    ("_AVVideoCodecKey", HostConstant::NSString("AVVideoCodecKey")),
    (
        "_AVVideoCodecH264",
        HostConstant::NSString("avc1"),
    ),
    (
        "_AVVideoCodecJPEG",
        HostConstant::NSString("jpeg"),
    ),
    (
        "_AVVideoCodecAppleProRes422",
        HostConstant::NSString("apcn"),
    ),
    (
        "_AVVideoCodecAppleProRes4444",
        HostConstant::NSString("ap4h"),
    ),
    (
        "_AVVideoWidthKey",
        HostConstant::NSString("AVVideoWidthKey"),
    ),
    (
        "_AVVideoHeightKey",
        HostConstant::NSString("AVVideoHeightKey"),
    ),
    (
        "_AVVideoCompressionPropertiesKey",
        HostConstant::NSString("AVVideoCompressionPropertiesKey"),
    ),
    (
        "_AVVideoAverageBitRateKey",
        HostConstant::NSString("AVVideoAverageBitRateKey"),
    ),
    (
        "_AVVideoMaxKeyFrameIntervalKey",
        HostConstant::NSString("AVVideoMaxKeyFrameIntervalKey"),
    ),
    (
        "_AVVideoProfileLevelKey",
        HostConstant::NSString("AVVideoProfileLevelKey"),
    ),
    (
        "_AVVideoScalingModeKey",
        HostConstant::NSString("AVVideoScalingModeKey"),
    ),
    (
        "_AVVideoScalingModeFit",
        HostConstant::NSString("AVVideoScalingModeFit"),
    ),
    (
        "_AVVideoScalingModeResize",
        HostConstant::NSString("AVVideoScalingModeResize"),
    ),
    (
        "_AVVideoScalingModeResizeAspect",
        HostConstant::NSString("AVVideoScalingModeResizeAspect"),
    ),
    (
        "_AVVideoScalingModeResizeAspectFill",
        HostConstant::NSString("AVVideoScalingModeResizeAspectFill"),
    ),
    // AVFileType* — UTI strings used as the second argument to
    // -[AVAssetExportSession initWithAsset:presetName:] /
    // outputFileType. Literals are Apple's documented UTIs.
    (
        "_AVFileTypeQuickTimeMovie",
        HostConstant::NSString("com.apple.quicktime-movie"),
    ),
    (
        "_AVFileTypeMPEG4",
        HostConstant::NSString("public.mpeg-4"),
    ),
    (
        "_AVFileTypeAppleM4V",
        HostConstant::NSString("com.apple.m4v-video"),
    ),
    (
        "_AVFileTypeAppleM4A",
        HostConstant::NSString("com.apple.m4a-audio"),
    ),
    (
        "_AVFileTypeMPEGLayer3",
        HostConstant::NSString("public.mp3"),
    ),
    (
        "_AVFileTypeWAVE",
        HostConstant::NSString("com.microsoft.waveform-audio"),
    ),
    (
        "_AVFileTypeAIFF",
        HostConstant::NSString("public.aiff-audio"),
    ),
    (
        "_AVFileTypeAIFC",
        HostConstant::NSString("public.aifc-audio"),
    ),
    (
        "_AVFileTypeAMR",
        HostConstant::NSString("org.3gpp.adaptive-multi-rate-audio"),
    ),
    (
        "_AVFileType3GPP",
        HostConstant::NSString("public.3gpp"),
    ),
    (
        "_AVFileTypeCoreAudioFormat",
        HostConstant::NSString("com.apple.coreaudio-format"),
    ),
    // AVAssetExportSession preset names — also bare `NSString *
    // const`. These map to Apple's published preset identifiers used
    // by -[AVAssetExportSession exportPresetsCompatibleWithAsset:].
    (
        "_AVAssetExportPresetLowQuality",
        HostConstant::NSString("AVAssetExportPresetLowQuality"),
    ),
    (
        "_AVAssetExportPresetMediumQuality",
        HostConstant::NSString("AVAssetExportPresetMediumQuality"),
    ),
    (
        "_AVAssetExportPresetHighestQuality",
        HostConstant::NSString("AVAssetExportPresetHighestQuality"),
    ),
    (
        "_AVAssetExportPreset640x480",
        HostConstant::NSString("AVAssetExportPreset640x480"),
    ),
    (
        "_AVAssetExportPreset960x540",
        HostConstant::NSString("AVAssetExportPreset960x540"),
    ),
    (
        "_AVAssetExportPreset1280x720",
        HostConstant::NSString("AVAssetExportPreset1280x720"),
    ),
    (
        "_AVAssetExportPreset1920x1080",
        HostConstant::NSString("AVAssetExportPreset1920x1080"),
    ),
    (
        "_AVAssetExportPresetAppleM4A",
        HostConstant::NSString("AVAssetExportPresetAppleM4A"),
    ),
    (
        "_AVAssetExportPresetPassthrough",
        HostConstant::NSString("AVAssetExportPresetPassthrough"),
    ),
    // -----------------------------------------------------------------
    // AVCaptureSession preset names (additional, iOS 5+).
    // <https://developer.apple.com/documentation/avfoundation/avcapturesessionpresetiframe960x540>
    // -----------------------------------------------------------------
    (
        "_AVCaptureSessionPresetiFrame960x540",
        HostConstant::NSString("AVCaptureSessionPresetiFrame960x540"),
    ),
    (
        "_AVCaptureSessionPresetiFrame1280x720",
        HostConstant::NSString("AVCaptureSessionPresetiFrame1280x720"),
    ),
    // -----------------------------------------------------------------
    // AVAudioFormat / AVAudio settings dictionary keys.
    // <https://developer.apple.com/documentation/avfoundation/avaudiosettings>
    // -----------------------------------------------------------------
    (
        "_AVChannelLayoutKey",
        HostConstant::NSString("AVChannelLayoutKey"),
    ),
    // -----------------------------------------------------------------
    // AVURLAsset init option keys.
    // <https://developer.apple.com/documentation/avfoundation/avurlassetpreferprecisedurationandtimingkey>
    // -----------------------------------------------------------------
    (
        "_AVURLAssetPreferPreciseDurationAndTimingKey",
        HostConstant::NSString("AVURLAssetPreferPreciseDurationAndTimingKey"),
    ),
    (
        "_AVURLAssetReferenceRestrictionsKey",
        HostConstant::NSString("AVURLAssetReferenceRestrictionsKey"),
    ),
    // -----------------------------------------------------------------
    // AVVideoProfileLevel / AVVideoCodec constants.
    // <https://developer.apple.com/documentation/avfoundation/avvideoprofilelevelh264main31>
    // -----------------------------------------------------------------
    (
        "_AVVideoProfileLevelH264Main31",
        HostConstant::NSString("H264_Main_3_1"),
    ),
    (
        "_AVVideoProfileLevelH264Baseline30",
        HostConstant::NSString("H264_Baseline_3_0"),
    ),
    (
        "_AVVideoProfileLevelH264Baseline31",
        HostConstant::NSString("H264_Baseline_3_1"),
    ),
    (
        "_AVVideoProfileLevelH264High40",
        HostConstant::NSString("H264_High_4_0"),
    ),
];

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/AVFoundation.framework/AVFoundation",
    aliases: &[],
    class_exports: &[
        av_audio_player::CLASSES,
        av_audio_session::CLASSES,
        av_capture::CLASSES,
        av_player::CLASSES,
        av_speech::CLASSES,
    ],
    constant_exports: &[
        av_audio_session::CONSTANTS,
        av_capture::CONSTANTS,
        STUB_CONSTANTS,
    ],
    function_exports: &[],
};
