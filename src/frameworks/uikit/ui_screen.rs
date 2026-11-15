/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIScreen`.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::NSInteger;
use crate::frameworks::uikit::ui_geometry::UIEdgeInsets;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, ClassExports, HostObject,
    TrivialHostObject, SEL,
};

pub const CONSTANTS: ConstantExports = &[
    (
        "_UIScreenDidConnectNotification",
        HostConstant::NSString("UIScreenDidConnectNotification"),
    ),
    (
        "_UIScreenDidDisconnectNotification",
        HostConstant::NSString("UIScreenDidDisconnectNotification"),
    ),
];

#[derive(Default)]
pub struct State {
    main_screen: Option<id>,
    current_mode: Option<id>,
    /// `-brightness`, clamped to the documented 0.0-1.0 range.
    brightness: CGFloat,
    /// `-wantsSoftwareDimming`.
    wants_software_dimming: bool,
    /// `-overscanCompensation` (`UIScreenOverscanCompensation`). Overscan only
    /// affects an external display, so on the built-in display we emulate this
    /// is recorded and otherwise unused. iOS starts at
    /// `UIScreenOverscanCompensationScale`, which is 0, the default here.
    overscan_compensation: NSInteger,
}

/// Return the main screen's physical pixel size for the current orientation.
/// UIKit `bounds` is expressed in logical points; `nativeBounds` and
/// `UIScreenMode.size` are expressed in pixels.
fn screen_pixel_size_for_current_orientation(env: &mut crate::Environment) -> (CGFloat, CGFloat) {
    let (width, height) = screen_size_for_current_orientation(env);
    let scale = env.window().screen_scale() as CGFloat;
    (width as CGFloat * scale, height as CGFloat * scale)
}

fn screen_size_for_current_orientation(env: &mut crate::Environment) -> (u32, u32) {
    // Use the window's effective screen size, which honours a custom resolution
    // set via `--screen-size=WxH` or `--device-family=auto` (`host_screen_size`)
    // and otherwise falls back to the selected device family's fixed point size.
    // Previously this read `device_family().portrait_size()` directly, so a
    // custom/auto resolution was stored but never actually exposed to the guest
    // — the app kept rendering at the device's native point size.
    let portrait_size = env.window().screen_size();
    let orientation = env.window().current_rotation();
    let landscape_bounds = crate::env_flag_cached!("TOUCHHLE_LANDSCAPE_UISCREEN_BOUNDS");
    screen_size_for_orientation(portrait_size, orientation, landscape_bounds)
}

fn screen_size_for_orientation(
    portrait_size: (u32, u32),
    orientation: crate::window::DeviceOrientation,
    landscape_bounds: bool,
) -> (u32, u32) {
    let (portrait_width, portrait_height) = portrait_size;
    if landscape_bounds
        && matches!(
            orientation,
            crate::window::DeviceOrientation::LandscapeLeft
                | crate::window::DeviceOrientation::LandscapeRight
        )
    {
        (portrait_height, portrait_width)
    } else {
        (portrait_width, portrait_height)
    }
}

/// Per Apple documentation for `-setBrightness:`, values are clamped to the
/// documented 0.0-1.0 range by the setter below.
fn clamp_brightness(value: CGFloat) -> CGFloat {
    if value < 0.0 {
        0.0
    } else if value > 1.0 {
        1.0
    } else {
        value
    }
}

const UI_USER_INTERFACE_IDIOM_PHONE: NSInteger = 0;
const UI_USER_INTERFACE_IDIOM_PAD: NSInteger = 1;
const UI_USER_INTERFACE_SIZE_CLASS_COMPACT: NSInteger = 1;
const UI_USER_INTERFACE_SIZE_CLASS_REGULAR: NSInteger = 2;
const UI_FORCE_TOUCH_CAPABILITY_UNAVAILABLE: NSInteger = 1;
// `UIScreenOverscanCompensation`, from UIKit's `UIScreen.h`. `None` was called
// `InsetApplicationFrame` before iOS 9 and kept the same value.
const UISCREEN_OVERSCAN_COMPENSATION_SCALE: NSInteger = 0;
const UISCREEN_OVERSCAN_COMPENSATION_INSET_BOUNDS: NSInteger = 1;
const UISCREEN_OVERSCAN_COMPENSATION_NONE: NSInteger = 2;

#[derive(Default)]
struct UITraitCollectionHostObject {
    user_interface_idiom: NSInteger,
    horizontal_size_class: NSInteger,
    vertical_size_class: NSInteger,
    display_scale: CGFloat,
}
impl HostObject for UITraitCollectionHostObject {}

fn screen_trait_values(env: &mut crate::Environment) -> (NSInteger, NSInteger, NSInteger, CGFloat) {
    if env.window.is_none() {
        return (
            UI_USER_INTERFACE_IDIOM_PHONE,
            UI_USER_INTERFACE_SIZE_CLASS_COMPACT,
            UI_USER_INTERFACE_SIZE_CLASS_REGULAR,
            1.0,
        );
    }
    let idiom = if env.window().device_family().is_ipad() {
        UI_USER_INTERFACE_IDIOM_PAD
    } else {
        UI_USER_INTERFACE_IDIOM_PHONE
    };
    let landscape = matches!(
        env.window().current_rotation(),
        crate::window::DeviceOrientation::LandscapeLeft
            | crate::window::DeviceOrientation::LandscapeRight
    );
    let (horizontal, vertical) = if idiom == UI_USER_INTERFACE_IDIOM_PAD {
        (
            UI_USER_INTERFACE_SIZE_CLASS_REGULAR,
            UI_USER_INTERFACE_SIZE_CLASS_REGULAR,
        )
    } else if landscape {
        (
            UI_USER_INTERFACE_SIZE_CLASS_COMPACT,
            UI_USER_INTERFACE_SIZE_CLASS_COMPACT,
        )
    } else {
        (
            UI_USER_INTERFACE_SIZE_CLASS_COMPACT,
            UI_USER_INTERFACE_SIZE_CLASS_REGULAR,
        )
    };
    (
        idiom,
        horizontal,
        vertical,
        env.window().screen_scale() as CGFloat,
    )
}

fn new_trait_collection(
    env: &mut crate::Environment,
    user_interface_idiom: NSInteger,
    horizontal_size_class: NSInteger,
    vertical_size_class: NSInteger,
    display_scale: CGFloat,
) -> id {
    let class = env.objc.get_known_class("UITraitCollection", &mut env.mem);
    let collection = env.objc.alloc_object(
        class,
        Box::new(UITraitCollectionHostObject {
            user_interface_idiom,
            horizontal_size_class,
            vertical_size_class,
            display_scale,
        }),
        &mut env.mem,
    );
    autorelease(env, collection)
}

/// Backing object for the `id<UICoordinateSpace>` values that `UIScreen`
/// returns. It only needs `bounds`, because conversions between the screen's
/// own coordinate spaces are the identity.
#[derive(Default)]
struct UICoordinateSpaceHostObject {
    bounds: CGRect,
}
impl HostObject for UICoordinateSpaceHostObject {}

/// Create the coordinate-space object for `screen`. Apple returns a
/// long-lived object here; we return an autoreleased one because nothing in
/// the emulator retains it.
fn new_screen_coordinate_space(env: &mut crate::Environment, screen: id) -> id {
    let bounds: CGRect = msg![env; screen bounds];
    let class = env
        .objc
        .get_known_class("UICoordinateSpace", &mut env.mem);
    let object = env.objc.alloc_object(
        class,
        Box::new(UICoordinateSpaceHostObject { bounds }),
        &mut env.mem,
    );
    autorelease(env, object)
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIScreen: NSObject

// MARK: - Singleton

+ (id)mainScreen {
    if let Some(screen) = env.framework_state.uikit.ui_screen.main_screen {
        screen
    } else {
        let new = env.objc.alloc_static_object(
            this,
            Box::new(TrivialHostObject),
            &mut env.mem,
        );
        env.framework_state.uikit.ui_screen.main_screen = Some(new);
        new
    }
}

+ (id)screens {
    // Only one screen is ever available.
    let main: id = msg![env; this mainScreen];
    msg_class![env; NSArray arrayWithObject:main]
}

// MARK: - Retain / release (singleton — no-ops)

- (id)retain      { this }
- (())release     {}
- (id)autorelease { this }

// MARK: - Geometry

- (id)traitCollection {
    let (idiom, horizontal, vertical, scale) = screen_trait_values(env);
    new_trait_collection(env, idiom, horizontal, vertical, scale)
}

- (CGRect)bounds {
    let (width, height) = screen_size_for_current_orientation(env);
    if crate::env_flag_cached!("TOUCHHLE_LANDSCAPE_UISCREEN_BOUNDS") {
        // `-bounds` is called hundreds of times during app launch, and every
        // `log!` costs a logcat round trip plus a write to the log file, so
        // report the override once instead of on every call.
        static REPORTED: std::sync::Once = std::sync::Once::new();
        REPORTED.call_once(|| {
            log!(
                "TOUCHHLE_LANDSCAPE_UISCREEN_BOUNDS=1: UIScreen bounds reporting {}x{}",
                width,
                height
            );
        });
    }
    CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width:  width  as CGFloat,
            height: height as CGFloat,
        },
    }
}

- (CGRect)nativeBounds {
    // Same as bounds at scale 1 — we don't model the physical pixel grid.
    // nativeBounds is the physical screen in pixels (bounds.size * scale);
    // aliasing it to point-sized bounds made games render at half resolution
    // on Retina device profiles.
    let bounds: CGRect = msg![env; this bounds];
    let scale = env.window().screen_scale() as CGFloat;
    CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width:  bounds.size.width as CGFloat * scale,
            height: bounds.size.height as CGFloat * scale,
        },
    }
}

- (CGRect)applicationFrame {
    let mut bounds: CGRect = msg![env; this bounds];
    const STATUS_BAR_HEIGHT: CGFloat = 20.0;
    if !env.framework_state.uikit.ui_application.status_bar_hidden {
        bounds.origin.y    += STATUS_BAR_HEIGHT;
        bounds.size.height -= STATUS_BAR_HEIGHT;
    }
    bounds
}

// MARK: - Refresh rate

/// `- (NSInteger)maximumFramesPerSecond` (iOS 10.3+)
///
/// The emulated hardware is a 60 Hz iPhone, which is what the device classes
/// this emulator models actually run at. Apps query it to pick their video
/// timing; the unimplemented selector answered 0 (via the "does not respond"
/// path), which reads as "unknown refresh rate".
/// https://developer.apple.com/documentation/uikit/uiscreen/3029468-maximumframespersecond
- (NSInteger)maximumFramesPerSecond {
    60
}

// MARK: - Scale

- (CGFloat)scale {
    env.window().screen_scale() as CGFloat
}

- (CGFloat)nativeScale {
    // On real iOS devices nativeScale equals scale (the point-to-pixel
    // ratio of the physical display). Games use it to detect Retina and
    // size their framebuffers; reporting 1.0 while `scale` reports 2.0
    // made such games allocate a half-resolution framebuffer and draw
    // zoomed / cropped.
    env.window().screen_scale() as CGFloat
}

// MARK: - Brightness

- (CGFloat)brightness {
    env.framework_state.uikit.ui_screen.brightness
}

- (())setBrightness:(CGFloat)brightness {
    // Per Apple documentation, brightness is in the range 0.0 (darkest)
    // to 1.0 (lightest); clamp out-of-range values like the real UIKit.
    let clamped = clamp_brightness(brightness);
    if clamped != env.framework_state.uikit.ui_screen.brightness {
        log!(
            "[UIScreen setBrightness:] {} -> {} (host display unaffected)",
            env.framework_state.uikit.ui_screen.brightness,
            clamped
        );
        env.framework_state.uikit.ui_screen.brightness = clamped;
    }
}

- (bool)wantsSoftwareDimming {
    env.framework_state.uikit.ui_screen.wants_software_dimming
}

- (())setWantsSoftwareDimming:(bool)value {
    // Per Apple documentation this only selects whether brightness changes
    // are implemented by dimming; store the preference.
    env.framework_state.uikit.ui_screen.wants_software_dimming = value;
}

// MARK: - Display mode / overscan

- (id)currentMode {
    // `UIScreenMode.size` is in *pixels*, not points (it is the size of the
    // framebuffer the screen is currently rendering at). On a real device
    // this is `bounds.size * scale` — e.g. 640x960 on a Retina iPhone 4
    // whose `bounds` are 320x480 points. We used to report the point size,
    // which made engines that size their renderbuffer/projection from
    // `currentMode.size` (Gameloft's Dust engine — N.O.V.A. 3, Firemint,
    // …) allocate a viewport of half (or quarter) the EAGL renderbuffer
    // area and draw the scene into a corner of the screen on Retina
    // device profiles.
    let (width, height) = screen_size_for_current_orientation(env);
    let scale = env.window().screen_scale() as CGFloat;
    let size = CGSize {
        width:  width  as CGFloat * scale,
        height: height as CGFloat * scale,
    };

    crate::frameworks::uikit::ui_screen_mode::from_size(env, size, 1.0)
}

- (id)preferredMode {
    nil
}

- (id)availableModes {
    let current_mode: id = msg![env; this currentMode];
    msg_class![env; NSArray arrayWithObject:current_mode]
}

// Apple's
// <https://developer.apple.com/documentation/uikit/uiscreen/1617815-setcurrentmode>
// (now deprecated, was originally on UIScreen but in iOS 5+ Apple moved
// resolution control to UIScreenMode itself). The setter is documented as
// a no-op on screens that have only a single supported mode, which is
// the case for every device we emulate (a single fixed framebuffer).
// Accept the call so apps that probe / try to set the mode at launch
// don't get a "doesNotRecognizeSelector" log spam.
- (())setCurrentMode:(id)_mode {
    log_dbg!("[UIScreen setCurrentMode:] ignored; we emulate a single fixed display mode.");
}

// Apple's
// <https://developer.apple.com/documentation/uikit/uiscreen/overscancompensation>
// property. The documented values are Scale (0), InsetBounds (1) and None (2).
- (NSInteger)overscanCompensation {
    env.framework_state.uikit.ui_screen.overscan_compensation
}

- (())setOverscanCompensation:(NSInteger)compensation {
    // Overscan compensation only applies to an external, overscanning
    // display, and we emulate a single built-in one, so like iOS on a screen
    // with nothing to compensate for, the setter just records the value.
    // Undocumented values are a programming error and are ignored.
    let documented = matches!(
        compensation,
        UISCREEN_OVERSCAN_COMPENSATION_SCALE
            | UISCREEN_OVERSCAN_COMPENSATION_INSET_BOUNDS
            | UISCREEN_OVERSCAN_COMPENSATION_NONE
    );
    if !documented {
        log!(
            "[UIScreen setOverscanCompensation:] ignoring undocumented value {}",
            compensation
        );
        return;
    }
    env.framework_state.uikit.ui_screen.overscan_compensation = compensation;
}

// Apple's
// <https://developer.apple.com/documentation/uikit/uiscreen/overscancompensationinsets>
// property (iOS 9+): the insets that avoid clipping on an overscanning
// display. The built-in display we emulate doesn't overscan, so this is
// UIEdgeInsetsZero.
- (UIEdgeInsets)overscanCompensationInsets {
    UIEdgeInsets::default()
}

// MARK: - Mirroring (iOS 4.3+)

- (id)mirroredScreen {
    nil
}

- (bool)isCaptured {
    false
}

// MARK: - Coordinate conversion helpers

- (CGRect)convertRect:(CGRect)rect toScreen:(id)_other_screen {
    // Single-screen device — coordinates are always the same.
    rect
}

- (CGRect)convertRect:(CGRect)rect fromScreen:(id)_other_screen {
    rect
}

- (CGPoint)convertPoint:(CGPoint)point toScreen:(id)_other_screen {
    point
}

- (CGPoint)convertPoint:(CGPoint)point fromScreen:(id)_other_screen {
    point
}
// MARK: - Display Link

- (id)displayLinkWithTarget:(id)target selector:(SEL)selector {
    msg_class![env; CADisplayLink displayLinkWithTarget:target selector:selector]
}

// Apple's
// <https://developer.apple.com/documentation/uikit/uiscreen/1617810-coordinatespace>:
// "The current coordinate space of the screen. ... use this coordinate space
// with the `UICoordinateSpace` protocol methods to convert between the
// screen's coordinate system and other coordinate systems." The screen space
// always has its origin at the top-left corner and `bounds` as its extent.
- (id)coordinateSpace {
    new_screen_coordinate_space(env, this)
}

// Apple's
// <https://developer.apple.com/documentation/uikit/uiscreen/1617823-fixedcoordinatespace>:
// "The fixed coordinate space of the screen. ... unlike the coordinateSpace
// property, this coordinate space does not change when the interface
// rotates." The built-in screen always reports portrait-oriented bounds, so
// both properties describe the same space.
- (id)fixedCoordinateSpace {
    new_screen_coordinate_space(env, this)
}

@end

@implementation UICoordinateSpace: NSObject

- (CGRect)bounds {
    env.objc.borrow::<UICoordinateSpaceHostObject>(this).bounds
}

// The only coordinate spaces this emulator hands out describe the same
// screen-sized, origin-at-zero space, so converting between them, or between
// them and themselves, never changes the value.
- (CGPoint)convertPoint:(CGPoint)point toCoordinateSpace:(id)_space {
    point
}
- (CGPoint)convertPoint:(CGPoint)point fromCoordinateSpace:(id)_space {
    point
}
- (CGRect)convertRect:(CGRect)rect toCoordinateSpace:(id)_space {
    rect
}
- (CGRect)convertRect:(CGRect)rect fromCoordinateSpace:(id)_space {
    rect
}

@end

@implementation UITraitCollection: NSObject

+ (id)currentTraitCollection {
    let screen: id = msg_class![env; UIScreen mainScreen];
    msg![env; screen traitCollection]
}

+ (id)traitCollectionWithUserInterfaceIdiom:(NSInteger)idiom {
    let (_, _, _, scale) = screen_trait_values(env);
    let (horizontal, vertical) = if idiom == UI_USER_INTERFACE_IDIOM_PAD {
        (UI_USER_INTERFACE_SIZE_CLASS_REGULAR, UI_USER_INTERFACE_SIZE_CLASS_REGULAR)
    } else {
        (UI_USER_INTERFACE_SIZE_CLASS_COMPACT, UI_USER_INTERFACE_SIZE_CLASS_REGULAR)
    };
    new_trait_collection(env, idiom, horizontal, vertical, scale)
}

+ (id)traitCollectionWithHorizontalSizeClass:(NSInteger)horizontalSizeClass
                            verticalSizeClass:(NSInteger)verticalSizeClass {
    let (idiom, _, _, scale) = screen_trait_values(env);
    new_trait_collection(env, idiom, horizontalSizeClass, verticalSizeClass, scale)
}

+ (id)traitCollectionWithDisplayScale:(CGFloat)displayScale {
    let (idiom, horizontal, vertical, _) = screen_trait_values(env);
    new_trait_collection(env, idiom, horizontal, vertical, displayScale)
}

- (NSInteger)userInterfaceIdiom {
    env.objc.borrow::<UITraitCollectionHostObject>(this).user_interface_idiom
}

- (NSInteger)horizontalSizeClass {
    env.objc.borrow::<UITraitCollectionHostObject>(this).horizontal_size_class
}

- (NSInteger)verticalSizeClass {
    env.objc.borrow::<UITraitCollectionHostObject>(this).vertical_size_class
}

- (CGFloat)displayScale {
    env.objc.borrow::<UITraitCollectionHostObject>(this).display_scale
}

- (NSInteger)forceTouchCapability {
    UI_FORCE_TOUCH_CAPABILITY_UNAVAILABLE
}

- (NSInteger)layoutDirection { 0 }

@end

};

#[cfg(test)]
mod tests {
    use super::screen_size_for_orientation;
    use crate::window::{DeviceFamily, DeviceOrientation};

    #[test]
    fn iphone_5_screen_bounds_follow_landscape_orientation() {
        for device_family in [DeviceFamily::iPhone5, DeviceFamily::iPhone5c] {
            let portrait_size = device_family.portrait_size();
            assert_eq!(
                screen_size_for_orientation(portrait_size, DeviceOrientation::LandscapeLeft, true,),
                (568, 320)
            );
            assert_eq!(
                screen_size_for_orientation(portrait_size, DeviceOrientation::LandscapeRight, true,),
                (568, 320)
            );
            assert_eq!(
                screen_size_for_orientation(portrait_size, DeviceOrientation::Portrait, true,),
                (320, 568)
            );
            assert_eq!(
                screen_size_for_orientation(
                    portrait_size,
                    DeviceOrientation::PortraitUpsideDown,
                    true,
                ),
                (320, 568)
            );
            assert_eq!(
                screen_size_for_orientation(portrait_size, DeviceOrientation::LandscapeLeft, false,),
                (320, 568)
            );
        }
    }
}
