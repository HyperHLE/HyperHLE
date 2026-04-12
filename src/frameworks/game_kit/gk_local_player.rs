/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `GKLocalPlayer`.

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::ns_string;
use crate::mem::Ptr;
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, ClassExports, HostObject, NSZonePtr,
};
use crate::Environment;

// MARK: - Per-process state

/// Singleton cache for `[GKLocalPlayer localPlayer]`.
#[derive(Default)]
pub struct State {
    local_player: Option<id>,
}

impl State {
    fn get(env: &mut Environment) -> &mut State {
        &mut env.framework_state.game_kit.local_player
    }
}

// MARK: - Host object

struct GKLocalPlayerHostObject {
    /// `NSString*`
    player_id: id,
    /// `NSString*`
    alias: id,
    /// `NSString*`
    display_name: id,
    authenticated: bool,
    underage: bool,
    /// `NSArray*` of `NSString*` friend player IDs
    friends: id,
}
impl HostObject for GKLocalPlayerHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation GKLocalPlayer: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(GKLocalPlayerHostObject {
        player_id: nil,
        alias: nil,
        display_name: nil,
        authenticated: false,
        underage: false,
        friends: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// MARK: - Singleton

+ (id)localPlayer {
    // Real GameKit always returns the same retained singleton.
    // Cache it in State so it survives autorelease pool drains.
    if let Some(player) = State::get(env).local_player {
        return player;
    }

    // alloc gives refcount=1 which is our singleton retain — do NOT
    // autorelease here.
    let player: id = msg![env; this alloc];
    let player: id = msg![env; player init];

    let player_id = ns_string::from_rust_string(
        env,
        "GKLocalPlayer:touchHLE".to_string(),
    );
    let alias   = ns_string::from_rust_string(env, "Player".to_string());
    let display = ns_string::from_rust_string(env, "Player".to_string());
    let friends = msg_class![env; NSArray new];

    {
        let host = env.objc.borrow_mut::<GKLocalPlayerHostObject>(player);
        host.player_id    = player_id;
        host.alias        = alias;
        host.display_name = display;
        host.friends      = friends;
    }

    State::get(env).local_player = Some(player);
    log!("GKLocalPlayer localPlayer: singleton created");
    player
}

// MARK: - Score / achievement convenience (class-level)

+ (())setDefaultLeaderboardIdentifier:(id)_identifier
               withCompletionHandler:(id)_handler {
    log!("GKLocalPlayer setDefaultLeaderboardIdentifier:withCompletionHandler: stubbed");
}

+ (())loadDefaultLeaderboardIdentifierWithCompletionHandler:(id)_handler {
    log!("GKLocalPlayer loadDefaultLeaderboardIdentifierWithCompletionHandler: stubbed");
}

// MARK: - Init / dealloc

- (id)init {
    this
}

- (())dealloc {
    let host = env.objc.borrow::<GKLocalPlayerHostObject>(this);
    let (player_id, alias, display_name, friends) =
        (host.player_id, host.alias, host.display_name, host.friends);
    release(env, player_id);
    release(env, alias);
    release(env, display_name);
    release(env, friends);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Identity

- (id)playerID {
    env.objc.borrow::<GKLocalPlayerHostObject>(this).player_id
}

- (id)alias {
    env.objc.borrow::<GKLocalPlayerHostObject>(this).alias
}

- (id)displayName {
    env.objc.borrow::<GKLocalPlayerHostObject>(this).display_name
}

// MARK: - Authentication state

- (bool)isAuthenticated {
    env.objc.borrow::<GKLocalPlayerHostObject>(this).authenticated
}

- (bool)isUnderage {
    env.objc.borrow::<GKLocalPlayerHostObject>(this).underage
}

// Call the completion block synchronously so the game can handle the
// "not authenticated" state immediately rather than hanging forever.
// Block layout (ARM): +0x00 isa, +0x04 flags, +0x08 reserved,
// +0x0C invoke — void (*invoke)(void *block, NSError *error).
- (())authenticateWithCompletionHandler:(id)completion_handler {
    log!("GKLocalPlayer authenticateWithCompletionHandler: stubbed (not authenticated)");
    if completion_handler == nil {
        return;
    }
    // Read the invoke function pointer from the block descriptor.
    let invoke_field: crate::mem::ConstPtr<u32> =
        Ptr::from_bits(completion_handler.to_bits() + 0x0C);
    let invoke_bits = env.mem.read(invoke_field);
    if invoke_bits == 0 {
        return;
    }
    let invoke = GuestFunction::from_addr_with_thumb_bit(invoke_bits);
    // Call block(block_ptr, nil_error).
    () = invoke.call_from_host(env, (completion_handler, nil));
}

// iOS 6+ variant — block signature: void(^)(UIViewController*, NSError*).
// +0x0C invoke: void (*invoke)(void *block, UIViewController*, NSError*).
- (())setAuthenticateHandler:(id)handler {
    log!("GKLocalPlayer setAuthenticateHandler: stubbed (not authenticated)");
    if handler == nil {
        return;
    }
    let invoke_field: crate::mem::ConstPtr<u32> =
        Ptr::from_bits(handler.to_bits() + 0x0C);
    let invoke_bits = env.mem.read(invoke_field);
    if invoke_bits == 0 {
        return;
    }
    let invoke = GuestFunction::from_addr_with_thumb_bit(invoke_bits);
    // Call block(block_ptr, nil_view_controller, nil_error).
    () = invoke.call_from_host(env, (handler, nil, nil));
}

// MARK: - Friends

- (id)friends {
    env.objc.borrow::<GKLocalPlayerHostObject>(this).friends
}

- (())loadFriendsWithCompletionHandler:(id)_completion_handler {
    log!("GKLocalPlayer loadFriendsWithCompletionHandler: stubbed (no friends)");
}

- (id)description {
    let player_id =
        env.objc.borrow::<GKLocalPlayerHostObject>(this).player_id;
    let id_str = ns_string::to_rust_string(env, player_id);
    let desc = format!("<GKLocalPlayer: playerID={}>", id_str);
    let ns = ns_string::from_rust_string(env, desc);
    crate::objc::autorelease(env, ns)
}

@end

};

pub const GKPlayerAuthenticationDidChangeNotificationName: &str =
    "GKPlayerAuthenticationDidChangeNotificationName";

pub const CONSTANTS: ConstantExports = &[(
    "_GKPlayerAuthenticationDidChangeNotificationName",
    HostConstant::NSString(GKPlayerAuthenticationDidChangeNotificationName),
)];

