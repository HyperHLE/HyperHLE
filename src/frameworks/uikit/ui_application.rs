/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIApplication` and `UIApplicationMain`.

use super::ui_device::*;
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::core_graphics::CGRect;
use crate::frameworks::foundation::ns_string::{from_rust_string, get_static_str};
use crate::frameworks::foundation::{ns_array, ns_string, NSInteger, NSUInteger};
use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr, SEL,
};
use crate::window::DeviceOrientation;
use crate::{todo_objc_setter, Environment};

#[derive(Default)]
pub struct State {
    /// [UIApplication sharedApplication]
    shared_application: Option<id>,
    pub(super) status_bar_hidden: bool,
}

struct UIApplicationHostObject {
    delegate: id,
    delegate_is_retained: bool,
}
impl HostObject for UIApplicationHostObject {}

pub type UIInterfaceOrientation = UIDeviceOrientation;
#[allow(unused)]
pub const UIInterfaceOrientationPortrait: UIInterfaceOrientation = UIDeviceOrientationPortrait;
#[allow(unused)]
pub const UIInterfaceOrientationPortraitUpsideDown: UIInterfaceOrientation =
    UIDeviceOrientationPortraitUpsideDown;
// These are intentionally swapped and documented as such (the UI on the device
// rotates in the opposite direction to how the device is rotated).
pub const UIInterfaceOrientationLandscapeLeft: UIInterfaceOrientation =
    UIDeviceOrientationLandscapeRight;
pub const UIInterfaceOrientationLandscapeRight: UIInterfaceOrientation =
    UIDeviceOrientationLandscapeLeft;

type UIRemoteNotificationType = NSUInteger;
type UIStatusBarAnimation = NSInteger;
type UIStatusBarStyle = NSInteger;
pub type UIApplicationState = NSInteger;
pub const UIApplicationStateActive:     UIApplicationState = 0;
pub const UIApplicationStateInactive:   UIApplicationState = 1;
pub const UIApplicationStateBackground: UIApplicationState = 2;


pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIApplication: UIResponder

// This should only be called by UIApplicationMain
+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIApplicationHostObject {
        delegate: nil,
        delegate_is_retained: false,
    });
    env.objc.alloc_static_object(this, host_object, &mut env.mem)
}

+ (id)sharedApplication {
    env.framework_state.uikit.ui_application.shared_application.unwrap_or(nil)
}

- (())setNetworkActivityIndicatorVisible:(bool)visible {
    // touchHLE doesn't render the iOS status bar, so we just stub this
    // and ignore the request to show/hide the spinner.
    log_dbg!("Stubbed setNetworkActivityIndicatorVisible: {}", visible);
}

- (bool)isNetworkActivityIndicatorVisible {
    // Always report that it's hidden.
    false
}

// This should only be called by UIApplicationMain
- (id)init {
    assert!(env.framework_state.uikit.ui_application.shared_application.is_none());
    env.framework_state.uikit.ui_application.shared_application = Some(this);
    this
}

// This is a singleton, it shouldn't be deallocated.
- (id)retain { this }
- (id)autorelease { this }
- (())release {}

- (id)delegate {
    env.objc.borrow::<UIApplicationHostObject>(this).delegate
}
- (())setDelegate:(id)delegate { // something implementing UIApplicationDelegate
    let host_object = env.objc.borrow_mut::<UIApplicationHostObject>(this);
    // This property is quasi-non-retaining: https://stackoverflow.com/a/14271150/736162
    let old_delegate = std::mem::replace(&mut host_object.delegate, delegate);
    if host_object.delegate_is_retained {
        host_object.delegate_is_retained = false;
        if delegate != old_delegate {
            release(env, old_delegate);
        }
    }
}

- (bool)isStatusBarHidden {
    env.framework_state.uikit.ui_application.status_bar_hidden
}
- (())setStatusBarHidden:(bool)hidden {
    env.framework_state.uikit.ui_application.status_bar_hidden = hidden;
}
- (())setStatusBarHidden:(bool)hidden
                animated:(bool)_animated {
    // TODO: animation
    msg![env; this setStatusBarHidden:hidden]
}
- (())setStatusBarHidden:(bool)hidden
           withAnimation:(UIStatusBarAnimation)_animation {
    // TODO: animation
    msg![env; this setStatusBarHidden:hidden]
}

- (())setStatusBarStyle:(UIStatusBarStyle)style {
    todo_objc_setter!(this, style);
}

- (())setStatusBarStyle:(UIStatusBarStyle)style
               animated:(bool)_animated {
    msg![env; this setStatusBarStyle:style]
}

- (UIInterfaceOrientation)statusBarOrientation {
    match env.window().current_rotation() {
        DeviceOrientation::Portrait => UIDeviceOrientationPortrait,
        DeviceOrientation::LandscapeLeft => UIDeviceOrientationLandscapeLeft,
        DeviceOrientation::LandscapeRight => UIDeviceOrientationLandscapeRight
    }
}

- (f64)statusBarOrientationAnimationDuration {
    0.3
}

- (())setStatusBarOrientation:(UIInterfaceOrientation)orientation {
    env.on_parent_stack_in_coroutine(|window, _| {window.rotate_device(match orientation {
        UIDeviceOrientationPortrait => DeviceOrientation::Portrait,
        UIDeviceOrientationLandscapeLeft => DeviceOrientation::LandscapeLeft,
        UIDeviceOrientationLandscapeRight => DeviceOrientation::LandscapeRight,
        _ => unimplemented!("Orientation {} not handled yet", orientation),
    })});
}
- (())setStatusBarOrientation:(UIInterfaceOrientation)orientation
                     animated:(bool)_animated {
    // TODO: animation
    msg![env; this setStatusBarOrientation:orientation]
}

- (bool)isIdleTimerDisabled {
    !env.window().is_screen_saver_enabled()
}
- (())setIdleTimerDisabled:(bool)disabled {
    env.on_parent_stack_in_coroutine(|window, _| window.set_screen_saver_enabled(!disabled))
}

- (bool)canOpenURL:(id)_url { // NSURL
    log!("TODO: stubbed canOpenURL:");
    false
}

- (bool)openURL:(id)url { // NSURL
    let ns_string = msg![env; url absoluteString];
    let url_string = ns_string::to_rust_string(env, ns_string);
    if let Err(e) = crate::window::open_url(env, &url_string) {
        echo!("App opened URL {:?} unsuccessfully ({}), exiting.", url_string, e);
    } else {
        echo!("App opened URL {:?}, exiting.", url_string);
    }

    exit(env);
    true
}

-(())beginIgnoringInteractionEvents {
    log!("TODO: ignoring beginIgnoringInteractionEvents");
}
- (bool)isIgnoringInteractionEvents {
    false
}
-(())endIgnoringInteractionEvents {
    log!("TODO: ignoring endIgnoringInteractionEvents");
}

- (())sendEvent:(id)event { // UIEvent*
    log_dbg!("UIApplication sendEvent: forwarding to key window");
    let window: id = msg![env; this keyWindow];
    if window != nil {
        msg![env; window sendEvent:event]
    }
}

- (bool)sendAction:(SEL)action
                to:(id)target
              from:(id)sender
          forEvent:(id)event { // UIEvent*
    if target != nil {
        let responds: bool = msg![env; target respondsToSelector:action];
        if responds {
            () = msg![env; target performSelector:action withObject:sender];
            return true;
        }
        return false;
    }
    // Walk responder chain if target is nil.
    let mut responder: id = sender;
    while responder != nil {
        let responds: bool = msg![env; responder respondsToSelector:action];
        if responds {
            () = msg![env; responder performSelector:action withObject:sender];
            return true;
        }
        responder = msg![env; responder nextResponder];
    }
    false
}

- (())beginBackgroundTaskWithExpirationHandler:(id)_handler {
    log!("UIApplication beginBackgroundTaskWithExpirationHandler: stubbed");
}

- (())endBackgroundTask:(NSUInteger)_task {
    log!("UIApplication endBackgroundTask: stubbed");
}

- (NSUInteger)backgroundTimeRemaining {
    // Report effectively infinite time remaining.
    NSUInteger::MAX
}

- (UIApplicationState)applicationState {
    // Always report active.
    UIApplicationStateActive
}

- (bool)isProtectedDataAvailable {
    true
}

- (())setMinimumBackgroundFetchInterval:(f64)_interval {
    log!("UIApplication setMinimumBackgroundFetchInterval: stubbed");
}

- (())registerForRemoteNotifications {
    log!("UIApplication registerForRemoteNotifications: stubbed");
}

- (())unregisterForRemoteNotifications {
    log!("UIApplication unregisterForRemoteNotifications: stubbed");
}

- (bool)isRegisteredForRemoteNotifications {
    false
}

- (())registerUserNotificationSettings:(id)_settings {
    log!("UIApplication registerUserNotificationSettings: stubbed");
}

- (id)currentUserNotificationSettings {
    nil
}

- (())cancelAllLocalNotifications {
    log!("UIApplication cancelAllLocalNotifications: stubbed");
}

- (())cancelLocalNotification:(id)_notification {
    log!("UIApplication cancelLocalNotification: stubbed");
}

- (())scheduleLocalNotification:(id)_notification {
    log!("UIApplication scheduleLocalNotification: stubbed");
}

- (id)scheduledLocalNotifications {
    msg_class![env; NSArray new]
}

- (())setScheduledLocalNotifications:(id)_notifications {
    log!("UIApplication setScheduledLocalNotifications: stubbed");
}

- (bool)supportsShakeToEdit {
    false
}

- (())setSupportsShakeToEdit:(bool)_value {
    // Stub.
}

- (())clearKeychainIfNecessary {
    // Stub.
}

- (CGRect)statusBarFrame {
    // Report a zero-height status bar since we don't render one.
    CGRect {
        origin: crate::frameworks::core_graphics::CGPoint { x: 0.0, y: 0.0 },
        size: crate::frameworks::core_graphics::CGSize { width: 320.0, height: 0.0 },
    }
}

- (())presentLocalNotificationNow:(id)_notification {
    log!("UIApplication presentLocalNotificationNow: stubbed");
}

- (id)keyWindow {
    let Some(key_window) = env
        .framework_state
        .uikit
        .ui_view
        .ui_window
        .key_window else {
        return nil;
    };
    assert!(env
        .framework_state
        .uikit
        .ui_view
        .ui_window
        .windows
        .contains(&key_window));
    key_window
}

- (id)windows {
    let windows: Vec<id> = (*env
        .framework_state
        .uikit
        .ui_view
        .ui_window
        .windows).to_vec();
    for window in &windows {
        retain(env, *window);
    }
    let windows = ns_array::from_vec(env, windows);
    autorelease(env, windows)
}

- (())registerForRemoteNotificationTypes:(UIRemoteNotificationType)types {
    log!("TODO: ignoring registerForRemoteNotificationTypes:{}", types);
}

- (NSInteger)applicationIconBadgeNumber {
    0 // default value
}
- (())setApplicationIconBadgeNumber:(NSInteger)bn {
    log!("TODO: ignoring setApplicationIconBadgeNumber:{}", bn);
}

- (id)nextResponder {
    let delegate = msg![env; this delegate];
    let app_delegate_class = msg![env; delegate class];
    let ui_responder_class = env.objc.get_known_class("UIResponder", &mut env.mem);
    if env.objc.class_is_subclass_of(app_delegate_class, ui_responder_class) {
        delegate
    } else {
        nil
    }
}

@end

};

/// `UIApplicationMain`, the entry point of the application.
pub(super) fn UIApplicationMain(
    env: &mut Environment,
    _argc: i32,
    _argv: MutPtr<MutPtr<u8>>,
    principal_class_name: id, // NSString*
    delegate_class_name: id,  // NSString*
) {
    let ui_application = {
        let pool: id = msg_class![env; NSAutoreleasePool new];

        let principal_class = if principal_class_name != nil {
            let name = ns_string::to_rust_string(env, principal_class_name);
            env.objc.get_known_class(&name, &mut env.mem)
        } else {
            env.objc.get_known_class("UIApplication", &mut env.mem)
        };
        let ui_application: id = msg![env; principal_class new];

        let device_family = env.options.device_family;

        if let Some(main_nib_filename) = env
            .bundle
            .main_nib_filename(device_family)
            .map(str::to_owned)
        {
            let ns_main_nib_filename = from_rust_string(env, main_nib_filename);
            let type_: id = get_static_str(env, "nib");
            let bundle: id = msg_class![env; NSBundle mainBundle];
            let res: id = msg![env; bundle pathForResource:ns_main_nib_filename ofType:type_];
            if res != nil {
                let nib: id = msg_class![env; UINib nibWithNibName:ns_main_nib_filename bundle:nil];
                release(env, ns_main_nib_filename);
                let _: id = msg![env; nib instantiateWithOwner:ui_application
                                               options:nil];
            } else {
                log!("Warning: couldn't load main nib file.");
            }
        }

        if env.bundle.status_bar_hidden() {
            let _: () = msg![env; ui_application setStatusBarHidden:true];
        }

        let delegate: id = msg![env; ui_application delegate];
        if delegate != nil {
            env.objc
                .borrow_mut::<UIApplicationHostObject>(ui_application)
                .delegate_is_retained = true;
            retain(env, delegate);
        } else {
            assert!(delegate_class_name != nil);
            if msg![env; delegate_class_name isEqual:principal_class_name] {
                let _: () = msg![env; ui_application setDelegate:ui_application];
            } else {
                let name = ns_string::to_rust_string(env, delegate_class_name);
                let class = env.objc.get_known_class(&name, &mut env.mem);
                let delegate: id = msg![env; class new];
                let _: () = msg![env; ui_application setDelegate:delegate];
            }
        };

        let _: () = msg![env; pool drain];
        ui_application
    };

    {
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let delegate: id = msg![env; ui_application delegate];
        if env.objc.object_has_method_named(&env.mem, delegate, "application:didFinishLaunchingWithOptions:") {
            let empty_dict: id = msg_class![env; NSDictionary dictionary];
            () = msg![env; delegate application:ui_application didFinishLaunchingWithOptions:empty_dict];
        } else if env.objc.object_has_method_named(&env.mem, delegate, "applicationDidFinishLaunching:") {
            () = msg![env; delegate applicationDidFinishLaunching:ui_application];
        }

        let center: id = msg_class![env; NSNotificationCenter defaultCenter];
        let notif_name = get_static_str(env, UIApplicationDidFinishLaunchingNotification);
        () = msg![env; center postNotificationName:notif_name object:ui_application userInfo:nil];

        let _: () = msg![env; pool drain];
    }

    let views = env.framework_state.uikit.ui_view.views.clone();
    for view in views {
        () = msg![env; view layoutSubviews];
    }

    {
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let delegate: id = msg![env; ui_application delegate];
        if env.objc.object_has_method_named(&env.mem, delegate, "applicationDidBecomeActive:") {
            () = msg![env; delegate applicationDidBecomeActive:ui_application];
        }
        let center: id = msg_class![env; NSNotificationCenter defaultCenter];
        let notif_name = get_static_str(env, UIApplicationDidBecomeActiveNotification);
        () = msg![env; center postNotificationName:notif_name object:ui_application userInfo:nil];
        let _: () = msg![env; pool drain];
    }

    let run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
    let _: () = msg![env; run_loop run];
}

pub(super) fn exit(env: &mut Environment) {
    let ui_application: id = msg_class![env; UIApplication sharedApplication];
    let center: id = msg_class![env; NSNotificationCenter defaultCenter];

    {
        let pool: id = msg_class![env; NSAutoreleasePool new];
        if !env.is_app_picker {
            let user_defaults: id = msg_class![env; NSUserDefaults standardUserDefaults];
            let _: bool = msg![env; user_defaults synchronize];
        }
        let delegate: id = msg![env; ui_application delegate];
        if env.objc.object_has_method_named(&env.mem, delegate, "applicationWillResignActive:") {
            () = msg![env; delegate applicationWillResignActive:ui_application];
        }
        let notif_name = get_static_str(env, UIApplicationWillResignActiveNotification);
        () = msg![env; center postNotificationName:notif_name object:ui_application userInfo:nil];
        let _: () = msg![env; pool drain];
    };

    {
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let delegate: id = msg![env; ui_application delegate];
        if env.objc.object_has_method_named(&env.mem, delegate, "applicationWillTerminate:") {
            () = msg![env; delegate applicationWillTerminate:ui_application];
        }
        let notif_name = get_static_str(env, UIApplicationWillTerminateNotification);
        () = msg![env; center postNotificationName:notif_name object:ui_application userInfo:nil];
        let _: () = msg![env; pool drain];
    };

    std::process::exit(0);
}

const UIApplicationDidFinishLaunchingNotification: &str = "UIApplicationDidFinishLaunchingNotification";
const UIApplicationDidBecomeActiveNotification: &str = "UIApplicationDidBecomeActiveNotification";
const UIApplicationDidEnterBackgroundNotification: &str = "UIApplicationDidEnterBackgroundNotification";
const UIApplicationWillEnterForegroundNotification: &str = "UIApplicationWillEnterForegroundNotification";
const UIApplicationWillResignActiveNotification: &str = "UIApplicationWillResignActiveNotification";
const UIApplicationWillTerminateNotification: &str = "UIApplicationWillTerminateNotification";
const UIApplicationLaunchOptionsRemoteNotificationKey: &str = "UIApplicationLaunchOptionsRemoteNotificationKey";
const UIApplicationDidReceiveMemoryWarningNotification: &str = "UIApplicationDidReceiveMemoryWarningNotification";

pub const CONSTANTS: ConstantExports = &[
    ("_UIApplicationDidFinishLaunchingNotification", HostConstant::NSString(UIApplicationDidFinishLaunchingNotification)),
    ("_UIApplicationDidBecomeActiveNotification", HostConstant::NSString(UIApplicationDidBecomeActiveNotification)),
    ("_UIApplicationDidEnterBackgroundNotification", HostConstant::NSString(UIApplicationDidEnterBackgroundNotification)),
    ("_UIApplicationWillEnterForegroundNotification", HostConstant::NSString(UIApplicationWillEnterForegroundNotification)),
    ("_UIApplicationWillResignActiveNotification", HostConstant::NSString(UIApplicationWillResignActiveNotification)),
    ("_UIApplicationWillTerminateNotification", HostConstant::NSString(UIApplicationWillTerminateNotification)),
    ("_UIApplicationDidReceiveMemoryWarningNotification", HostConstant::NSString(UIApplicationDidReceiveMemoryWarningNotification)),
    ("_UIApplicationLaunchOptionsRemoteNotificationKey", HostConstant::NSString(UIApplicationLaunchOptionsRemoteNotificationKey)),
];

pub const FUNCTIONS: FunctionExports = &[export_c_func!(UIApplicationMain(_, _, _, _))];

