/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `GLKit.framework/GLKit`.
//!
//! Implements `GLKViewController` so apps like Binding of Isaac (which drive
//! their render loop via `GLKViewControllerDelegate`) can run.
//!
//! ## How GLKViewController works
//!
//! On each CADisplayLink tick the controller calls:
//!   1. `[delegate glkViewControllerUpdate:self]`  — game logic
//!   2. `[delegate glkView:self drawInRect:rect]`  — rendering
//!
//! In touchHLE, CADisplayLink is backed by NSTimer, which the run loop fires
//! every frame already. We create it in `-viewDidAppear:` and point it at
//! `-_glkStep:` on self.

use crate::dyld::{ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::foundation::NSInteger;
use crate::impl_HostObject_with_superclass;
use crate::mem::{ConstVoidPtr, SafeRead};
use crate::objc::{
    id, msg, msg_class, msg_super, nil, objc_classes, retain, release,
    ClassExports, HostObject, NSZonePtr,
};
use crate::frameworks::uikit::ui_view_controller::UIViewControllerHostObject;
use crate::Environment;

// ---------------------------------------------------------------------------
// GLKMatrix4Identity constant
// ---------------------------------------------------------------------------

#[repr(C)]
struct GLKMatrix4 {
    m: [f32; 16],
}
unsafe impl SafeRead for GLKMatrix4 {}

fn glk_matrix4_identity(env: &mut Environment) -> ConstVoidPtr {
    let identity = GLKMatrix4 {
        m: [
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ],
    };
    env.mem.alloc_and_write(identity).cast_void().cast_const()
}

// ---------------------------------------------------------------------------
// GLKViewControllerHostObject
// ---------------------------------------------------------------------------

pub(crate) struct GLKViewControllerHostObject {
    pub superclass: UIViewControllerHostObject,
    /// Weak (non-retained) reference to delegate, matching Apple's `weak` property.
    pub delegate: id,
    pub preferred_frames_per_second: NSInteger,
    pub paused: bool,
    /// The CADisplayLink we created; retained so it stays alive.
    pub display_link: id,
}

impl Default for GLKViewControllerHostObject {
    fn default() -> Self {
        GLKViewControllerHostObject {
            superclass: Default::default(),
            delegate: nil,
            preferred_frames_per_second: 30,
            paused: false,
            display_link: nil,
        }
    }
}

impl_HostObject_with_superclass!(GLKViewControllerHostObject);

// ---------------------------------------------------------------------------
// ObjC classes
// ---------------------------------------------------------------------------

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// =========================================================================
// GLKViewController : UIViewController
// =========================================================================

@implementation GLKViewController: UIViewController

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<GLKViewControllerHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    let _: id = msg![env; this initWithNibName:nil bundle:nil];
    this
}

// ---- delegate (weak, not retained) --------------------------------------

- (id)delegate {
    env.objc.borrow::<GLKViewControllerHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<GLKViewControllerHostObject>(this).delegate = delegate;
}

// ---- preferredFramesPerSecond -------------------------------------------

- (NSInteger)preferredFramesPerSecond {
    env.objc.borrow::<GLKViewControllerHostObject>(this).preferred_frames_per_second
}
- (())setPreferredFramesPerSecond:(NSInteger)fps {
    env.objc.borrow_mut::<GLKViewControllerHostObject>(this).preferred_frames_per_second = fps;
    // Update the display link interval if it already exists.
    let dl = env.objc.borrow::<GLKViewControllerHostObject>(this).display_link;
    if dl != nil && fps > 0 {
        let interval = 1.0_f64 / fps as f64;
        // CADisplayLink exposes frameInterval (int), not a direct time setter.
        // frameInterval = 60 / fps (clamped to >= 1).
        let frame_interval = (60 / fps.max(1)).max(1) as crate::frameworks::foundation::NSInteger;
        let _: () = msg![env; dl setFrameInterval:frame_interval];
    }
}

// ---- paused -------------------------------------------------------------

- (bool)paused {
    env.objc.borrow::<GLKViewControllerHostObject>(this).paused
}
- (())setPaused:(bool)paused {
    env.objc.borrow_mut::<GLKViewControllerHostObject>(this).paused = paused;
    let dl = env.objc.borrow::<GLKViewControllerHostObject>(this).display_link;
    if dl != nil {
        let _: () = msg![env; dl setPaused:paused];
    }
}

// ---- informational read-only properties ---------------------------------

- (bool)pauseOnWillResignActive   { false }
- (())setPauseOnWillResignActive:(bool)_v {}
- (bool)resumeOnDidBecomeActive   { true  }
- (())setResumeOnDidBecomeActive:(bool)_v {}

- (NSInteger)framesPerSecond  {
    env.objc.borrow::<GLKViewControllerHostObject>(this).preferred_frames_per_second
}
- (NSInteger)framesDisplayed  { 0 }
- (f64)timeSinceLastUpdate    { 1.0 / 30.0 }
- (f64)timeSinceLastDraw      { 1.0 / 30.0 }
- (f64)timeSinceFirstResume   { 0.0 }
- (f64)timeSinceLastResume    { 0.0 }

