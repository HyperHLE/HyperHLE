/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! `NSURLConnection`.
//!
//! This is a "Success Stub" implementation. It does not perform real networking,
//! but it tells the app that requests succeeded with empty data. This prevents
//! many SDKs (like PlayHaven) from hanging while waiting for a connection.

use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain,
    ClassExports, HostObject, NSZonePtr,
};

struct NSURLConnectionHostObject {
    delegate: id,
    cancelled: bool,
}
impl HostObject for NSURLConnectionHostObject {}

/// Helper to create a fake successful NSURLResponse
fn make_fake_response(env: &mut crate::Environment, request: id) -> id {
    let url: id = if request != nil { msg![env; request URL] } else { nil };
    let response: id = msg_class![env; NSURLResponse alloc];
    let response: id = msg![env; response initWithURL:url
                                             MIMEType:nil
                                expectedContentLength:0
                                     textEncodingName:nil];
    autorelease(env, response)
}

/// Triggers the sequence of delegate calls for a successful (but empty) download
fn notify_delegate_success(
    env: &mut crate::Environment,
    connection: id,
    delegate: id,
    request: id,
) {
    if delegate == nil { return; }
    
    log!("NSURLConnection: Faking success for delegate {:?}", delegate);

    // 1. Tell delegate we got a response (200 OK)
    let response = make_fake_response(env, request);
    let _: () = msg![env; delegate connection:connection didReceiveResponse:response];

    // 2. Tell delegate we finished
    let _: () = msg![env; delegate connectionDidFinishLoading:connection];
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSURLConnection: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host = Box::new(NSURLConnectionHostObject {
        delegate: nil,
        cancelled: false,
    });
    env.objc.alloc_object(this, host, &mut env.mem)
}

+ (bool)canHandleRequest:(id)_request {
    true
}

// MARK: - Synchronous API
+ (id)sendSynchronousRequest:(id)request
           returningResponse:(MutPtr<id>)response_ptr
                       error:(MutPtr<id>)error_ptr {

    log!("NSURLConnection sendSynchronousRequest: Faking success");

    if !response_ptr.is_null() {
        let response = make_fake_response(env, request);
        retain(env, response); 
        env.mem.write(response_ptr, response);
    }

    if !error_ptr.is_null() {
        env.mem.write(error_ptr, nil);
    }

    // Return empty data instead of nil
    msg_class![env; NSData data]
}

// MARK: - Asynchronous API
+ (id)connectionWithRequest:(id)request
                   delegate:(id)delegate {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithRequest:request delegate:delegate];
    autorelease(env, new)
}

- (id)initWithRequest:(id)request
             delegate:(id)delegate {
    msg![env; this initWithRequest:request delegate:delegate startImmediately:true]
}

- (id)initWithRequest:(id)request
             delegate:(id)delegate
     startImmediately:(bool)start_immediately {

    if request == nil {
        release(env, this);
        return nil;
    }

    retain(env, delegate);
    {
        let mut host = env.objc.borrow_mut::<NSURLConnectionHostObject>(this);
        host.delegate  = delegate;
        host.cancelled = false;
    }

    if start_immediately {
        // We need to trigger success. We use a deferred approach usually, 
        // but for a stub, direct call is often enough to unblock the UI.
        retain(env, this);
        notify_delegate_success(env, this, delegate, request);
        env.objc.borrow_mut::<NSURLConnectionHostObject>(this).cancelled = true;
        autorelease(env, this);
    }

    this
}

- (())start {
    let (delegate, already, request) = {
        let host = env.objc.borrow::<NSURLConnectionHostObject>(this);
        // Note: we don't store request in host object currently, 
        // passing nil to helper is safe enough for a stub.
        (host.delegate, host.cancelled, nil)
    };

    if !already {
        env.objc.borrow_mut::<NSURLConnectionHostObject>(this).cancelled = true;
        retain(env, this);
        notify_delegate_success(env, this, delegate, request);
        autorelease(env, this);
    }
}

- (())cancel {
    env.objc.borrow_mut::<NSURLConnectionHostObject>(this).cancelled = true;
}

- (())dealloc {
    let delegate = env.objc.borrow::<NSURLConnectionHostObject>(this).delegate;
    release(env, delegate);
    env.objc.dealloc_object(this, &mut env.mem);
}

@end
};
