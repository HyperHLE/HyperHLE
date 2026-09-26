/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `GKLocalPlayer`.

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::ns_string;
use crate::mem::{ConstVoidPtr, MutPtr, Ptr};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

/// Apple GameKit `GKError.h`:
/// `GKErrorNotAuthenticated = 6`. Returned by GameKit APIs when the
/// local player is not signed in to Game Center. We use this code in
/// the NSError we hand back to authentication completion handlers,
/// because touchHLE has no Game Center connectivity and so the local
/// player can never be authenticated.
const GK_ERROR_NOT_AUTHENTICATED: i32 = 6;

/// Apple Block ABI: word offset 3 (== byte offset 12) of a block
/// struct holds its `invoke` function pointer.
/// <https://clang.llvm.org/docs/Block-ABI-Apple.html>
const BLOCK_INVOKE_WORD_OFFSET: u32 = 3;

/// Build an autoreleased `NSError*` describing the "not signed in to
/// Game Center" condition. Domain = `GKErrorDomain`,
/// code = `GKErrorNotAuthenticated`, with a localized description so
/// games that surface the error to the user get a sensible message.
fn make_not_authenticated_error(env: &mut Environment) -> id {
    use crate::frameworks::foundation::ns_string::{from_rust_string, get_static_str};
    let domain = from_rust_string(env, "GKErrorDomain".to_string());
    autorelease(env, domain);

    let desc_key = get_static_str(env, "NSLocalizedDescription");
    let desc_val = from_rust_string(
        env,
        "The requested operation could not be completed because local \
         player has not been authenticated. (touchHLE: Game Center is \
         offline)"
            .to_string(),
    );
    autorelease(env, desc_val);

    let user_info: id = msg_class![env; NSMutableDictionary new];
    autorelease(env, user_info);
    () = msg![env; user_info setObject:desc_val forKey:desc_key];

    let error: id = msg_class![env; NSError alloc];
    let error: id = msg![env;
        error initWithDomain:domain
                        code:(GK_ERROR_NOT_AUTHENTICATED)
                    userInfo:user_info];
    autorelease(env, error);
    error
}

/// Read the `invoke` function pointer of an ObjC block, returning `None`
/// when the block is nil or its invoke pointer is zero. The latter happens
/// when the guest hands us a stack-allocated literal block that was never
/// `Block_copy`-ed and has already gone out of scope. Apple's `Block_ABI`:
/// <https://clang.llvm.org/docs/Block-ABI-Apple.html>.
fn block_invoke(env: &mut Environment, block: id) -> Option<GuestFunction> {
    if block == nil {
        return None;
    }
    let block_ptr: MutPtr<u32> = Ptr::from_bits(block.to_bits());
    let invoke_addr: u32 = env.mem.read(block_ptr + BLOCK_INVOKE_WORD_OFFSET);
    if invoke_addr == 0 {
        log!(
            "Warning: GKLocalPlayer block {:?} has NULL invoke pointer; not calling.",
            block
        );
        return None;
    }
    Some(GuestFunction::from_addr_with_thumb_bit(invoke_addr))
}

/// `Block_copy`/`Block_release` for the stored handlers. Blocks don't respond
/// to `copy`/`release` messages in touchHLE.
fn copy_block(env: &mut Environment, block: id) -> id {
    let copy = crate::libc::blocks::copy_block(env, Ptr::from_bits(block.to_bits()));
    Ptr::from_bits(copy.to_bits())
}
fn release_block(env: &mut Environment, block: id) {
    crate::libc::blocks::release_block(env, Ptr::from_bits(block.to_bits()));
}

/// Invoke an ObjC block whose underlying C function has the signature
/// `void (^)(NSError *)`.
fn invoke_error_block(env: &mut Environment, block: id, error: id) {
    let Some(invoke) = block_invoke(env, block) else {
        return;
    };
    let block_arg: ConstVoidPtr = Ptr::from_bits(block.to_bits()).cast_const();
    <GuestFunction as CallFromHost<(), (ConstVoidPtr, id)>>::call_from_host(
        &invoke,
        env,
        (block_arg, error),
    );
}

/// Invoke an ObjC block whose underlying C function has the signature
/// `void (^)(UIViewController *viewController, NSError *error)`. Used
/// by `-[GKLocalPlayer setAuthenticateHandler:]` introduced in iOS 6.
fn invoke_vc_error_block(env: &mut Environment, block: id, vc: id, error: id) {
    let Some(invoke) = block_invoke(env, block) else {
        return;
    };
    let block_arg: ConstVoidPtr = Ptr::from_bits(block.to_bits()).cast_const();
    <GuestFunction as CallFromHost<(), (ConstVoidPtr, id, id)>>::call_from_host(
        &invoke,
        env,
        (block_arg, vc, error),
    );
}

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

