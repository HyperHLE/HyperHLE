/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMoviePlayerController` etc.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::{ns_string, ns_url, NSInteger};
use crate::frameworks::uikit::ui_device::UIDeviceOrientation;
use crate::frameworks::core_graphics::CGRect;
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, retain, todo_objc_setter, ClassExports,
    HostObject, NSZonePtr,
};
use crate::Environment;
use std::collections::VecDeque;
use std::time::Instant;

#[derive(Default)]
pub struct State {
    active_player: Option<id>,
    /// Various apps (e.g. Crash Bandicoot Nitro Kart 3D and Spore Origins)
    /// create or start a player and await some kind of notification, but can't
    /// handle it if that notification happens immediately.
    /// This queue lets us
    /// delay such notifications until the app next returns to the run loop,
    /// which seems to be late enough.
    pending_notifications: VecDeque<(&'static str, id, Instant)>,
}
impl State {
    fn get(env: &mut Environment) -> &mut Self {
        &mut env.framework_state.media_player.movie_player
    }
}

type MPMovieScalingMode = NSInteger;
type MPMovieControlStyle = NSInteger;
type MPMovieSourceType = NSInteger;
type MPMovieRepeatMode = NSInteger;

type MPMoviePlaybackState = NSInteger;
const MPMoviePlaybackStateStopped: MPMoviePlaybackState = 0;
const MPMoviePlaybackStatePlaying: MPMoviePlaybackState = 1;
const MPMoviePlaybackStatePaused: MPMoviePlaybackState = 2;

// Values might not be correct, but as these are linked symbol constants, it
// shouldn't matter.
pub const MPMoviePlayerPlaybackDidFinishNotification: &str =
    "MPMoviePlayerPlaybackDidFinishNotification";
/// Apparently an undocumented, private API. Spore Origins uses it.
pub const MPMoviePlayerContentPreloadDidFinishNotification: &str =
    "MPMoviePlayerContentPreloadDidFinishNotification";
pub const MPMoviePlayerScalingModeDidChangeNotification: &str =
    "MPMoviePlayerScalingModeDidChangeNotification";
pub const MPMoviePlayerLoadStateDidChangeNotification: &str = 
    "MPMoviePlayerLoadStateDidChangeNotification";

const MPMoviePlayerPlaybackDidFinishReasonUserInfoKey: &str =
    "MPMoviePlayerPlaybackDidFinishReasonUserInfoKey";

/// `NSNotificationName` values and other constants.
pub const CONSTANTS: ConstantExports = &[
    (
        "_MPMoviePlayerPlaybackDidFinishNotification",
        HostConstant::NSString(MPMoviePlayerPlaybackDidFinishNotification),
    ),
    (
        "_MPMoviePlayerContentPreloadDidFinishNotification",
        HostConstant::NSString(MPMoviePlayerContentPreloadDidFinishNotification),
    ),
    (
        "_MPMoviePlayerScalingModeDidChangeNotification",
        HostConstant::NSString(MPMoviePlayerScalingModeDidChangeNotification),
    ),
    (
        "_MPMoviePlayerLoadStateDidChangeNotification",
        HostConstant::NSString(MPMoviePlayerLoadStateDidChangeNotification),
    ),
    (
        "_MPMoviePlayerPlaybackDidFinishReasonUserInfoKey",
        HostConstant::NSString(MPMoviePlayerPlaybackDidFinishReasonUserInfoKey),
    ),
];
struct MPMoviePlayerControllerHostObject {
    // NSURL *
    content_url: id,
    // UIView *
    view: id,
    background_view: id,
    scaling_mode: MPMovieScalingMode,
    control_style: MPMovieControlStyle,
    source_type: MPMovieSourceType,
    repeat_mode: MPMovieRepeatMode,
    should_autoplay: bool,
    initial_playback_time: f64,
    playback_state: MPMoviePlaybackState,
}
impl HostObject for MPMoviePlayerControllerHostObject {}
/// Ensure the player has a valid dummy view, creating one lazily if needed.
/// Returns the view id (always non-nil after this call).
fn ensure_view(env: &mut Environment, this: id) -> id {
    let existing = env
        .objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .view;
    if existing != nil {
        return existing;
    }
    
    let view_alloc: id = msg_class![env; UIView alloc];
    let view: id = msg![env; view_alloc init];
    
    // Patch: Define standard iPhone landscape frame to prevent 0x6 Null-Page Read crashes
    let frame = crate::frameworks::core_graphics::CGRect {
        origin: crate::frameworks::core_graphics::CGPoint { x: 0.0, y: 0.0 },
        size: crate::frameworks::core_graphics::CGSize { width: 480.0, height: 320.0 },
    };
    let _: () = msg![env; view setFrame: frame];

    retain(env, view);
    env.objc
        .borrow_mut::<MPMoviePlayerControllerHostObject>(this)
        .view = view;
    view
}

fn ensure_background_view(env: &mut Environment, this: id) -> id {
    let existing = env
        .objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .background_view;
    if existing != nil {
        return existing;
    }
    let view_alloc: id = msg_class![env; UIView alloc];
    let view: id = msg![env; view_alloc init];
    retain(env, view);
    env.objc
        .borrow_mut::<MPMoviePlayerControllerHostObject>(this)
        .background_view = view;
    view
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMoviePlayerController: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(MPMoviePlayerControllerHostObject {
        content_url: nil,
        view: nil,
        background_view: nil,
        scaling_mode: 0,
        control_style: 0,
        source_type: 0,
        repeat_mode: 0,
        should_autoplay: true,
        initial_playback_time: -1.0,
        playback_state: MPMoviePlaybackStateStopped,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithContentURL:(id)url { // NSURL*
    log!(
        "[(MPMoviePlayerController*){:?} initWithContentURL:{:?}]",
        this,
        ns_url::to_rust_path(env, url),
    );
    
    let this: id = msg![env; this init];
    retain(env, url);

    {
        let mut host = env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this);
        host.content_url = url;
    }

    ensure_view(env, this);
    ensure_background_view(env, this);

    // 1. Notify app that content is preloaded
    retain(env, this);
    State::get(env).pending_notifications.push_back((
        MPMoviePlayerContentPreloadDidFinishNotification,
        this,
        Instant::now(),
    ));

    // 2. Notify app that load state has changed (Crucial for Power Rangers Samurai)
    retain(env, this);
    State::get(env).pending_notifications.push_back((
        MPMoviePlayerLoadStateDidChangeNotification,
        this,
        Instant::now(),
    ));

    // 3. THE SKIP HACK: Tell the game the video finished after 150ms.
    // This forces the game to move from the Saban logo to the Main Menu.
    retain(env, this);
    State::get(env).pending_notifications.push_back((
        MPMoviePlayerPlaybackDidFinishNotification,
        this,
        Instant::now() + std::time::Duration::from_millis(150),
    ));
    
    this
}
    - (())dealloc {
    // 1. Extract the IDs into local variables so we can drop the 'host' borrow
    let (url, view, bg_view) = {
        let host = env.objc.borrow::<MPMoviePlayerControllerHostObject>(this);
        (host.content_url, host.view, host.background_view)
    }; 

    // 2. Now we can safely borrow 'env' mutably for the release calls
    release(env, url);
    release(env, view);
    release(env, bg_view);

    // 3. Finally, destroy the object itself
    env.objc.dealloc_object(this, &mut env.mem);
}
    
- (id)contentURL {
    env.objc.borrow::<MPMoviePlayerControllerHostObject>(this).content_url
}

- (id)backgroundColor {
    msg_class![env; UIColor blackColor]
}
- (())setBackgroundColor:(id)color { 
    todo_objc_setter!(this, color);
}

- (MPMovieScalingMode)scalingMode {
    env.objc.borrow::<MPMoviePlayerControllerHostObject>(this).scaling_mode
}
- (())setScalingMode:(MPMovieScalingMode)mode {
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).scaling_mode = mode;
}

- (MPMovieControlStyle)controlStyle {
    env.objc.borrow::<MPMoviePlayerControllerHostObject>(this).control_style
}
- (())setControlStyle:(MPMovieControlStyle)style {
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).control_style = style;
}

- (MPMovieSourceType)movieSourceType {
    env.objc.borrow::<MPMoviePlayerControllerHostObject>(this).source_type
}
- (())setMovieSourceType:(MPMovieSourceType)source_type {
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).source_type = source_type;
}

- (MPMovieRepeatMode)repeatMode {
    env.objc.borrow::<MPMoviePlayerControllerHostObject>(this).repeat_mode
}
- (())setRepeatMode:(MPMovieRepeatMode)mode {
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).repeat_mode = mode;
}

- (bool)shouldAutoplay {
    env.objc.borrow::<MPMoviePlayerControllerHostObject>(this).should_autoplay
}
- (())setShouldAutoplay:(bool)autoplay {
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).should_autoplay = autoplay;
}

- (())setUseApplicationAudioSession:(bool)use_session {
    todo_objc_setter!(this, use_session);
}

- (())setFullscreen:(bool)fullscreen {
    todo_objc_setter!(this, fullscreen);
}

- (())setFullscreen:(bool)fullscreen animated:(bool)animated {
    log!("TODO: setFullscreen animated");
}

- (id)view {
    ensure_view(env, this)
}
    
- (id)backgroundView {
    ensure_background_view(env, this)
}
- (())setBackgroundView:(id)view {
    todo_objc_setter!(this, view);
}

- (MPMoviePlaybackState)playbackState {
    env.objc.borrow::<MPMoviePlayerControllerHostObject>(this).playback_state
}

- (f64)currentPlaybackTime {
    1.0 
}
- (())setCurrentPlaybackTime:(f64)time {
    todo_objc_setter!(this, time);
}
    - (f64)initialPlaybackTime {
    env.objc.borrow::<MPMoviePlayerControllerHostObject>(this).initial_playback_time
}
- (())setInitialPlaybackTime:(f64)time {
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).initial_playback_time = time;
}

- (f64)duration { 1.0 }
- (f64)playableDuration { 1.0 }
- (bool)isPreparedToPlay { true }
- (bool)readyForDisplay { true }

- (())prepareToPlay {}

- (())setMovieControlMode:(NSInteger)_mode {}

- (())setOrientation:(UIDeviceOrientation)_orientation animated:(bool)_animated {}

- (())play {
    log!("[(MPMoviePlayerController*){:?} play]", this);
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).playback_state = MPMoviePlaybackStatePlaying;
}

- (())pause {
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).playback_state = MPMoviePlaybackStatePaused;
}

- (())stop {
    log!("[(MPMoviePlayerController*){:?} stop]", this);
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).playback_state = MPMoviePlaybackStateStopped;
    if env.framework_state.media_player.movie_player.active_player == Some(this) {
        env.framework_state.media_player.movie_player.active_player = None;
        release(env, this);
    }
}

@end

@implementation MPMoviePlayerViewController: UIViewController

- (id)initWithContentURL:(id)url {
    let this: id = msg![env; this init];
    this
}

@end

};

pub(super) fn handle_players(env: &mut Environment) {
    let mut notifs_to_run = Vec::new();
    let pending_notifs = &mut State::get(env).pending_notifications;
    let mut i = 0;
    while i < pending_notifs.len() {
        let (name_str, object, time) = pending_notifs[i];
        if Instant::now() >= time {
            notifs_to_run.push((name_str, object));
            pending_notifs.swap_remove_back(i);
        } else {
            i += 1;
        }
    }

    for (name_str, object) in notifs_to_run {
        if name_str == MPMoviePlayerPlaybackDidFinishNotification {
            env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(object).playback_state = MPMoviePlaybackStateStopped;
        }
        let name = ns_string::get_static_str(env, name_str);
        let center: id = msg_class![env; NSNotificationCenter defaultCenter];
        
        if name_str == MPMoviePlayerPlaybackDidFinishNotification {
            let reason_num: id = msg_class![env; NSNumber numberWithInt:0i32];
            let reason_key = ns_string::get_static_str(env, MPMoviePlayerPlaybackDidFinishReasonUserInfoKey);
            let user_info: id = msg_class![env; NSDictionary dictionaryWithObject:reason_num forKey:reason_key];
            let _: () = msg![env; center postNotificationName:name object:object userInfo:user_info];
        } else {
            let _: () = msg![env; center postNotificationName:name object:object];
        }
        release(env, object);
    }
}
