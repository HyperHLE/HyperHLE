/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `GKLocalPlayer`.

use crate::frameworks::foundation::NSInteger;
use crate::dyld::{ConstantExports, HostConstant};
use crate::objc::{id, nil, objc_classes, ClassExports};

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// TODO: proper inheritance chain
@implementation GKLocalPlayer: NSObject
+ (id)localPlayer {
    nil
}

@end

@implementation GKSession: NSObject

- (id)peerID {
    nil
}

- (id)disconnectFromAllPeers {
    nil
}

- (())initWithSessionID:(NSInteger)session displayName:(bool)_name sessionMode:(bool)mode {
    // TODO
}

- (())setAvailable:(bool)available {
    log!("TODO: setAvailable:{}", available);
}

- (())setDelegate:(bool)delegate {
    log!("TODO: setDelegate:{}", delegate);
}

- (())setDataReceiveHandler:(NSInteger)handler withContext:(bool)_context {
    // TODO
}

@end

};

pub const GKPlayerAuthenticationDidChangeNotificationName: &str =
    "GKPlayerAuthenticationDidChangeNotificationName";

/// `NSNotificationName` values.
pub const CONSTANTS: ConstantExports = &[(
    "_GKPlayerAuthenticationDidChangeNotificationName",
    HostConstant::NSString(GKPlayerAuthenticationDidChangeNotificationName),
)];
