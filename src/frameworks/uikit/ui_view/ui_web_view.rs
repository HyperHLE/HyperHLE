/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIWebView`.

use crate::frameworks::foundation::ns_string::{self, to_rust_string};
use crate::frameworks::core_graphics::CGRect;
use crate::msg;
use crate::objc::{id, nil, objc_classes, release, ClassExports, HostObject, NSZonePtr};
use super::NSUInteger; // <-- ИСПРАВЛЕНО
use std::borrow::Cow;

struct UIWebViewHostObject {
    /// UIWebViewDelegate — weak reference
    delegate: id,
    scales_page_to_fit: bool,
    detects_phone_numbers: bool,
    data_detector_types: NSUInteger,
    allows_inline_media_playback: bool,
    media_playback_requires_user_action: bool,
    /// NSString* — last URL string passed to loadRequest:
    current_url: id,
    loading: bool,
}
impl HostObject for UIWebViewHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIWebView: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIWebViewHostObject {
        delegate: nil,
        scales_page_to_fit: false,
        detects_phone_numbers: true,
        data_detector_types: 0,
        allows_inline_media_playback: false,
        media_playback_requires_user_action: true,
        current_url: nil,
        loading: false,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    this
}

- (id)initWithFrame:(CGRect)_frame {
    this
}

- (id)initWithCoder:(id)_coder {
    this
}

- (())dealloc {
    let current_url = env.objc.borrow::<UIWebViewHostObject>(this).current_url;
    release(env, current_url);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Delegate

- (id)delegate {
    env.objc.borrow::<UIWebViewHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    // Weak reference — no retain.
    env.objc.borrow_mut::<UIWebViewHostObject>(this).delegate = delegate;
}

// MARK: - Loading

- (())loadRequest:(id)request { // NSURLRequest*
    let url_string: Cow<str> = if request != nil {
        let url: id = msg![env; request URL];
        let url_desc: id = msg![env; url description];
        to_rust_string(env, url_desc)
    } else {
        Cow::default()
    };
    log!("TODO: UIWebView loadRequest: {} (stubbed, will not load)", url_string);

    // Store the URL string and mark as loading briefly.
    let old = env.objc.borrow::<UIWebViewHostObject>(this).current_url;
    release(env, old);
    let ns_url = ns_string::from_rust_string(env, url_string.into_owned());
    env.objc.borrow_mut::<UIWebViewHostObject>(this).current_url = ns_url;
    env.objc.borrow_mut::<UIWebViewHostObject>(this).loading = true;
}

- (())loadHTMLString:(id)html // NSString*
            baseURL:(id)_base_url { // NSURL*
    let html_str = to_rust_string(env, html);
    log!("TODO: UIWebView loadHTMLString: ({} chars, stubbed)", html_str.len());
    env.objc.borrow_mut::<UIWebViewHostObject>(this).loading = true;
}

- (())loadData:(id)_data   // NSData*
      MIMEType:(id)mime    // NSString*
      textEncodingName:(id)_enc  // NSString*
       baseURL:(id)_base_url { // NSURL*
    let mime_str = to_rust_string(env, mime);
    log!("TODO: UIWebView loadData:MIMEType:{} (stubbed)", mime_str);
    env.objc.borrow_mut::<UIWebViewHostObject>(this).loading = true;
}

- (())reload {
    log!("TODO: UIWebView reload (stubbed)");
}

- (())stopLoading {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).loading = false;
}

- (bool)isLoading {
    env.objc.borrow::<UIWebViewHostObject>(this).loading
}

// MARK: - Navigation

- (bool)canGoBack {
    false
}

- (bool)canGoForward {
    false
}

- (())goBack {
    log!("TODO: UIWebView goBack (stubbed)");
}

- (())goForward {
    log!("TODO: UIWebView goForward (stubbed)");
}

// MARK: - JavaScript

- (id)stringByEvaluatingJavaScriptFromString:(id)script { // NSString*
    let script_str = to_rust_string(env, script);
    log!("TODO: UIWebView stringByEvaluatingJavaScriptFromString: {:?} (returning nil)", script_str);
    nil
}

// MARK: - Properties

- (bool)scalesPageToFit {
    env.objc.borrow::<UIWebViewHostObject>(this).scales_page_to_fit
}

- (())setScalesPageToFit:(bool)scales {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).scales_page_to_fit = scales;
}

- (bool)detectsPhoneNumbers {
    env.objc.borrow::<UIWebViewHostObject>(this).detects_phone_numbers
}

- (())setDetectsPhoneNumbers:(bool)value {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).detects_phone_numbers = value;
}

- (NSUInteger)dataDetectorTypes {
    env.objc.borrow::<UIWebViewHostObject>(this).data_detector_types
}

- (())setDataDetectorTypes:(NSUInteger)types {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).data_detector_types = types;
}

- (bool)allowsInlineMediaPlayback {
    env.objc.borrow::<UIWebViewHostObject>(this).allows_inline_media_playback
}

- (())setAllowsInlineMediaPlayback:(bool)value {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).allows_inline_media_playback = value;
}

- (bool)mediaPlaybackRequiresUserAction {
    env.objc.borrow::<UIWebViewHostObject>(this).media_playback_requires_user_action
}

- (())setMediaPlaybackRequiresUserAction:(bool)value {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).media_playback_requires_user_action = value;
}

// MARK: - Request / URL accessors

- (id)request { // NSURLRequest*
    // We don't construct a real NSURLRequest; return nil.
    nil
}

@end

};
