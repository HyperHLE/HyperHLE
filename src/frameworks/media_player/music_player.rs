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
pub const MPMediaItemPropertyPersistentID: &str = "persistentID";
pub const MPMediaItemPropertyAlbumArtist: &str = "albumArtist";
pub const MPMediaItemPropertyAlbumTitle: &str = "albumTitle";
pub const MPMediaItemPropertyArtist: &str = "artist";
pub const MPMediaItemPropertyComposer: &str = "composer";
pub const MPMediaItemPropertyGenre: &str = "genre";
pub const MPMediaItemPropertyTitle: &str = "title";

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
        "_MPMediaItemPropertyPersistentID",
        HostConstant::NSString(MPMediaItemPropertyPersistentID),
    ),
    (
        "_MPMediaItemPropertyAlbumArtist",
        HostConstant::NSString(MPMediaItemPropertyAlbumArtist),
    ),
    (
        "_MPMediaItemPropertyAlbumTitle",
        HostConstant::NSString(MPMediaItemPropertyAlbumTitle),
    ),
    (
        "_MPMediaItemPropertyArtist",
        HostConstant::NSString(MPMediaItemPropertyArtist),
    ),
    (
        "_MPMediaItemPropertyComposer",
        HostConstant::NSString(MPMediaItemPropertyComposer),
    ),
    (
        "_MPMediaItemPropertyGenre",
        HostConstant::NSString(MPMediaItemPropertyGenre),
    ),
    (
        "_MPMediaItemPropertyTitle",
        HostConstant::NSString(MPMediaItemPropertyTitle),
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

};