#[derive(Default)]
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
    /// Retained `void (^)(UIViewController*, NSError*)` block stored by
    /// `setAuthenticateHandler:`. GameKit keeps this for the lifetime of the
    /// process; we hold a strong reference and release it on dealloc.
    authenticate_handler: id,
    /// Pending one-shot `void (^)(NSError*)` completion block from
    /// `authenticateWithCompletionHandler:`, retained until it has been
    /// delivered on the next run-loop iteration, then released and cleared.
    pending_completion_handler: id,
}
impl HostObject for GKLocalPlayerHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation GKLocalPlayer: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<GKLocalPlayerHostObject>::default();
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

// Apple reference (iOS 7+):
// <https://developer.apple.com/documentation/gamekit/gklocalplayer/1521031-setdefaultleaderboardidentifier>
// "The completion handler is called with a nil error if the
//  identifier was set, or an NSError if the request failed."
//
// touchHLE has no Game Center connectivity, so the request always
// fails with `GKErrorNotAuthenticated`. We still invoke the
// completion handler so the caller can proceed.
+ (())setDefaultLeaderboardIdentifier:(id)_identifier
               withCompletionHandler:(id)handler {
    let error = make_not_authenticated_error(env);
    invoke_error_block(env, handler, error);
}

// Apple reference (iOS 7+):
// <https://developer.apple.com/documentation/gamekit/gklocalplayer/1521090-loaddefaultleaderboardidentifier>
// "If the default leaderboard identifier was loaded successfully,
//  this block receives a string. […] Otherwise the error parameter
//  contains an NSError describing the failure."
//
// The completion block signature is
//   void (^)(NSString *leaderboardIdentifier, NSError *error)
// which is the same ABI as `void (^)(id, id)`.
+ (())loadDefaultLeaderboardIdentifierWithCompletionHandler:(id)handler {
    let Some(invoke) = block_invoke(env, handler) else {
        return;
    };
    let block_arg: ConstVoidPtr = Ptr::from_bits(handler.to_bits()).cast_const();
    let error = make_not_authenticated_error(env);
    <GuestFunction as CallFromHost<(), (ConstVoidPtr, id, id)>>::call_from_host(
        &invoke, env, (block_arg, nil, error),
    );
}

// MARK: - Init / dealloc

- (id)init {
    this
}

