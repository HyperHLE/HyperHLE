/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AVAssetExportSession` — stub implementation.
//!
//! Real export sessions transcode media assets to different formats.
//! touchHLE has no transcoding pipeline, so all exports immediately
//! report completion with `AVAssetExportSessionStatusCompleted` so that
//! apps that gate on export completion can proceed normally.

use crate::frameworks::foundation::ns_string;
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, retain,
    ClassExports, HostObject, NSZonePtr,
};

// =========================================================================
// MARK: - AVAssetExportSessionStatus constants
// =========================================================================

pub type AVAssetExportSessionStatus = i32;
pub const AVAssetExportSessionStatusUnknown:    AVAssetExportSessionStatus = 0;
pub const AVAssetExportSessionStatusWaiting:    AVAssetExportSessionStatus = 1;
pub const AVAssetExportSessionStatusExporting:  AVAssetExportSessionStatus = 2;
pub const AVAssetExportSessionStatusCompleted:  AVAssetExportSessionStatus = 3;
pub const AVAssetExportSessionStatusFailed:     AVAssetExportSessionStatus = 4;
pub const AVAssetExportSessionStatusCancelled:  AVAssetExportSessionStatus = 5;

// =========================================================================
// MARK: - Host object
// =========================================================================

struct AVAssetExportSessionHostObject {
    /// The source asset. AVAsset* — retained.
    asset: id,
    /// The preset name string. NSString* — retained.
    preset_name: id,
    /// The output file type. NSString* — retained.
    output_file_type: id,
    /// The output URL. NSURL* — retained.
    output_url: id,
    /// Current status.
    status: AVAssetExportSessionStatus,
    /// Error — NSError* if status is Failed, otherwise nil.
    error: id,
    /// Whether existing files at the output URL should be overwritten.
    should_optimize_for_network_use: bool,
    /// Time range — stored as two f64 values (start, duration) in seconds.
    time_range_start: f64,
    time_range_duration: f64,
    /// Metadata — NSArray* — retained.
    metadata: id,
    /// Video composition — AVVideoComposition* — retained.
    video_composition: id,
    /// Audio mix — AVAudioMix* — retained.
    audio_mix: id,
}
impl HostObject for AVAssetExportSessionHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation AVAssetExportSession: NSObject

// =========================================================================
// MARK: - Allocation
// =========================================================================

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(AVAssetExportSessionHostObject {
        asset: nil,
        preset_name: nil,
        output_file_type: nil,
        output_url: nil,
        status: AVAssetExportSessionStatusUnknown,
        error: nil,
        should_optimize_for_network_use: false,
        time_range_start: 0.0,
        time_range_duration: f64::MAX,
        metadata: nil,
        video_composition: nil,
        audio_mix: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// =========================================================================
// MARK: - Constructors
// =========================================================================

// `+exportSessionWithAsset:presetName:`
+ (id)exportSessionWithAsset:(id)asset presetName:(id)preset_name {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithAsset:asset presetName:preset_name];
    crate::objc::autorelease(env, new)
}

+ (id)allExportPresets { // NSArray<NSString*>*
    let presets = [
        "AVAssetExportPresetLowQuality",
        "AVAssetExportPresetMediumQuality",
        "AVAssetExportPresetHighestQuality",
        "AVAssetExportPreset640x480",
        "AVAssetExportPreset960x540",
        "AVAssetExportPreset1280x720",
        "AVAssetExportPreset1920x1080",
        "AVAssetExportPresetAppleM4A",
        "AVAssetExportPresetPassthrough",
    ];
    let arr: id = msg_class![env; NSMutableArray new];
    for &p in &presets {
        let s = ns_string::get_static_str(env, p);
        let _: () = msg![env; arr addObject:s];
    }
    crate::objc::autorelease(env, arr)
}

+ (id)exportPresetsCompatibleWithAsset:(id)_asset { // NSArray<NSString*>*
    msg_class![env; AVAssetExportSession allExportPresets]
}

+ (id)exportPresetsCompatibleWithAsset:(id)_asset
              outputFileType:(id)_file_type { // NSArray<NSString*>*
    msg_class![env; AVAssetExportSession allExportPresets]
}
    
// `-initWithAsset:presetName:`
- (id)initWithAsset:(id)asset presetName:(id)preset_name {
    retain(env, asset);
    retain(env, preset_name);
    {
        let host = env.objc.borrow_mut::<AVAssetExportSessionHostObject>(this);
        host.asset       = asset;
        host.preset_name = preset_name;
    }
    this
}

// =========================================================================
// MARK: - Dealloc
// =========================================================================

- (())dealloc {
    let h = env.objc.borrow::<AVAssetExportSessionHostObject>(this);
    let (asset, preset, oft, url, error, meta, vc, am) = (
        h.asset, h.preset_name, h.output_file_type, h.output_url,
        h.error, h.metadata, h.video_composition, h.audio_mix,
    );
    release(env, asset);
    release(env, preset);
    release(env, oft);
    release(env, url);
    release(env, error);
    release(env, meta);
    release(env, vc);
    release(env, am);
    env.objc.dealloc_object(this, &mut env.mem)
}

// =========================================================================
// MARK: - Properties
// =========================================================================

- (id)asset {
    env.objc.borrow::<AVAssetExportSessionHostObject>(this).asset
}

- (id)presetName { // NSString*
    env.objc.borrow::<AVAssetExportSessionHostObject>(this).preset_name
}

- (id)outputFileType { // NSString* (AVFileType)
    env.objc.borrow::<AVAssetExportSessionHostObject>(this).output_file_type
}

- (())setOutputFileType:(id)file_type {
    let old = env.objc.borrow::<AVAssetExportSessionHostObject>(this).output_file_type;
    release(env, old);
    retain(env, file_type);
    env.objc.borrow_mut::<AVAssetExportSessionHostObject>(this).output_file_type = file_type;
}

- (id)outputURL { // NSURL*
    env.objc.borrow::<AVAssetExportSessionHostObject>(this).output_url
}

- (())setOutputURL:(id)url {
    let old = env.objc.borrow::<AVAssetExportSessionHostObject>(this).output_url;
    release(env, old);
    retain(env, url);
    env.objc.borrow_mut::<AVAssetExportSessionHostObject>(this).output_url = url;
}

- (AVAssetExportSessionStatus)status {
    env.objc.borrow::<AVAssetExportSessionHostObject>(this).status
}

- (id)error { // NSError*
    env.objc.borrow::<AVAssetExportSessionHostObject>(this).error
}

- (bool)shouldOptimizeForNetworkUse {
    env.objc.borrow::<AVAssetExportSessionHostObject>(this).should_optimize_for_network_use
}

- (())setShouldOptimizeForNetworkUse:(bool)v {
    env.objc.borrow_mut::<AVAssetExportSessionHostObject>(this)
        .should_optimize_for_network_use = v;
}

// =========================================================================
// MARK: - Time range (CMTimeRange stored as two f64)
// =========================================================================

// setTimeRange: takes a CMTimeRange struct {CMTime start, CMTime duration}.
// CMTime is {i64 value, i32 timescale, u32 flags, i64 epoch} = 24 bytes.
// CMTimeRange is therefore 48 bytes. We read start and duration as seconds.
- (())setTimeRange:(crate::mem::ConstPtr<u8>)range_ptr {
    if range_ptr.is_null() { return; }
    // CMTime layout: value(i64) timescale(i32) flags(u32) epoch(i64) = 24 bytes
    let value_ptr:     crate::mem::ConstPtr<i64> = range_ptr.cast();
    let timescale_ptr: crate::mem::ConstPtr<i32> = crate::mem::Ptr::from_bits(range_ptr.to_bits() + 8);
    let dur_value_ptr: crate::mem::ConstPtr<i64> = crate::mem::Ptr::from_bits(range_ptr.to_bits() + 24);
    let dur_ts_ptr:    crate::mem::ConstPtr<i32> = crate::mem::Ptr::from_bits(range_ptr.to_bits() + 32);

    let start_value = env.mem.read(value_ptr);
    let start_ts    = env.mem.read(timescale_ptr);
    let dur_value   = env.mem.read(dur_value_ptr);
    let dur_ts      = env.mem.read(dur_ts_ptr);

    let start_secs = if start_ts != 0 { start_value as f64 / start_ts as f64 } else { 0.0 };
    let dur_secs   = if dur_ts != 0   { dur_value   as f64 / dur_ts   as f64 } else { f64::MAX };

    let host = env.objc.borrow_mut::<AVAssetExportSessionHostObject>(this);
    host.time_range_start    = start_secs;
    host.time_range_duration = dur_secs;
    log_dbg!("AVAssetExportSession setTimeRange: start={} duration={}", start_secs, dur_secs);
}

// =========================================================================
// MARK: - Metadata / composition
// =========================================================================

- (id)metadata { // NSArray*
    let m = env.objc.borrow::<AVAssetExportSessionHostObject>(this).metadata;
    if m != nil { m } else { msg_class![env; NSArray array] }
}

- (())setMetadata:(id)metadata {
    let old = env.objc.borrow::<AVAssetExportSessionHostObject>(this).metadata;
    release(env, old);
    retain(env, metadata);
    env.objc.borrow_mut::<AVAssetExportSessionHostObject>(this).metadata = metadata;
}

- (id)videoComposition {
    env.objc.borrow::<AVAssetExportSessionHostObject>(this).video_composition
}

- (())setVideoComposition:(id)vc {
    let old = env.objc.borrow::<AVAssetExportSessionHostObject>(this).video_composition;
    release(env, old);
    retain(env, vc);
    env.objc.borrow_mut::<AVAssetExportSessionHostObject>(this).video_composition = vc;
}

- (id)audioMix {
    env.objc.borrow::<AVAssetExportSessionHostObject>(this).audio_mix
}

- (())setAudioMix:(id)am {
    let old = env.objc.borrow::<AVAssetExportSessionHostObject>(this).audio_mix;
    release(env, old);
    retain(env, am);
    env.objc.borrow_mut::<AVAssetExportSessionHostObject>(this).audio_mix = am;
}

// =========================================================================
// MARK: - Progress
// =========================================================================

- (f32)progress {
    let status = env.objc.borrow::<AVAssetExportSessionHostObject>(this).status;
    match status {
        AVAssetExportSessionStatusCompleted => 1.0,
        AVAssetExportSessionStatusFailed |
        AVAssetExportSessionStatusCancelled => 0.0,
        _ => 0.0,
    }
}

- (id)supportedFileTypes { // NSArray<NSString*>*
    let types = [
        "com.apple.m4a-audio",
        "com.apple.m4v-video",
        "public.mpeg-4",
        "public.mpeg-4-audio",
        "public.aiff-audio",
        "com.microsoft.waveform-audio",
        "public.3gpp",
        "public.3gpp2",
        "public.movie",
    ];
    let arr: id = msg_class![env; NSMutableArray new];
    for &t in &types {
        let s = ns_string::get_static_str(env, t);
        let _: () = msg![env; arr addObject:s];
    }
    crate::objc::autorelease(env, arr)
}

// =========================================================================
// MARK: - Export
// touchHLE has no transcoding pipeline. We immediately mark the export as
// completed and call the completion handler so the app can proceed.
// =========================================================================

- (())exportAsynchronouslyWithCompletionHandler:(id)handler {
    log_dbg!("AVAssetExportSession exportAsynchronouslyWithCompletionHandler: — completing immediately");

    // Mark as completed immediately.
    env.objc.borrow_mut::<AVAssetExportSessionHostObject>(this).status =
        AVAssetExportSessionStatusCompleted;

    // Call the completion handler block (no arguments — it's a ^void(void) block).
    if handler != nil {
        let sel = env.objc.lookup_selector("invoke").unwrap();
        let responds: bool = msg![env; handler respondsToSelector:sel];
        if responds {
            let _: () = msg![env; handler invoke];
        } else {
            log!("AVAssetExportSession: completion handler does not respond to invoke");
        }
    }
}

- (())cancelExport {
    log_dbg!("AVAssetExportSession cancelExport");
    env.objc.borrow_mut::<AVAssetExportSessionHostObject>(this).status =
        AVAssetExportSessionStatusCancelled;
}

// =========================================================================
// MARK: - Estimated output file length
// =========================================================================

- (i64)estimatedOutputFileLength {
    // Return 0 — no real transcoding so no size estimate.
    0i64
}

- (f64)fileLengthLimitPerSecond {
    0.0f64
}

- (())setFileLengthLimitPerSecond:(f64)_limit {
    log_dbg!("AVAssetExportSession setFileLengthLimitPerSecond: — ignored");
}

// =========================================================================
// MARK: - Description
// =========================================================================

- (id)description {
    let status = env.objc.borrow::<AVAssetExportSessionHostObject>(this).status;
    let status_str = match status {
        AVAssetExportSessionStatusUnknown   => "Unknown",
        AVAssetExportSessionStatusWaiting   => "Waiting",
        AVAssetExportSessionStatusExporting => "Exporting",
        AVAssetExportSessionStatusCompleted => "Completed",
        AVAssetExportSessionStatusFailed    => "Failed",
        AVAssetExportSessionStatusCancelled => "Cancelled",
        _                                   => "?",
    };
    let s = format!(
        "<AVAssetExportSession: {:?}; status={}>",
        this, status_str
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

};
