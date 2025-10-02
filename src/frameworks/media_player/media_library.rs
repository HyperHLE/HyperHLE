/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaLibrary`.

use crate::frameworks::foundation::NSUInteger;
use crate::objc::{id, msg, nil, objc_classes, ClassExports};

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaLibrary: NSObject

+ (id)defaultMediaLibrary {
    log!("TODO: [MPMediaLibrary defaultMediaLibrary] (not implemented yet)");
    nil
}

@end

@implementation MPMoviePlayerViewController: NSObject

- (id)moviePlayer {
    nil
}

- (id)initWithContentURL:(NSUInteger)_url {
    msg![env; this init]
}

@end 

@implementation MPMediaItem: NSObject
@end 

@implementation GKPeerPickerController: NSObject
@end

@implementation MFMessageComposeViewController: UINavigationController

+ (id)canSendText {
    nil
}

@end

@implementation ABPeoplePickerNavigationController: NSObject
@end

};
