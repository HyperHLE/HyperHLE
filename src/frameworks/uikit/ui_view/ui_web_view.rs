/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIWebView`.

use crate::frameworks::foundation::ns_string::to_rust_string;
use crate::frameworks::foundation::{NSUInteger, NSInteger};
use crate::msg;
use crate::objc::{id, nil, objc_classes, retain, ClassExports, NSZonePtr};
use std::borrow::Cow;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIWebView: UIView

+ (id)instanceMethodSignatureForSelector:(NSUInteger)_selector {
    msg![env; this init]
}

+ (id)methodReturnType {
    nil
}

+ (id)numberOfArguments {
    nil
}

// NSCopying implementation
- (id)copyWithZone:(NSZonePtr)_zone {
    retain(env, this)
}

- (id)stopLoading {
    nil
}

- (id)delegate {
    nil
}

- (id)request {
    nil
}

- (id)stringByEvaluatingJavaScriptFromString:(NSUInteger)string {
    msg![env; this init]
}

- (())loadData:(NSInteger)data MIMEType:(bool)_type textEncodingName:(bool)_name baseURL:(bool)_url {
    // TODO
}

- (())loadHTMLString:(NSInteger)string baseURL:(bool)_url {
    // TODO
}

// NSCoding implementation
- (id)initWithCoder:(NSUInteger)coder {
    msg![env; this init]
}

- (())setScalesPageToFit:(bool)_scales {
    // TODO
}
- (())setDelegate:(id)_delegate {
    // TODO
}
- (())loadRequest:(id)request { // NSURLRequest*
    let url_string = if request != nil {
        let url = msg![env; request URL];
        let url_desc = msg![env; url description];
        to_rust_string(env, url_desc)
    } else {
        Cow::default()
    };
    log!("TODO: [(UIWebView*) {:?} loadRequest:{:?} ({})]", this, request, url_string);
}

- (())setDataDetectorTypes:(bool)detector {
    log!("TODO: setDataDetectorTypes:{}", detector);
}

- (())setDetectsPhoneNumbers:(bool)numbers {
    log!("TODO: setDetectsPhoneNumbers:{}", numbers);
}

@end

};
