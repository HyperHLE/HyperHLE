/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSHTTPCookie`.
//!
//! Per Apple's documentation:
//! <https://developer.apple.com/documentation/foundation/nshttpcookie>
//!
//! touchHLE does not perform networking, but apps build cookies from a
//! property dictionary before their first request — typically to hand them to
//! `NSHTTPCookieStorage` or to `NSMutableURLRequest`. We model the object
//! closely enough for that: it retains the property dictionary and answers
//! the documented accessors from it.
//!
//! `+cookieWithProperties:` follows Apple's contract and returns `nil` unless
//! `Name`, `Value`, `Domain` and `Path` are all present as non-empty strings;
//! those four keys are required by `<Foundation/NSHTTPCookie.h>`.

use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, HostObject, NSZonePtr,
};
use crate::Environment;

#[derive(Default)]
struct NSHTTPCookieHostObject {
    /// The property dictionary the cookie was created from (retained).
    properties: id,
}
impl HostObject for NSHTTPCookieHostObject {}

/// The four keys Apple requires before a property dictionary counts as a
/// cookie (`NSHTTPCookieName` and friends, whose literal values follow).
const REQUIRED_KEYS: [&'static str; 4] = ["Name", "Value", "Domain", "Path"];

/// Look up `key` in a cookie property dictionary.
fn property(env: &mut Environment, properties: id, key: &'static str) -> id {
    if properties == nil {
        return nil;
    }
    let key = get_static_str(env, key);
    msg![env; properties objectForKey:key]
}

/// Apple's validation for `+cookieWithProperties:`: required keys must be
/// present and hold non-empty strings.
fn is_valid_properties(env: &mut Environment, properties: id) -> bool {
    if properties == nil {
        return false;
    }
    let string_class: crate::objc::Class = msg_class![env; NSString class];
    for key in REQUIRED_KEYS {
        let value = property(env, properties, key);
        if value == nil || !msg![env; value isKindOfClass:string_class] {
            return false;
        }
        let length: NSUInteger = msg![env; value length];
        if length == 0 {
            return false;
        }
    }
    true
}

fn cookie_properties(env: &mut Environment, cookie: id) -> id {
    env.objc
        .borrow::<NSHTTPCookieHostObject>(cookie)
        .properties
}

fn string_for_key(env: &mut Environment, cookie: id, key: &'static str) -> id {
    let properties = cookie_properties(env, cookie);
    property(env, properties, key)
}

fn bool_for_key(env: &mut Environment, cookie: id, key: &'static str) -> bool {
    let value = string_for_key(env, cookie, key);
    if value == nil {
        false
    } else {
        msg![env; value boolValue]
    }
}

pub const CLASSES: crate::objc::ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSHTTPCookie: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<NSHTTPCookieHostObject>::default(), &mut env.mem)
}

+ (id)cookieWithProperties:(id)properties {
    if !is_valid_properties(env, properties) {
        return nil;
    }
    let cookie: id = msg![env; this alloc];
    let cookie: id = msg![env; cookie initWithProperties:properties];
    autorelease(env, cookie)
}

+ (id)cookiesWithResponseHeaderFields:(id)_header_fields forURL:(id)_url {
    // With no networking there are no response headers to parse; an empty
    // (non-nil) array is the documented "no cookies" answer.
    msg_class![env; NSArray array]
}

- (id)initWithProperties:(id)properties {
    if !is_valid_properties(env, properties) {
        release(env, this);
        return nil;
    }
    retain(env, properties);
    env.objc.borrow_mut::<NSHTTPCookieHostObject>(this).properties = properties;
    this
}

- (id)properties {
    let properties = cookie_properties(env, this);
    if properties == nil {
        return nil;
    }
    let copy: id = msg![env; properties copy];
    autorelease(env, copy)
}

// MARK: - Accessors

- (id)name { string_for_key(env, this, "Name") }
- (id)value { string_for_key(env, this, "Value") }
- (id)domain { string_for_key(env, this, "Domain") }
- (id)path { string_for_key(env, this, "Path") }
- (id)comment { string_for_key(env, this, "Comment") }
- (id)commentURL { string_for_key(env, this, "CommentURL") }
- (id)portList { string_for_key(env, this, "Port") }
- (id)expiresDate { string_for_key(env, this, "Expires") }
- (bool)isSecure { bool_for_key(env, this, "Secure") }
- (bool)isHTTPOnly { bool_for_key(env, this, "HTTPOnly") }

// Apple: "A cookie is session-only if it has no expiration date."
- (bool)isSessionOnly { string_for_key(env, this, "Expires") == nil }

- (NSUInteger)version {
    let value = string_for_key(env, this, "Version");
    if value == nil {
        return 0;
    }
    let version: NSInteger = msg![env; value integerValue];
    version.max(0) as NSUInteger
}

- (())dealloc {
    let properties = {
        let mut host = env.objc.borrow_mut::<NSHTTPCookieHostObject>(this);
        std::mem::replace(&mut host.properties, nil)
    };
    release(env, properties);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};