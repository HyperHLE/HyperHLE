/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMusicPlayerController` etc.

use crate::{
    dyld::{ConstantExports, HostConstant},
    objc::{id, nil, objc_classes, ClassExports},
};

pub const MPMusicPlayerControllerNowPlayingItemDidChangeNotification: &str =
    "MPMusicPlayerControllerNowPlayingItemDidChangeNotification";
pub const MPMusicPlayerControllerPlaybackStateDidChangeNotification: &str =
    "MPMusicPlayerControllerPlaybackStateDidChangeNotification";
pub const MPMusicPlayerControllerVolumeDidChangeNotification: &str =
    "MPMusicPlayerControllerVolumeDidChangeNotification";
pub const MPMusicPlayerControllerQueueDidChange: &str =
    "MPMusicPlayerControllerQueueDidChange";

/// `NSNotificationName` values.
pub const CONSTANTS: ConstantExports = &[
    (
        "_MPMusicPlayerControllerNowPlayingItemDidChangeNotification",
        HostConstant::NSString(MPMusicPlayerControllerNowPlayingItemDidChangeNotification),
    ),
    (
        "_MPMusicPlayerControllerPlaybackStateDidChangeNotification",
        HostConstant::NSString(MPMusicPlayerControllerPlaybackStateDidChangeNotification),
    ),
    (
        "_MPMusicPlayerControllerVolumeDidChangeNotification",
        HostConstant::NSString(MPMusicPlayerControllerVolumeDidChangeNotification),
    ),
    (
        "_MPMusicPlayerControllerQueueDidChange",
        HostConstant::NSString(MPMusicPlayerControllerQueueDidChange),
    ),
];

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMusicPlayerController: NSObject

+ (id)iPodMusicPlayer {
    log_dbg!(
        "TODO: [(MPMusicPlayerController*){:?} iPodMusicPlayer]",
        this
    );
    nil
}

+ (id)applicationMusicPlayer {
    log_dbg!(
        "TODO: [(MPMusicPlayerController*){:?} applicationMusicPlayer]",
        this
    );
    nil
}

@end

@implementation MPMediaPlaylist: NSObject
@end

@implementation MPAVController: NSObject
+ (id)sharedInstance {
    nil
}

- (id)sharedInstance {
    nil
}

@end

};