- (())dealloc {
    let host = env.objc.borrow::<GKLocalPlayerHostObject>(this);
    let (player_id, alias, display_name, friends, authenticate_handler, pending) = (
        host.player_id,
        host.alias,
        host.display_name,
        host.friends,
        host.authenticate_handler,
        host.pending_completion_handler,
    );
    release(env, player_id);
    release(env, alias);
    release(env, display_name);
    release(env, friends);
    release_block(env, authenticate_handler);
    release_block(env, pending);
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

// Apple reference (iOS 4.1, deprecated iOS 6):
// <https://developer.apple.com/documentation/gamekit/gklocalplayer/1521099-authenticatewithcompletionhandl>
// "If the local player can't be authenticated, GameKit calls your
//  completion handler with an error."
//
// touchHLE has no Game Center connectivity, so we follow the documented
// "not authenticated" branch: leave `isAuthenticated == NO` and invoke the
// completion handler with a `GKErrorNotAuthenticated` NSError.
//
// CRITICAL: real GameKit *never* calls this handler synchronously from
// inside the method — authentication is asynchronous and the handler is
// delivered on a later main run-loop iteration. Some games (e.g.
// Bloodmasque / VampGame, com.square-enix.bloodmasque.e) re-enter GameKit
// or touch state that is only valid once the current call stack has
// unwound; calling the handler inline made the guest `abort()` right after
// "Error while logging in: Error Domain=GKErrorDomain Code=6". We therefore
// retain the block and schedule delivery via
// performSelector:withObject:afterDelay:0, exactly like NSURLConnection's
// deferred delegate callbacks, so it fires on the next run-loop turn on the
// main thread.
- (())authenticateWithCompletionHandler:(id)completion_handler {
    if completion_handler == nil {
        // Still post the notification, mirroring GameKit's ordering.
        post_auth_change_notification(env);
        return;
    }

    // Block_copy the handler so it survives the caller's stack frame, and
    // park it on the host object until the deferred selector fires.
    let retained: id = copy_block(env, completion_handler);
    {
        let host = env.objc.borrow_mut::<GKLocalPlayerHostObject>(this);
        // If a previous authentication is still pending, drop it; GameKit
        // only honours the most recent request.
        let old = host.pending_completion_handler;
        host.pending_completion_handler = retained;
        if old != nil {
            release_block(env, old);
        }
    }

    let sel = env
        .objc
        .register_host_selector("_touchHLE_deliverAuthCompletion".to_string(), &mut env.mem);
    () = msg![env; this performSelector:sel withObject:nil afterDelay:0.0_f64];
}

// Internal: deliver the parked completion handler on the next run-loop
// iteration (main thread). Posts the state-change notification first, to
// match GameKit's documented ordering, then invokes the block and releases
// our retained reference.
- (())_touchHLE_deliverAuthCompletion {
    let handler = {
        let host = env.objc.borrow_mut::<GKLocalPlayerHostObject>(this);
        let h = host.pending_completion_handler;
        host.pending_completion_handler = nil;
        h
    };
    if handler == nil {
        return;
    }

    post_auth_change_notification(env);

    let error = make_not_authenticated_error(env);
    invoke_error_block(env, handler, error);
    release_block(env, handler);
}

// Apple reference (iOS 6+):
// <https://developer.apple.com/documentation/gamekit/gklocalplayer/1521050-authenticatehandler>
// "Setting the value of this property triggers authentication. […]
//  If the player needs to sign in, the handler is called with a view
//  controller. If the player cannot sign in, the handler is called with a
//  non-nil error."
//
// We have no UI to present, so we always take the "cannot sign in" branch
// and invoke the handler with a nil view controller and the
// `GKErrorNotAuthenticated` NSError. As with
// authenticateWithCompletionHandler:, the handler must NOT run inline —
// GameKit invokes it asynchronously on the main thread. We Block_copy and
// retain it for the lifetime of the singleton (GameKit keeps the handler
// and may call it again on auth-state changes) and schedule the first
// delivery via performSelector:withObject:afterDelay:0.
- (())setAuthenticateHandler:(id)handler {
    // Replace any previously stored handler (GameKit keeps exactly one).
    let retained: id = if handler != nil {
        copy_block(env, handler)
    } else {
        nil
    };
    {
        let host = env.objc.borrow_mut::<GKLocalPlayerHostObject>(this);
        let old = host.authenticate_handler;
        host.authenticate_handler = retained;
        if old != nil {
            release_block(env, old);
        }
    }
    if retained == nil {
        return;
    }

    let sel = env
        .objc
        .register_host_selector("_touchHLE_deliverAuthHandler".to_string(), &mut env.mem);
    () = msg![env; this performSelector:sel withObject:nil afterDelay:0.0_f64];
}

- (id)authenticateHandler {
    env.objc.borrow::<GKLocalPlayerHostObject>(this).authenticate_handler
}

// Internal: deliver the stored authenticate handler on the next run-loop
// iteration (main thread) with a nil view controller and a not-authenticated
// error. We keep the handler retained on the host object (do not release it
// here) because GameKit may invoke it more than once.
- (())_touchHLE_deliverAuthHandler {
    let handler = env.objc.borrow::<GKLocalPlayerHostObject>(this).authenticate_handler;
    if handler == nil {
        return;
    }
    post_auth_change_notification(env);
    let error = make_not_authenticated_error(env);
    invoke_vc_error_block(env, handler, nil, error);
}

// MARK: - Friends

- (id)friends {
    env.objc.borrow::<GKLocalPlayerHostObject>(this).friends
}

// Apple reference (iOS 4.1+, deprecated iOS 10):
// <https://developer.apple.com/documentation/gamekit/gklocalplayer/1521101-loadfriendswithcompletionhandle>
// "If the friend list was loaded, this block receives an array of
//  player IDs (NSString). Otherwise, the error parameter contains an
//  NSError object that describes the problem."
//
// The completion block signature is
//   void (^)(NSArray *friends, NSError *error)
// We have no remote service, but we *do* have a deterministic local
// friends array (always empty) — so we report success with an empty
// array, matching what a real device returns when the local player
// is authenticated but has no friends.
- (())loadFriendsWithCompletionHandler:(id)completion_handler {
    let Some(invoke) = block_invoke(env, completion_handler) else {
        return;
    };
    let block_arg: ConstVoidPtr =
        Ptr::from_bits(completion_handler.to_bits()).cast_const();

    // Hand back the cached empty friends array; do NOT autorelease, the
    // contract is that the block borrows the reference for the call.
    let friends: id = env.objc.borrow::<GKLocalPlayerHostObject>(this).friends;
    let error: id = nil;
    <GuestFunction as CallFromHost<(), (ConstVoidPtr, id, id)>>::call_from_host(
        &invoke, env, (block_arg, friends, error),
    );
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

/// Post `GKPlayerAuthenticationDidChangeNotificationName` so registered
/// observers see the (unchanged, not-authenticated) state, matching the
/// ordering GameKit uses before invoking authentication callbacks.
fn post_auth_change_notification(env: &mut Environment) {
    let notif_center: id = msg_class![env; NSNotificationCenter defaultCenter];
    let name = ns_string::from_rust_string(
        env,
        GKPlayerAuthenticationDidChangeNotificationName.to_string(),
    );
    autorelease(env, name);
    () = msg![env; notif_center postNotificationName:name object:nil];
}

pub const GKPlayerAuthenticationDidChangeNotificationName: &str =
    "GKPlayerAuthenticationDidChangeNotificationName";
pub const GKPlayerDidChangeNotificationName: &str = "GKPlayerDidChangeNotificationName";

pub const CONSTANTS: ConstantExports = &[
    (
        "_GKPlayerAuthenticationDidChangeNotificationName",
        HostConstant::NSString(GKPlayerAuthenticationDidChangeNotificationName),
    ),
    (
        "_GKPlayerDidChangeNotificationName",
        HostConstant::NSString(GKPlayerDidChangeNotificationName),
    ),
];