// ---- view lifecycle -----------------------------------------------------

- (())viewDidLoad {
    () = msg_super![env; this viewDidLoad];
    log_dbg!("[GLKViewController {:?} viewDidLoad]", this);
}

- (())viewDidAppear:(bool)animated {
    () = msg_super![env; this viewDidAppear:animated];
    log_dbg!("[GLKViewController {:?} viewDidAppear:]", this);

    // Create the CADisplayLink once and add it to the main run loop.
    let already = env.objc.borrow::<GLKViewControllerHostObject>(this).display_link;
    if already != nil {
        return;
    }

    // The selector we register here must match the method defined below.
    let sel = env.objc.lookup_selector("_glkStep:")
        .expect("_glkStep: selector must be registered by objc_classes! macro");

    let dl: id = msg_class![env; CADisplayLink displayLinkWithTarget:this selector:sel];
    if dl == nil {
        log!("[GLKViewController] Warning: CADisplayLink creation failed");
        return;
    }
    retain(env, dl);
    env.objc.borrow_mut::<GLKViewControllerHostObject>(this).display_link = dl;

    // Set frame interval from preferredFramesPerSecond.
    let fps = env.objc.borrow::<GLKViewControllerHostObject>(this).preferred_frames_per_second;
    if fps > 0 {
        let frame_interval = (60 / fps.max(1)).max(1);
        let _: () = msg![env; dl setFrameInterval:frame_interval];
    }

    let run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
    let mode: id = crate::frameworks::foundation::ns_string::get_static_str(
        env, crate::frameworks::core_foundation::cf_run_loop::kCFRunLoopDefaultMode,
    );
    let _: () = msg![env; dl addToRunLoop:run_loop forMode:mode];

    log!("[GLKViewController {:?}] CADisplayLink {:?} registered", this, dl);
}

- (())viewWillDisappear:(bool)animated {
    () = msg_super![env; this viewWillDisappear:animated];
    let dl = env.objc.borrow::<GLKViewControllerHostObject>(this).display_link;
    if dl != nil {
        let _: () = msg![env; dl setPaused:true];
    }
}

// ---- _glkStep: — fired by CADisplayLink each frame ----------------------

- (())_glkStep:(id)_sender {
    let paused = env.objc.borrow::<GLKViewControllerHostObject>(this).paused;
    if paused {
        return;
    }
    let delegate = env.objc.borrow::<GLKViewControllerHostObject>(this).delegate;
    if delegate == nil {
        return;
    }

    // 1. Logic: [delegate glkViewControllerUpdate:self]
    if env.objc.object_has_method_named(&env.mem, delegate, "glkViewControllerUpdate:") {
        let _: () = msg![env; delegate glkViewControllerUpdate:this];
    }

    // 2. Draw: [delegate glkView:view drawInRect:bounds]
    if env.objc.object_has_method_named(&env.mem, delegate, "glkView:drawInRect:") {
        let view: id = msg![env; this view];
        let bounds: crate::frameworks::core_graphics::CGRect = msg![env; view bounds];
        let _: () = msg![env; delegate glkView:view drawInRect:bounds];
    }
}

// ---- dealloc ------------------------------------------------------------

- (())dealloc {
    let dl = env.objc.borrow::<GLKViewControllerHostObject>(this).display_link;
    if dl != nil {
        let _: () = msg![env; dl invalidate];
        release(env, dl);
        env.objc.borrow_mut::<GLKViewControllerHostObject>(this).display_link = nil;
    }
    // delegate is weak — do not release.
    () = msg_super![env; this dealloc];
}

@end

// =========================================================================
// GLKView : UIView  (minimal stub)
// =========================================================================

@implementation GLKView: UIView

- (id)initWithFrame:(crate::frameworks::core_graphics::CGRect)frame
            context:(id)_context {
    msg![env; this initWithFrame:frame]
}

- (())setContext:(id)_context  {}
- (id)context                  { nil }
- (())setDelegate:(id)_d       {}
- (id)delegate                 { nil }

- (())setDrawableColorFormat:(i32)_v   {}
- (i32)drawableColorFormat             { 0 }
- (())setDrawableDepthFormat:(i32)_v   {}
- (i32)drawableDepthFormat             { 0 }
- (())setDrawableStencilFormat:(i32)_v {}
- (i32)drawableStencilFormat           { 0 }
- (())setDrawableMultisample:(i32)_v   {}
- (i32)drawableMultisample             { 0 }

- (())setEnableSetNeedsDisplay:(bool)_v {}
- (bool)enableSetNeedsDisplay           { false }

- (())display      {}
- (())bindDrawable {}
- (())deleteDrawable {}
- (i32)drawableWidth   { 320 }
- (i32)drawableHeight  { 480 }

@end

};

// ---------------------------------------------------------------------------
// Exports
// ---------------------------------------------------------------------------

pub const CONSTANTS: ConstantExports = &[
    ("_GLKMatrix4Identity", HostConstant::Custom(glk_matrix4_identity)),
];

pub const FUNCTIONS: FunctionExports = &[];

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/GLKit.framework/GLKit",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[FUNCTIONS],
};
