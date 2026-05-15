/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AVPlayerItem` notification name constants.

use crate::dyld::{ConstantExports, HostConstant};

const AVPlayerItemDidPlayToEndTimeNotification: &str = "AVPlayerItemDidPlayToEndTimeNotification";
const AVPlayerItemFailedToPlayToEndTimeNotification: &str =
    "AVPlayerItemFailedToPlayToEndTimeNotification";
const AVPlayerItemFailedToPlayToEndTimeErrorKey: &str =
    "AVPlayerItemFailedToPlayToEndTimeErrorKey";

pub const CONSTANTS: ConstantExports = &[
    (
        "_AVPlayerItemDidPlayToEndTimeNotification",
        HostConstant::NSString(AVPlayerItemDidPlayToEndTimeNotification),
    ),
    (
        "_AVPlayerItemFailedToPlayToEndTimeNotification",
        HostConstant::NSString(AVPlayerItemFailedToPlayToEndTimeNotification),
    ),
    (
        "_AVPlayerItemFailedToPlayToEndTimeErrorKey",
        HostConstant::NSString(AVPlayerItemFailedToPlayToEndTimeErrorKey),
    ),
];
