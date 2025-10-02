/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSURLConnection`.

use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::objc::{autorelease, id, msg, nil, objc_classes, release, ClassExports};

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSURLConnection: NSObject

+ (id)connectionWithRequest:(id)request // NSURLRequest *
                   delegate:(id)delegate {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithRequest:request delegate:delegate];
    autorelease(env, new)
}

+ (id)length {
  nil
}

+ (id)bytes {
  nil
}

+ (id)canHandleRequest:(NSUInteger)_handle {
    msg![env; this init]
}

+ (id)canHandleRequest {
  nil
}

+ (())sendSynchronousRequest:(NSInteger)request returningResponse:(bool)_response error:(bool)_error {
    // TODO
}

- (id)initWithRequest:(id)request // NSURLRequest *
             delegate:(id)delegate {
    msg![env; this initWithRequest:request delegate:delegate startImmediately:true]
}

- (id)initWithRequest:(id)request // NSURLRequest *
             delegate:(id)delegate
     startImmediately:(bool)start_immediately {
    log!(
        "TODO: [(NSURLConnection *){:?} initWithRequest:{:?} delegate:{:?} startImmediately:{}]",
        this,
        request,
        delegate,
        start_immediately,
    );
    release(env, this);
    nil
}

@end

};
