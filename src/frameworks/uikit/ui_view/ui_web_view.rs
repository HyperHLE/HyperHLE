/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIWebView`.

use crate::android_web_view;
use crate::frameworks::core_graphics::{cg_image, CGRect};
use crate::frameworks::foundation::ns_string::{self, to_rust_string};
use crate::frameworks::foundation::NSUInteger;
use crate::frameworks::uikit::ui_view::UIViewHostObject;
use crate::image::Image;
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, msg_class, msg_super, nil, objc_classes, release,
    retain, Class, ClassExports, NSZonePtr,
};
use crate::Environment;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

// UIWebViewNavigationType constants
pub type UIWebViewNavigationType = i32;
pub const UIWebViewNavigationTypeLinkClicked: UIWebViewNavigationType = 0;
pub const UIWebViewNavigationTypeFormSubmitted: UIWebViewNavigationType = 1;
pub const UIWebViewNavigationTypeBackForward: UIWebViewNavigationType = 2;
pub const UIWebViewNavigationTypeReload: UIWebViewNavigationType = 3;
pub const UIWebViewNavigationTypeFormResubmitted: UIWebViewNavigationType = 4;
pub const UIWebViewNavigationTypeOther: UIWebViewNavigationType = 5;

// UIDataDetectorTypes bitmask
pub type UIDataDetectorTypes = NSUInteger;
pub const UIDataDetectorTypePhoneNumber: UIDataDetectorTypes = 1 << 0;
pub const UIDataDetectorTypeLink: UIDataDetectorTypes = 1 << 1;
pub const UIDataDetectorTypeAddress: UIDataDetectorTypes = 1 << 2;
pub const UIDataDetectorTypeCalendarEvent: UIDataDetectorTypes = 1 << 3;
pub const UIDataDetectorTypeNone: UIDataDetectorTypes = 0;
pub const UIDataDetectorTypeAll: UIDataDetectorTypes = u32::MAX as UIDataDetectorTypes;

/// A load that was initiated while the view had no on-screen extent (or
/// the host window wasn't ready), so the native overlay could not be
/// created yet. The load is remembered and retried from `-setFrame:`
/// once the view is laid out. See [retry_pending_load].
#[derive(Clone)]
enum PendingLoad {
    Url(String),
    /// (payload, MIME type)
    Data(String, String),
}

#[derive(Default)]
struct UIWebViewHostObject {
    superclass: UIViewHostObject,
    /// UIWebViewDelegate — weak reference (no retain per Apple docs)
    delegate: id,
    scales_page_to_fit: bool,
    detects_phone_numbers: bool,
    data_detector_types: UIDataDetectorTypes,
    allows_inline_media_playback: bool,
    media_playback_requires_user_action: bool,
    media_playback_allows_air_play: bool,
    suppress_incremental_rendering: bool,
    keyboard_display_requires_user_action: bool,
    pagination_mode: i32,
    pagination_breaking_mode: i32,
    page_length: f64,
    gap_between_pages: f64,
    /// NSString* — last URL string passed to loadRequest:
    current_url: id,

    loading: bool,
    /// Simple back/forward stack — NSString* items.
    back_stack: Vec<id>,
    forward_stack: Vec<id>,
    /// Host-side native overlay (Android WebView) id. `-2` = no overlay has
    /// been created yet (first `show` seeds it with a page); `-1` = a show
    /// was attempted but unavailable/zero-size; `>= 0` = live overlay id.
    overlay_id: i32,
    /// Load deferred until the view has an on-screen extent.
    pending_load: Option<PendingLoad>,
    /// How many times the deferred load has been retried on a timer so
    /// far (bounded so a view that never gets a frame can't poll forever).
    deferred_polls: u32,
}
impl_HostObject_with_superclass!(UIWebViewHostObject);

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIWebView: UIView

// =========================================================================
// MARK: - Allocation
// =========================================================================

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIWebViewHostObject {
        superclass: UIViewHostObject::default(),
        delegate: nil,
        scales_page_to_fit: false,
        detects_phone_numbers: true,
        data_detector_types: UIDataDetectorTypePhoneNumber,
        allows_inline_media_playback: false,
        media_playback_requires_user_action: true,
        media_playback_allows_air_play: true,
        suppress_incremental_rendering: false,
        keyboard_display_requires_user_action: true,

        pagination_mode: 0,
        pagination_breaking_mode: 0,
        page_length: 0.0,
        gap_between_pages: 0.0,
        current_url: nil,
        loading: false,
        back_stack: Vec::new(),
        forward_stack: Vec::new(),
        overlay_id: -2,
        pending_load: None,
        deferred_polls: 0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// =========================================================================
// MARK: - Initializers
// =========================================================================

- (id)init {
    this
}

- (id)initWithFrame:(CGRect)_frame {
    this
}

- (id)initWithCoder:(id)_coder {
    this
}

// `UIWebView` conforms to `NSCoding` (unlike `UIView`), so archiving a view
// hierarchy that contains one messages this selector. Its state is the loaded
// page and history, which cannot be restored here, so encoding succeeds
// without writing anything.
- (())encodeWithCoder:(id)_coder {}

// =========================================================================
// MARK: - Dealloc
// =========================================================================

- (())dealloc {
    let host = env.objc.borrow::<UIWebViewHostObject>(this);
    let (current_url, back_stack, forward_stack, overlay_id) = (
        host.current_url,
        host.back_stack.clone(),
        host.forward_stack.clone(),
        host.overlay_id,
    );
    if overlay_id >= 0 {
        android_web_view::hide(overlay_id);
    }
    release(env, current_url);
    for url in back_stack    { release(env, url); }
    for url in forward_stack { release(env, url); }
    env.objc.dealloc_object(this, &mut env.mem)
}

// =========================================================================
// MARK: - Delegate
// =========================================================================

- (id)delegate {
    env.objc.borrow::<UIWebViewHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    // Weak reference — do NOT retain.
    env.objc.borrow_mut::<UIWebViewHostObject>(this).delegate = delegate;
}

// =========================================================================
// MARK: - Loading
// =========================================================================

- (())loadRequest:(id)request { // NSURLRequest*
    let url_string: String = if request != nil {
        let url: id = msg![env; request URL];
        let url_desc: id = msg![env; url description];
        if url_desc != nil { to_rust_string(env, url_desc).into_owned() } else { String::new() }
    } else {
        String::new()
    };
    log!("UIWebView loadRequest: {}", url_string);

    // Push current URL onto back stack before navigating.
    let old_url = env.objc.borrow::<UIWebViewHostObject>(this).current_url;
    if old_url != nil {
        retain(env, old_url);
        env.objc.borrow_mut::<UIWebViewHostObject>(this).back_stack.push(old_url);
        // Clear forward stack on new navigation.
        let fwd: Vec<id> = std::mem::take(
            &mut env.objc.borrow_mut::<UIWebViewHostObject>(this).forward_stack
        );
        for u in fwd { release(env, u); }
    }
    release(env, old_url);
    let ns_url = ns_string::from_rust_string(env, url_string.clone());
    {
        let host = env.objc.borrow_mut::<UIWebViewHostObject>(this);
        host.current_url = ns_url;
        host.loading = true;
    }
    fire_did_start_load(env, this);

    // Real engine path: hand the request to a genuine Android WebView
    // overlay; did-finish is scheduled for after the native page loads.
    if android_web_view::native_webview_available() {
        if overlay_load(env, this, Some(&url_string), None) {
            schedule_did_finish_load(env, this);
            return;
        }
        // The bridge is up, but the view has no on-screen extent yet.
        // Remember the load and retry it from -setFrame: once the view
        // is laid out.
        log!(
            "UIWebView: deferring load of {} until the view is laid out ({})",
            url_string,
            deferred_load_context(env, this)
        );
        {
            let host_obj = env.objc.borrow_mut::<UIWebViewHostObject>(this);
            host_obj.pending_load = Some(PendingLoad::Url(url_string));
            host_obj.deferred_polls = 0;
        }
        schedule_did_finish_load(env, this);
        return;
    }
    log!("UIWebView: native WebView bridge not available; using fallback rendering");

    // Desktop fallback: snapshot the URL with headless Chromium and install
    // the PNG as this view's layer contents. We don't stream content, so the
    // load is "finished" as soon as the snapshot is in.
    let frame: CGRect = msg![env; this frame];
    render_url_to_layer(env, this, &url_string, frame);
    finish_load(env, this);
}

- (())loadHTMLString:(id)html     // NSString*
            baseURL:(id)_base_url { // NSURL*
    let html_str = if html != nil {
        to_rust_string(env, html).into_owned()
    } else { String::new() };
    log_dbg!("UIWebView loadHTMLString: ({} chars)", html_str.len());

    if android_web_view::native_webview_available() {
        if overlay_load(env, this, None, Some((&html_str, "text/html"))) {
            schedule_did_finish_load(env, this);
            return;
        }
        // The bridge is up, but the view has no on-screen extent yet.
        log!(
            "UIWebView: deferring HTML load until the view is laid out ({})",
            deferred_load_context(env, this)
        );
        {
            let host_obj = env.objc.borrow_mut::<UIWebViewHostObject>(this);
            host_obj.pending_load = Some(PendingLoad::Data(html_str, "text/html".to_string()));
            host_obj.deferred_polls = 0;
        }
        schedule_did_finish_load(env, this);
        return;
    }
    log!("UIWebView: native WebView bridge not available; using fallback rendering");

    // Desktop fallback: write the HTML to a temp file and snapshot it.
    if !html_str.is_empty() {
        let idx = SNAP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("touchhle_uiwebview_{}.html", idx));
        if std::fs::write(&tmp, html_str.as_bytes()).is_ok() {
            let url = format!("file://{}", tmp.display());
            let frame: CGRect = msg![env; this frame];
            render_url_to_layer(env, this, &url, frame);
            let _ = std::fs::remove_file(&tmp);
        }
    }
    finish_load(env, this);
}

- (())loadData:(id)data            // NSData*
      MIMEType:(id)mime            // NSString*
      textEncodingName:(id)_enc    // NSString*
       baseURL:(id)_base_url {     // NSURL*
    let mime_str = if mime != nil { to_rust_string(env, mime).into_owned() } else { "(null)".into() };
    log_dbg!("UIWebView loadData:MIMEType:{}", mime_str);

    // Read the payload out of the guest NSData.
    let payload = if data != nil {
        let len: NSUInteger = msg![env; data length];
        let bytes_ptr: crate::mem::ConstPtr<u8> = msg![env; data bytes];
        if len > 0 && !bytes_ptr.is_null() {
            let bytes = env.mem.bytes_at(bytes_ptr, len);
            String::from_utf8_lossy(bytes).into_owned()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    if android_web_view::native_webview_available() {
        if overlay_load(env, this, None, Some((&payload, mime_str.as_str()))) {
            schedule_did_finish_load(env, this);
            return;
        }
        // The bridge is up, but the view has no on-screen extent yet.
        log!(
            "UIWebView: deferring data load until the view is laid out ({})",
            deferred_load_context(env, this)
        );
        {
            let host_obj = env.objc.borrow_mut::<UIWebViewHostObject>(this);
            host_obj.pending_load = Some(PendingLoad::Data(payload, mime_str));
            host_obj.deferred_polls = 0;
        }
        schedule_did_finish_load(env, this);
        return;
    }
    log!("UIWebView: native WebView bridge not available; using fallback rendering");

    // Desktop fallback: only HTML payloads can be rendered (temp file route).
    if mime_str.starts_with("text/html") && !payload.is_empty() {
        let idx = SNAP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("touchhle_uiwebview_{}.html", idx));
        if std::fs::write(&tmp, payload.as_bytes()).is_ok() {
            let url = format!("file://{}", tmp.display());
            let frame: CGRect = msg![env; this frame];
            render_url_to_layer(env, this, &url, frame);
            let _ = std::fs::remove_file(&tmp);
        }
    }
    finish_load(env, this);
}

// =========================================================================
// MARK: - Subview management
// =========================================================================

- (())insertSubview:(id)view aboveSubview:(id)sibling {
    if view == nil { return; }

    // If sibling is nil or not in our view hierarchy, just add at the top.
    if sibling == nil {
        let _: () = msg![env; this addSubview:view];
        return;
    }

    // Delegate to UIView's insertSubview:aboveSubview: on our own view.
    let self_view: id = msg![env; this view];
    if self_view != nil {
        let _: () = msg![env; self_view insertSubview:view aboveSubview:sibling];
    } else {
        // Fallback — just add it.
        let _: () = msg![env; this addSubview:view];
    }
}

- (())reload {
    log_dbg!("UIWebView reload");
    let current = env.objc.borrow::<UIWebViewHostObject>(this).current_url;
    if current == nil { return; }
    retain(env, current);

    // Вынесено в отдельную переменную для предотвращения ошибки E0283
    let url: id = msg_class![env; NSURL URLWithString:current];
    let ns_req: id = msg_class![env; NSURLRequest requestWithURL:url];

    let _: () = msg![env; this loadRequest:ns_req];
    release(env, current);
}

- (())stopLoading {
    log_dbg!("UIWebView stopLoading");
    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 {
        android_web_view::stop_loading(overlay_id);
    }
    env.objc.borrow_mut::<UIWebViewHostObject>(this).loading = false;

    let delegate = env.objc.borrow::<UIWebViewHostObject>(this).delegate;
    if delegate != nil {
        let sel = env.objc.register_host_selector(
            "webView:didFailLoadWithError:".to_string(),
            &mut env.mem,
        );
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            let error: id = nil;
            let _: () = msg![env; delegate webView:this didFailLoadWithError:error];
        }
    }
}

- (bool)isLoading {
    env.objc.borrow::<UIWebViewHostObject>(this).loading
}

// =========================================================================
// MARK: - Navigation
// =========================================================================

- (bool)canGoBack {
    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 && android_web_view::can_go_back(overlay_id) {
        return true;
    }
    !env.objc.borrow::<UIWebViewHostObject>(this).back_stack.is_empty()
}

- (bool)canGoForward {
    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 && android_web_view::can_go_forward(overlay_id) {
        return true;
    }
    !env.objc.borrow::<UIWebViewHostObject>(this).forward_stack.is_empty()
}

- (())goBack {
    let back_url = {
        let host = env.objc.borrow_mut::<UIWebViewHostObject>(this);
        host.back_stack.pop()
    };
    let Some(back_url) = back_url else { return; };

    // Push current to forward stack.
    let current = env.objc.borrow::<UIWebViewHostObject>(this).current_url;
    if current != nil {
        retain(env, current);
        env.objc.borrow_mut::<UIWebViewHostObject>(this).forward_stack.push(current);
    }
    release(env, current);

    // Android: the native WebView keeps its own history; just drive it.
    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 && android_web_view::native_webview_available() {
        android_web_view::go_back(overlay_id);
    } else if android_web_view::native_webview_available() {
        let url_str = to_rust_string(env, back_url).into_owned();
        overlay_load(env, this, Some(&url_str), None);
    }
    env.objc.borrow_mut::<UIWebViewHostObject>(this).current_url = back_url;
    env.objc.borrow_mut::<UIWebViewHostObject>(this).loading = false;
    finish_load(env, this);
}

- (())goForward {
    let fwd_url = {
        let host = env.objc.borrow_mut::<UIWebViewHostObject>(this);
        host.forward_stack.pop()
    };
    let Some(fwd_url) = fwd_url else { return; };

    let current = env.objc.borrow::<UIWebViewHostObject>(this).current_url;
    if current != nil {
        retain(env, current);
        env.objc.borrow_mut::<UIWebViewHostObject>(this).back_stack.push(current);
    }
    release(env, current);

    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 && android_web_view::native_webview_available() {
        android_web_view::go_forward(overlay_id);
    } else if android_web_view::native_webview_available() {
        let url_str = to_rust_string(env, fwd_url).into_owned();
        overlay_load(env, this, Some(&url_str), None);
    }
    env.objc.borrow_mut::<UIWebViewHostObject>(this).current_url = fwd_url;
    env.objc.borrow_mut::<UIWebViewHostObject>(this).loading = false;
    finish_load(env, this);
}

// =========================================================================
// MARK: - JavaScript
// =========================================================================

- (id)stringByEvaluatingJavaScriptFromString:(id)script { // NSString* -> NSString*
    let script_str = if script != nil {
        to_rust_string(env, script).into_owned()
    } else { String::new() };
    log_dbg!("UIWebView stringByEvaluatingJavaScriptFromString: {:?}", script_str);

    // Real path: evaluate in the native Android WebView and return its
    // result. Android returns the value JSON-encoded; unwrap plain strings
    // and nulls so guests see something close to UIWebView's conversion.
    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 {
        if let Some(result) = android_web_view::eval_js(overlay_id, &script_str) {
            let unwrapped = unwrap_js_result(&result);
            let s = ns_string::from_rust_string(env, unwrapped);
            return crate::objc::autorelease(env, s);
        }
    }
    // Return empty NSString rather than nil — some apps check the return value.
    let empty = ns_string::from_rust_string(env, String::new());
    crate::objc::autorelease(env, empty)
}

// =========================================================================
// MARK: - Request / URL accessors
// =========================================================================

- (id)request { // NSURLRequest*
    let url = env.objc.borrow::<UIWebViewHostObject>(this).current_url;
    if url == nil {
        return nil;
    }
    let ns_url: id = msg_class![env; NSURL URLWithString:url];
    if ns_url == nil {
        return nil;
    }
    msg_class![env; NSURLRequest requestWithURL:ns_url]
}

// Returns the URL of the currently loaded page as an NSString*.
- (id)_currentURLString { // NSString* (private helper)
    env.objc.borrow::<UIWebViewHostObject>(this).current_url
}

// =========================================================================
// MARK: - Properties
// =========================================================================

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

- (UIDataDetectorTypes)dataDetectorTypes {
    env.objc.borrow::<UIWebViewHostObject>(this).data_detector_types
}
- (())setDataDetectorTypes:(UIDataDetectorTypes)types {
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

- (bool)mediaPlaybackAllowsAirPlay {
    env.objc.borrow::<UIWebViewHostObject>(this).media_playback_allows_air_play
}
- (())setMediaPlaybackAllowsAirPlay:(bool)value {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).media_playback_allows_air_play = value;
}

- (bool)suppressesIncrementalRendering {
    env.objc.borrow::<UIWebViewHostObject>(this).suppress_incremental_rendering
}
- (())setSuppressesIncrementalRendering:(bool)value {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).suppress_incremental_rendering = value;
}

- (bool)keyboardDisplayRequiresUserAction {
    env.objc.borrow::<UIWebViewHostObject>(this).keyboard_display_requires_user_action
}
- (())setKeyboardDisplayRequiresUserAction:(bool)value {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).keyboard_display_requires_user_action = value;
}

// Pagination (iOS 7+)
- (i32)paginationMode { // UIWebPaginationMode
    env.objc.borrow::<UIWebViewHostObject>(this).pagination_mode
}
- (())setPaginationMode:(i32)mode {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).pagination_mode = mode;
}

- (i32)paginationBreakingMode { // UIWebPaginationBreakingMode
    env.objc.borrow::<UIWebViewHostObject>(this).pagination_breaking_mode
}
- (())setPaginationBreakingMode:(i32)mode {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).pagination_breaking_mode = mode;
}

- (f64)pageLength {
    env.objc.borrow::<UIWebViewHostObject>(this).page_length
}
- (())setPageLength:(f64)length {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).page_length = length;
}

- (f64)gapBetweenPages {
    env.objc.borrow::<UIWebViewHostObject>(this).gap_between_pages
}
- (())setGapBetweenPages:(f64)gap {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).gap_between_pages = gap;
}

- (u32)pageCount { // NSUInteger
    // No real rendering — always 0.
    0u32
}

// =========================================================================
// MARK: - Layout
// =========================================================================

- (())setFrame:(CGRect)frame {
    // `() = ` pins the void return type (house style for non-tail super
    // calls, e.g. UITableView's own -setFrame: override).
    () = msg_super![env; this setFrame:frame];
    // A load may have been deferred because the view had no on-screen
    // extent when the app initiated it (apps commonly load the request
    // before the view is laid out). Now that the view has a size, take
    // the deferred load.
    retry_pending_load(env, this);
}

// =========================================================================
// MARK: - View hierarchy (overlay teardown)
// =========================================================================

- (())removeFromSuperview {
    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 {
        android_web_view::hide(overlay_id);
        env.objc.borrow_mut::<UIWebViewHostObject>(this).overlay_id = -1;
    }
    // Mirror UIView's removeFromSuperview: detach from the superview. The
    // layer detaches itself from the superlayer; the superview drops `this`
    // from its `subviews` (which balances the retain taken by addSubview).
    let superview = env
        .objc
        .borrow_mut::<UIWebViewHostObject>(this)
        .superclass
        .superview;
    if superview == nil {
        return;
    }
    let layer = env.objc.borrow::<UIWebViewHostObject>(this).superclass.layer;
    let _: () = msg![env; layer removeFromSuperlayer];
    let removed = {
        let subviews = &mut env
            .objc
            .borrow_mut::<UIViewHostObject>(superview)
            .subviews;
        if let Some(idx) = subviews.iter().position(|&v| v == this) {
            subviews.remove(idx);
            true
        } else {
            false
        }
    };
    if removed {
        release(env, this);
    }
}

// =========================================================================
// MARK: - Scroll view
// =========================================================================

// Returns a stub scroll view so apps that access scrollView don't crash.
- (id)scrollView { // UIScrollView*
    // Return self as a passthrough — we don't have a real UIScrollView here.
    this
}

// =========================================================================
// MARK: - Native-overlay & async-load internals
// =========================================================================

// Called by an NSTimer scheduled in loadRequest:/loadHTMLString:/loadData:.
// The timer argument is the (retained) NSTimer; we release our retain of
// `this` here to keep refcounts balanced.
//
// A deferred load normally retries from -setFrame:, but a view that was
// sized via initWithFrame: and never re-laid-out never receives -setFrame:
// again — so this handler doubles as a poll: while a load stays deferred,
// keep retrying on a timer until the overlay can be shown (or we give up
// and simply report the load as finished).
- (())touchhleWebViewLoadDidFinish:(id)_timer {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).loading = false;
    let mut poll_again = false;
    if env.objc.borrow::<UIWebViewHostObject>(this).pending_load.is_some() {
        retry_pending_load(env, this);
        if env.objc.borrow::<UIWebViewHostObject>(this).pending_load.is_some() {
            let frame: CGRect = msg![env; this frame];
            // CGRect is #[repr(packed)]: taking a reference to a field is
            // unaligned (E0793), so copy the values out for the log.
            let frame_w = frame.size.width;
            let frame_h = frame.size.height;
            let host_obj = env.objc.borrow_mut::<UIWebViewHostObject>(this);
            host_obj.deferred_polls += 1;
            if host_obj.deferred_polls == 1 {
                // Make the poll visible in the log: without this line a
                // healthy poll cycle is indistinguishable from a dead one
                // while the view still has no frame.
                log!(
                    "UIWebView: deferred load still waiting (frame {}x{}); polling on a timer",
                    frame_w,
                    frame_h
                );
            }
            if host_obj.deferred_polls < 100 {
                poll_again = true;
            } else {
                log!(
                    "UIWebView: giving up on deferred load (view never got a frame)"
                );
                host_obj.pending_load = None;
            }
        }
    }
    if poll_again {
        // Schedule_did_finish_load re-retains `this`, balancing the
        // release below; the delegate's webViewDidFinishLoad: fires only
        // once, after the overlay is really up (or on give-up).
        schedule_did_finish_load(env, this);
    } else {
        fire_did_finish_load(env, this);
    }
    release(env, this);
}



// =========================================================================
// MARK: - Description
// =========================================================================

- (id)description {
    let (loading, current_url) = {
        let h = env.objc.borrow::<UIWebViewHostObject>(this);
        (h.loading, h.current_url)
    };
    let url_str = if current_url != nil {
        to_rust_string(env, current_url).into_owned()
    } else { "(nil)".into() };
    let s = format!(
        "<UIWebView: {:?}; loading={}; url={}>",
        this, loading, url_str
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

};

// =========================================================================
// MARK: - Chromium/CDP bridge: render a URL into the view's layer.contents
// =========================================================================
//
// touchHLE has no HTML rendering engine. As an opportunistic fallback (see
// PR description) we shell out to the host's headless Chromium to rasterise
// the target URL into a PNG, then install that PNG as the CALayer contents
// for the UIWebView. This gives apps like Google Mobile a visible web page
// instead of a blank rectangle, at the cost of interactivity.

/// Counter used for unique temp filenames.
static SNAP_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Find a headless-capable Chromium binary on the host. Returns `None` when
/// no suitable browser is available — in that case we leave the layer blank.
fn find_chromium_binary() -> Option<PathBuf> {
    // Allow env var override for advanced users / CI.
    if let Ok(path) = std::env::var("TOUCHHLE_CHROMIUM") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    let candidates = [
        "/opt/.devin/chrome/chrome/linux-137.0.7118.2/chrome-linux64/chrome",
        "/opt/.devin/playwright_browsers/chromium-1097/chrome-linux/chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/google-chrome-stable",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Shell out to headless Chromium to snapshot `url` at `width x height` and
/// return the resulting PNG bytes. Returns `None` on any failure; callers
/// are expected to treat that as "leave the layer blank".
fn snapshot_url_with_chromium(url: &str, width: u32, height: u32) -> Option<Vec<u8>> {
    let Some(chrome) = find_chromium_binary() else {
        log!("UIWebView bridge: no Chromium binary found; leaving layer blank");
        return None;
    };
    let idx = SNAP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("touchhle_uiwebview_{}.png", idx));
    // Chromium refuses to run as root unless given --no-sandbox.
    let status = Command::new(&chrome)
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--hide-scrollbars")
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .arg(format!("--window-size={},{}", width, height))
        .arg(format!("--screenshot={}", tmp.display()))
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            log!("UIWebView bridge: chromium exited with {}", s);
        }
        Err(e) => {
            log!("UIWebView bridge: failed to spawn chromium: {}", e);
            return None;
        }
    }
    let bytes = std::fs::read(&tmp).ok();
    let _ = std::fs::remove_file(&tmp);
    bytes
}

/// Snapshot `url` and install the decoded PNG as the UIWebView's
/// `layer.contents` so the user sees the rendered web page.
fn render_url_to_layer(env: &mut Environment, this: id, url: &str, frame: CGRect) {
    if cfg!(target_os = "android") {
        // There is no desktop Chromium to shell out to on a phone: the
        // native WebView overlay is the only real rendering path there.
        // (When the overlay is merely deferred we never get here either.)
        return;
    }
    if url.is_empty()
        || !(url.starts_with("http://")
            || url.starts_with("https://")
            || url.starts_with("file://"))
    {
        return;
    }
    let width = (frame.size.width.max(1.0) as u32).max(1);
    let height = (frame.size.height.max(1.0) as u32).max(1);
    let Some(png) = snapshot_url_with_chromium(url, width, height) else {
        return;
    };
    let Ok(image) = Image::from_bytes(&png) else {
        log!("UIWebView bridge: could not decode PNG snapshot");
        return;
    };
    let cg_image = cg_image::from_image(env, image);
    let layer: id = msg![env; this layer];
    let _: () = msg![env; layer setContents:cg_image];
    let _: () = msg![env; this setNeedsDisplay];
    cg_image::CGImageRelease(env, cg_image);
}

// =========================================================================
// MARK: - Native overlay plumbing
// =========================================================================

/// Hide (and forget) the native overlay backing `this`, if any. Called from
/// `UIView`'s `removeFromSuperview` when the removed view is a UIWebView.
pub(crate) fn webview_did_remove_from_superview(env: &mut Environment, this: id) {
    let overlay_id = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if overlay_id >= 0 {
        android_web_view::hide(overlay_id);
        env.objc.borrow_mut::<UIWebViewHostObject>(this).overlay_id = -1;
    }
}

/// Map the webview's guest frame to host window pixels and create/position
/// the native overlay. Returns the overlay id, or `None` if the view has no
/// on-screen extent (or native overlays are unavailable).
fn overlay_show_or_update(env: &mut Environment, this: id) -> Option<i32> {
    if !android_web_view::native_webview_available() {
        return None;
    }
    let frame: CGRect = msg![env; this frame];
    let (mut x, mut y, mut w, mut h) = env.window().guest_frame_to_window_px(frame);
    if w <= 0 || h <= 0 {
        // The view itself has no extent yet. Apps commonly add the web
        // view to a container and let autoresizing size it — which never
        // happens here, as touchHLE has no automatic layout pass. If we
        // already have a superview with a usable frame, borrow its
        // geometry instead of staying blank.
        let superview: id = msg![env; this superview];
        if superview != nil {
            let super_frame: CGRect = msg![env; superview frame];
            let (sx, sy, sw, sh) = env.window().guest_frame_to_window_px(super_frame);
            if sw > 0 && sh > 0 {
                log!("UIWebView overlay: view has no extent yet; using superview frame");
                x = sx;
                y = sy;
                w = sw;
                h = sh;
            }
        }
    }
    if w <= 0 || h <= 0 {
        log!("UIWebView overlay skipped: view has no on-screen extent yet");
        return None;
    }
    let current = env.objc.borrow::<UIWebViewHostObject>(this).overlay_id;
    if current == -2 {
        // First show: create the overlay seeded with a blank page; the
        // caller immediately navigates or loads data into it.
        let oid = android_web_view::show("about:blank", x, y, w, h);
        env.objc.borrow_mut::<UIWebViewHostObject>(this).overlay_id = oid;
        Some(oid)
    } else if current >= 0 {
        android_web_view::set_bounds(current, x, y, w, h);
        Some(current)
    } else {
        None
    }
}

/// Carry out a load that was deferred because the view had no on-screen
/// extent when the app initiated it. Called from `-setFrame:`; keeps the
/// load deferred if the view still can't be shown.
fn retry_pending_load(env: &mut Environment, this: id) {
    if env.objc.borrow::<UIWebViewHostObject>(this).pending_load.is_none() {
        return;
    }
    if !android_web_view::native_webview_available() {
        return;
    }
    let frame: CGRect = msg![env; this frame];
    if frame.size.width <= 0.0 || frame.size.height <= 0.0 {
        // The view itself has no size yet. The overlay can still be shown
        // from the superview's geometry (see overlay_show_or_update), so
        // only keep waiting when there is no usable superview either.
        let superview: id = msg![env; this superview];
        if superview == nil {
            return;
        }
        let super_frame: CGRect = msg![env; superview frame];
        if super_frame.size.width <= 0.0 || super_frame.size.height <= 0.0 {
            return;
        }
    }
    let pending = env
        .objc
        .borrow_mut::<UIWebViewHostObject>(this)
        .pending_load
        .take()
        .unwrap();
    log!("UIWebView: retrying deferred load now that the view is laid out");
    let shown = match &pending {
        PendingLoad::Url(url) => overlay_load(env, this, Some(url), None),
        PendingLoad::Data(payload, mime) => overlay_load(env, this, None, Some((payload, mime))),
    };
    if !shown {
        // The overlay still couldn't be shown (e.g. the host window
        // viewport isn't ready); keep the load deferred.
        env.objc.borrow_mut::<UIWebViewHostObject>(this).pending_load = Some(pending);
    }
}

/// One-line description of a deferred webview's geometry: its own frame
/// and, if it has one, its superview's frame. Included in the deferral log
/// so that a single line shows whether the superview fallback (see
/// `overlay_show_or_update`) will be able to size the overlay — and so the
/// line itself identifies builds that carry this diagnostic.
fn deferred_load_context(env: &mut Environment, this: id) -> String {
    let frame: CGRect = msg![env; this frame];
    // CGRect is #[repr(packed)]: references to its fields in format args
    // would be unaligned (E0793), so copy the values out first.
    let frame_w = frame.size.width;
    let frame_h = frame.size.height;
    let superview: id = msg![env; this superview];
    if superview == nil {
        return format!("frame {}x{}, no superview yet", frame_w, frame_h);
    }
    let super_frame: CGRect = msg![env; superview frame];
    let super_w = super_frame.size.width;
    let super_h = super_frame.size.height;
    format!(
        "frame {}x{}, superview {:?} {}x{}",
        frame_w, frame_h, superview, super_w, super_h
    )
}

/// Retry deferred loads for every UIWebView in `root`'s view subtree.
/// Called when a freshly built hierarchy has just been attached to a window
/// (see `-[UIViewController presentModalViewController:animated:]`): at that
/// point the containers the app created its webviews in finally exist, so a
/// load deferred because the webview had no extent can often proceed right
/// away — synchronously, without depending on the NSTimer poll.
pub(crate) fn retry_pending_loads_in_subtree(env: &mut Environment, root: id) {
    if root == nil {
        return;
    }
    let uiwebview_class: Class = msg_class![env; UIWebView class];
    let is_webview: bool = msg![env; root isKindOfClass:uiwebview_class];
    if is_webview
        && env
            .objc
            .borrow::<UIWebViewHostObject>(root)
            .pending_load
            .is_some()
    {
        log!(
            "UIWebView: view hierarchy is in place; retrying deferred load of {:?}",
            root
        );
        retry_pending_load(env, root);
        if env
            .objc
            .borrow::<UIWebViewHostObject>(root)
            .pending_load
            .is_none()
            && cfg!(target_os = "android")
        {
            // The retry just showed the overlay, which completes the load.
            // The NSTimer that would normally deliver webViewDidFinishLoad:
            // has never been observed to fire on Android, so finish the
            // load here instead. (If the timer ever does fire as well, a
            // second webViewDidFinishLoad: is harmless — real UIWebView
            // delivers it once per frame navigation.)
            finish_load(env, root);
        }
    }
    let subviews: id = msg![env; root subviews];
    if subviews != nil {
        let count: NSUInteger = msg![env; subviews count];
        let mut i: NSUInteger = 0;
        while i < count {
            let child: id = msg![env; subviews objectAtIndex:i];
            retry_pending_loads_in_subtree(env, child);
            i += 1;
        }
    }
}

/// Fire `webViewDidFinishLoad:` on the delegate (if it responds).
fn fire_did_finish_load(env: &mut Environment, this: id) {
    let delegate = env.objc.borrow::<UIWebViewHostObject>(this).delegate;
    if delegate != nil {
        let sel = env.objc.register_host_selector(
            "webViewDidFinishLoad:".to_string(),
            &mut env.mem,
        );
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            let _: () = msg![env; delegate webViewDidFinishLoad:this];
        }
    }
}

/// Fire `webViewDidStartLoad:` on the delegate (if it responds).
fn fire_did_start_load(env: &mut Environment, this: id) {
    let delegate = env.objc.borrow::<UIWebViewHostObject>(this).delegate;
    if delegate != nil {
        let sel = env.objc.register_host_selector(
            "webViewDidStartLoad:".to_string(),
            &mut env.mem,
        );
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            let _: () = msg![env; delegate webViewDidStartLoad:this];
        }
    }
}

/// Schedule the async `webViewDidFinishLoad:` callback. On Android a real
/// page load is in flight, so we delay briefly (matching real-UIWebView
/// timing); the timer holds a retain of `this` which is released in
/// `touchhleWebViewLoadDidFinish:`.
fn schedule_did_finish_load(env: &mut Environment, this: id) {
    retain(env, this);
    let sel = env
        .objc
        .register_host_selector("touchhleWebViewLoadDidFinish:".to_string(), &mut env.mem);
    let _timer: id = msg_class![env;
        NSTimer scheduledTimerWithTimeInterval:0.6
        target:this
        selector:sel
        userInfo:nil
        repeats:false
    ];
}

/// Show (or update) the native overlay and load `url` / `data` into it.
/// `url` is `Some` for URL loads; `data` is `(payload, mime)` for data loads.
/// Returns `true` when the native path was taken.
fn overlay_load(
    env: &mut Environment,
    this: id,
    url: Option<&str>,
    data: Option<(&str, &str)>,
) -> bool {
    let Some(overlay) = overlay_show_or_update(env, this) else {
        return false;
    };
    if let Some((payload, mime)) = data {
        android_web_view::load_data(overlay, payload, mime);
    } else if let Some(url) = url {
        if url.is_empty() {
            android_web_view::load_data(overlay, "<html><body></body></html>", "text/html");
        } else {
            android_web_view::navigate(overlay, url);
        }
    }
    true
}

/// Mark the load as finished and fire `webViewDidFinishLoad:`.
fn finish_load(env: &mut Environment, this: id) {
    env.objc.borrow_mut::<UIWebViewHostObject>(this).loading = false;
    fire_did_finish_load(env, this);
}

/// Android's `evaluateJavascript` returns a JSON-encoded value; unwrap the
/// common cases (quoted string, null, bare literals) into a plain string.
fn unwrap_js_result(result: &str) -> String {
    let t = result.trim();
    if t == "null" {
        return String::new();
    }
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        let inner = &t[1..t.len() - 1];
        return inner
            .replace("\\\\", "\u{1}")
            .replace("\\n", "\n")
            .replace("\\r", "\r")
            .replace("\\t", "\t")
            .replace("\\\"","\"")
            .replace('\u{1}', "\\");
    }
    t.to_string()
}
