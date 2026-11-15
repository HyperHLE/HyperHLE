/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//!
//! Handling of Objective-C messaging (`objc_msgSend` and friends).
//!
//! Resources:
//! - Apple's [Objective-C Runtime Programming Guide](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/ObjCRuntimeGuide/Articles/ocrtHowMessagingWorks.html)
//!
//!
//! - [Apple's documentation of `objc_msgSend`](https://developer.apple.com/documentation/objectivec/1456712-objc_msgsend)
//! - Mike Ash's [objc_msgSend's New Prototype](https://www.mikeash.com/pyblog/objc_msgsends-new-prototype.html)
//!
//!
//! - Peter Steinberger's [Calling Super at Runtime in Swift](https://steipete.com/posts/calling-super-at-runtime/) explains `objc_msgSendSuper2`

use super::{id, nil, Class, ObjC, IMP, SEL};
use crate::abi::{CallFromHost, GuestRet};
use crate::mem::{ConstPtr, MutVoidPtr, Ptr, SafeRead};
use crate::Environment;
use std::any::TypeId;
fn trace_cocos_menu(env: &mut Environment, menu: id, touch: id) {
    let saved_regs = *env.cpu.regs();
    let Some(location_selector) = env.objc.lookup_selector("locationInView:") else {
        return;
    };
    let location: CGPoint = msg_send(env, (touch, location_selector, nil));
    let director_class = env
        .objc
        .get_known_class("CCDirectorIOSUniversal", &mut env.mem);
    let Some(shared_director_selector) = env.objc.lookup_selector("sharedDirector") else {
        env.cpu.regs_mut().copy_from_slice(&saved_regs);
        return;
    };
    let director: id = msg_send(env, (director_class, shared_director_selector));
    let Some(convert_touch_selector) = env.objc.lookup_selector("convertTouchToGL:") else {
        env.cpu.regs_mut().copy_from_slice(&saved_regs);
        return;
    };
    let gl_location: CGPoint = msg_send(env, (director, convert_touch_selector, touch));
    let Some(convert_node_selector) = env.objc.lookup_selector("convertToNodeSpace:") else {
        env.cpu.regs_mut().copy_from_slice(&saved_regs);
        return;
    };
    let menu_location: CGPoint = msg_send(env, (menu, convert_node_selector, gl_location));
    let Some(parent_bounds_selector) = env.objc.lookup_selector("parentBounds") else {
        env.cpu.regs_mut().copy_from_slice(&saved_regs);
        return;
    };
    let parent_bounds: CGRect = msg_send(env, (menu, parent_bounds_selector));
    let Some(children_selector) = env.objc.lookup_selector("children") else {
        env.cpu.regs_mut().copy_from_slice(&saved_regs);
        return;
    };
    let children: id = msg_send(env, (menu, children_selector));
    let Some(count_selector) = env.objc.lookup_selector("count") else {
        env.cpu.regs_mut().copy_from_slice(&saved_regs);
        return;
    };
    let count: u32 = msg_send(env, (children, count_selector));
    log!(
        "MENU-TRACE menu={:?} bounds={:?} touch={:?} gl={:?} local={:?} children={}",
        menu,
        parent_bounds,
        location,
        gl_location,
        menu_location,
        count
    );
    let Some(object_at_index_selector) = env.objc.lookup_selector("objectAtIndex:") else {
        env.cpu.regs_mut().copy_from_slice(&saved_regs);
        return;
    };
    let Some(rect_selector) = env.objc.lookup_selector("rect") else {
        env.cpu.regs_mut().copy_from_slice(&saved_regs);
        return;
    };
    let Some(position_selector) = env.objc.lookup_selector("position") else {
        env.cpu.regs_mut().copy_from_slice(&saved_regs);
        return;
    };
    let Some(size_selector) = env.objc.lookup_selector("contentSize") else {
        env.cpu.regs_mut().copy_from_slice(&saved_regs);
        return;
    };
    let Some(visible_selector) = env.objc.lookup_selector("visible") else {
        env.cpu.regs_mut().copy_from_slice(&saved_regs);
        return;
    };
    let Some(enabled_selector) = env.objc.lookup_selector("isEnabled") else {
        env.cpu.regs_mut().copy_from_slice(&saved_regs);
        return;
    };
    for index in 0..count.min(64) {
        let child: id = msg_send(env, (children, object_at_index_selector, index));
        if child == nil {
            continue;
        }
        let child_class = ObjC::read_isa(child, &env.mem);
        let child_class_name = env.objc.get_class_name(child_class).to_owned();
        let position: CGPoint = msg_send(env, (child, position_selector));
        let size: CGSize = msg_send(env, (child, size_selector));
        let rect: CGRect = msg_send(env, (child, rect_selector));
        let visible: bool = msg_send(env, (child, visible_selector));
        let enabled: bool = msg_send(env, (child, enabled_selector));
        log!(
            "MENU-ITEM index={} object={:?} class={} position={:?} size={:?} rect={:?} visible={} enabled={}",
            index,
            child,
            child_class_name,
            position,
            size,
            rect,
            visible,
            enabled
        );
    }
    env.cpu.regs_mut().copy_from_slice(&saved_regs);
}
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};


/// Implements Apple's lazy `+initialize` contract:
/// > The runtime sends `initialize` to each class in a program just before the
/// > class, or any class that inherits from it, is sent its first message from
/// > within the program. Superclasses receive this message before their
/// > subclasses.
///
/// See <https://developer.apple.com/documentation/objectivec/nsobject/1418639-initialize>.
///
/// `class_to_init` must be a (regular) class, not a metaclass. The class is
/// marked as initialized *before* `+initialize` is dispatched, so that any
/// messages sent to it from within `+initialize` itself do not cause infinite
/// recursion.
fn ensure_class_initialized(env: &mut Environment, class_to_init: Class) {
    if class_to_init == nil {
        return;
    }
    if env.objc.initialized_classes.contains(&class_to_init) {
        return;
    }

    // Initialize the superclass first ("Superclasses receive this message
    // before their subclasses").
    let superclass = {
        let Some(host_object) = env.objc.get_host_object(class_to_init) else {
            // Class has no host object – nothing to initialize. Mark it as
            // done so we don't waste cycles re-checking on every dispatch.
            env.objc.initialized_classes.insert(class_to_init);
            return;
        };
        if let Some(co) = host_object
            .as_any()
            .downcast_ref::<super::ClassHostObject>()
        {
            co.superclass
        } else {
            // FakeClass / UnimplementedClass – nothing to initialize.
            env.objc.initialized_classes.insert(class_to_init);
            return;
        }
    };
    ensure_class_initialized(env, superclass);

    // Re-check after recursion (the recursive call could not have
    // initialised this class, but be defensive).
    if !env.objc.initialized_classes.insert(class_to_init) {
        return;
    }

    // Decide whether to actually dispatch `+initialize`. We send it iff
    // any class in the metaclass chain implements `initialize`. Otherwise
    // there is nothing to call (the inherited NSObject default is a no-op
    // anyway) and dispatching would just emit a "does not respond" warning.
    let metaclass = ObjC::read_isa(class_to_init, &env.mem);
    let Some(sel_initialize) = env.objc.lookup_selector("initialize") else {
        return;
    };
    if !env.objc.class_has_method(metaclass, sel_initialize) {
        return;
    }

    // `+initialize` only takes (self, _cmd); however, the *outer* message
    // dispatch we're nested inside has its real arguments sitting in r0..r3
    // (and possibly on the stack). Dispatching `+initialize` will clobber
    // r0..r3, so snapshot them and restore afterwards. SP/LR are already
    // preserved by `call_from_host`. Stack arguments and VFP registers used
    // for FP arguments aren't touched by a 2-argument `+initialize` call.
    let saved_r0_r3 = [
        env.cpu.regs()[0],
        env.cpu.regs()[1],
        env.cpu.regs()[2],
        env.cpu.regs()[3],
    ];
    log_dbg!("Dispatching +[{:?} initialize]", class_to_init);
    let _: () = msg_send_no_type_checking(env, (class_to_init, sel_initialize));
    let regs = env.cpu.regs_mut();
    regs[0..4].copy_from_slice(&saved_r0_r3);
}

/// The core implementation of `objc_msgSend`, the main function of Objective-C.
///
/// Note that while only two parameters (usually receiver and selector) are
/// defined by the wrappers over this function, a call to an `objc_msgSend`
/// variant may have additional arguments to be forwarded (or rather, left
/// untouched) by `objc_msgSend` when it tail-calls the method implementation it
/// looks up.
/// This is invisible to the Rust type system; we're relying on
/// [crate::abi::CallFromGuest] here.
///
/// Similarly, the return value of `objc_msgSend` is whatever value is returned
/// by the method implementation.
/// We are relying on CallFromGuest not
/// overwriting it.
#[allow(non_snake_case)]
fn objc_msgSend_inner(
    env: &mut Environment,
    receiver: id,
    selector: SEL,
    super2: Option<Class>,
    tolerate_type_mismatch: bool,
    skip_initialize: bool,
) {
    log_dbg!(
        "Dispatching {} for {:?}",
        selector.as_str(&env.mem),
        receiver
    );
    if crate::env_flag_cached!("TOUCHHLE_TRACE_ENUM") && receiver != nil {
        use std::sync::atomic::{AtomicU64, Ordering};
        static ENUM_CALLS: AtomicU64 = AtomicU64::new(0);
        if selector.as_str(&env.mem) == "countByEnumeratingWithState:objects:count:" {
            let n = ENUM_CALLS.fetch_add(1, Ordering::Relaxed);
            if n < 80 || n % 4096 == 0 {
                let class = ObjC::read_isa(receiver, &env.mem);
                let name = env.objc.get_class_name(class).to_owned();
                log!(
                    "TRACE_ENUM: countByEnumerating call #{} receiver={:?} class={}",
                    n, receiver, name
                );
            }
        }
    }
    if crate::env_flag_cached!("TOUCHHLE_TRACE_MSGS") && receiver != nil {
        use std::sync::Mutex;
        static SEEN: Mutex<Option<std::collections::HashSet<(Class, SEL)>>> = Mutex::new(None);
        let class = ObjC::read_isa(receiver, &env.mem);
        let sel_name = selector.as_str(&env.mem);
        if crate::env_flag_cached!("TOUCHHLE_TRACE_IVARS")
            && sel_name == "execute"
            && env.objc.get_class_name(class).contains("CallFunc")
        {
            let class_name = env.objc.get_class_name(class).to_string();
            let names = env.objc.debug_all_class_ivars_as_strings(class);
            log!("CALLFUNC-OBJECT class={} object={:?} ivars={:?}", class_name, receiver, names);
            for name in names {
                let value = env.objc.object_lookup_ivar(&env.mem, receiver, &name)
                    .map(|pointer| env.mem.read(pointer));
                log!("CALLFUNC-IVAR class={} object={:?} name={} value={:?}", class_name, receiver, name, value);
            }
            let callback_address = env.objc.object_lookup_ivar(&env.mem, receiver, &"_targetCallback".to_string())
                .map(|pointer| env.mem.read(pointer))
                .unwrap_or(0);
            if callback_address != 0 {
                let words: Vec<u32> = (0..6)
                    .map(|index| env.mem.read(crate::mem::ConstPtr::<u32>::from_bits(callback_address + index * 4)))
                    .collect();
                let callback_object = id::from_bits(callback_address);
                let callback_isa = ObjC::read_isa(callback_object, &env.mem);
                log!("CALLFUNC-BLOCK address={:#x} isa={:?} class={} words={:x?}", callback_address, callback_isa, env.objc.get_class_name(callback_isa), words);
            }
        }
        if sel_name.starts_with("addSubview") || sel_name.starts_with("insertSubview") || sel_name.starts_with("setFullscreen") || sel_name == "removeFromSuperview" || sel_name.starts_with("bringSubview") || sel_name.starts_with("sendSubview") {
            let r = env.cpu.regs();
            let arg_class = if sel_name.starts_with("setFullscreen") || r[2] == 0 || r[2] < 0x1000 { "-".to_string() } else { env.objc.get_class_name(ObjC::read_isa(crate::mem::Ptr::from_bits(r[2]), &env.mem)).to_string() };
            log!("VIEWOP {} {:?} {} arg={:#x}({}) r3={:#x} lr={:#x}", env.objc.get_class_name(class), receiver, sel_name, r[2], arg_class, r[3], r[14]);
        }
        if matches!(sel_name, "setEnabled:" | "setHidden:" | "setTitle:forState:" | "setImage:forState:" | "setBackgroundImage:forState:") && env.objc.get_class_name(class) == "UIButton" {
            let r = env.cpu.regs();
            log!("BTN {:?} {} arg={:#x} lr={:#x}", receiver, sel_name, r[2], r[14]);
        }
        if SEEN
            .lock()
            .unwrap()
            .get_or_insert_with(Default::default)
            .insert((class, selector))
        {
            log!(
                "MSGTRACE t{} [{} {}]",
                env.current_thread,
                env.objc.get_class_name(class),
                selector.as_str(&env.mem)
            );
        }
    }
    if crate::env_flag_cached!("TOUCHHLE_TRACE_MENU")
        && receiver != nil
        && selector.as_str(&env.mem) == "itemForTouch:"
    {
        let class = ObjC::read_isa(receiver, &env.mem);
        if env.objc.get_class_name(class) == "GameMenu" {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static MENU_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);
            if MENU_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 8 {
                let touch = id::from_bits(env.cpu.regs()[2]);
                trace_cocos_menu(env, receiver, touch);
            }
        }
    }
    // Host-side recursion guard. If an Objective-C method (typically
    // `hitTest:withEvent:` or `pointInside:withEvent:`) ends up recursing
    // into itself indirectly, the host call stack balloons because every
    // round trip goes host -> guest -> host. Without this guard that path
    // SIGSEGVs the whole emulator once the native stack is exhausted.
    //
    // We use a thread-local counter instead of tracking it in
    // `Environment`, since `objc_msgSend_inner` is the single chokepoint
    // through which every dispatch (host or guest) must pass.
    //
    // 128 is a deliberate compromise: real iOS view hierarchies rarely go
    // deeper than ~50 nested `nextResponder`/`hitTest:` levels, and a small
    // limit keeps us well clear of Android's 1 MB default thread stack
    // (each `objc_msgSend_inner` host frame is several KB once Rust adds
    // local variables, log!() temporaries, and the dispatch trampoline).
    const MAX_DEPTH: usize = 128;
    thread_local! {
        static DISPATCH_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }
    let depth = DISPATCH_DEPTH.with(|d| {
        let new = d.get() + 1;
        d.set(new);
        new
    });
    struct DepthGuard;
    impl Drop for DepthGuard {
        fn drop(&mut self) {
            DISPATCH_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        }
    }
    let _guard = DepthGuard;
    if depth > MAX_DEPTH {
        let sel_name = selector.as_str(&env.mem).to_string();

        if env.bundle.bundle_identifier() == "com.tayasui.mrpoofree"
            && (sel_name.contains("repaint")
                || sel_name.contains("draw")
                || sel_name.contains("displayLink")
                || sel_name.contains("startAnimation")
                || sel_name.contains("stopAnimation")
                || sel_name.contains("startRendering")
                || sel_name.contains("stopRendering")
                || sel_name.contains("presentRenderbuffer")
                || sel_name.contains("applicationDidBecomeActive")
                || sel_name.contains("setAnimationFrameInterval"))
        {
            log!(
                "[MR GOO MSG TRACE] receiver={:?} selector={}",
                receiver,
                sel_name
            );
        }
        log!(
            "Warning: objc_msgSend recursion limit ({}) exceeded while dispatching \"{}\" to {:?}; bailing out with a nil return.",
            MAX_DEPTH,
            sel_name,
            receiver,
        );
        // Special handling for allocWithZone: — if recursion is hit during
        // allocation, perform the allocation directly using NSObject's
        // fallback path rather than returning nil (which causes cascading
        // failures like "texture cannot be nil!" in games).
        if sel_name == "allocWithZone:" {
            let obj =
                env.objc
                    .alloc_object(receiver, Box::new(super::TrivialHostObject), &mut env.mem);
            env.cpu.regs_mut()[0] = obj.to_bits();
            return;
        }
        env.cpu.regs_mut()[0..2].fill(0);
        return;
    }

    let message_type_info = env.objc.message_type_info.take();

    if receiver == nil {
        // https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/ObjectiveC/Chapters/ocObjectsClasses.html#//apple_ref/doc/uid/TP30001163-CH11-SW7
        log_dbg!("[nil {}]", selector.as_str(&env.mem));
        env.cpu.regs_mut()[0..2].fill(0);
        return;
    }

    let orig_class = super2.unwrap_or_else(|| ObjC::read_isa(receiver, &env.mem));
    // Graceful exit if isa is nil — this typically means the object was
    // already deallocated (use-after-free in guest code) or was never
    // properly allocated. Per Apple's Objective-C runtime behavior,
    // messaging a deallocated object is undefined behavior, but we handle
    // it gracefully by returning nil/0 instead of crashing.
    if orig_class == nil {
        // Rate-limit these warnings to avoid flooding the log when the
        // guest app has a use-after-free bug that triggers repeatedly.
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NIL_ISA_COUNT: AtomicUsize = AtomicUsize::new(0);
        const NIL_ISA_LOG_LIMIT: usize = 8;
        let count = NIL_ISA_COUNT.fetch_add(1, Ordering::Relaxed);
        if count < NIL_ISA_LOG_LIMIT {
            log!(
                "Warning: receiver {:?} has nil isa! Ignoring message \"{}\". \
                 (This usually means the object was already freed — use-after-free \
                 in guest code.) [{}/{}]",
                receiver,
                selector.as_str(&env.mem),
                count + 1,
                NIL_ISA_LOG_LIMIT,
            );
        } else if count == NIL_ISA_LOG_LIMIT {
            log!(
                "Warning: suppressing further nil-isa warnings ({} already logged). \
                 The guest app has use-after-free bugs.",
                NIL_ISA_LOG_LIMIT,
            );
        }
        env.cpu.regs_mut()[0..2].fill(0);
        return;
    }

    if super2.is_none()
        && try_nsarray_indexed_subscript_interpose(env, receiver, selector, orig_class)
    {
        return;
    }

    if super2.is_none() && try_gdataxml_interpose(env, receiver, selector, orig_class) {
        return;
    }

    // Lazily dispatch `+initialize` to the receiver's class (and its
    // superclasses) before this message reaches its IMP. Skipped for super
    // calls — the calling class is already initialized by the time we reach
    // a `super` call site inside one of its methods.
    if super2.is_none() && !skip_initialize {
        let class_to_init = if let Some(host_object) = env.objc.get_host_object(orig_class) {
            if let Some(co) = host_object
                .as_any()
                .downcast_ref::<super::ClassHostObject>()
            {
                if co.is_metaclass {
                    // Class method: receiver itself is the class.
                    receiver
                } else {
                    // Instance method: orig_class is the class.
                    orig_class
                }
            } else {
                nil
            }
        } else {
            nil
        };
        if class_to_init != nil {
            ensure_class_initialized(env, class_to_init);
        }
    }

    // ULTRAHLE_MINIONJUMP_TAP_BRIDGE_BEGIN
    // Minion Jump / SheepEscape: map Cocos2D GrowButton/GrowStarButton objects
    // to their real target+selector callbacks. This is app-gated so Potato and
    // other games never see it.
    static ULTRAHLE_MINIONJUMP_BUTTON_CALLBACKS: std::sync::OnceLock<
        std::sync::Mutex<Vec<(u32, u32, u32, u32)>>,
    > = std::sync::OnceLock::new();
    static ULTRAHLE_MINIONJUMP_BUTTON_CALLBACK_REENTRY: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    static ULTRAHLE_MINIONJUMP_CURRENT_STAGE: std::sync::atomic::AtomicU32 =
        std::sync::atomic::AtomicU32::new(0);

    let ultrahle_minionjump_active = matches!(
        env.bundle.bundle_identifier(),
        "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro"
    );

    let ultrahle_minionjump_sel_name = if ultrahle_minionjump_active {
        selector.as_str(&env.mem).to_string()
    } else {
        String::new()
    };

    let ultrahle_minionjump_class_name = if ultrahle_minionjump_active {
        env.objc.get_class_name(orig_class).to_owned()
    } else {
        String::new()
    };

    let ultrahle_minionjump_release_arg0 = env.cpu.regs()[2];

    let mut ultrahle_minionjump_factory_should_map = false;
    let mut ultrahle_minionjump_factory_target: u32 = 0;
    let mut ultrahle_minionjump_factory_sel: u32 = 0;
    let mut ultrahle_minionjump_factory_stage: u32 = 0;
    let mut ultrahle_minionjump_factory_note = String::new();
    let mut ultrahle_minionjump_factory_clears_scene_map = false;

    if ultrahle_minionjump_active
        && ultrahle_minionjump_class_name == "GrowButton"
        && ultrahle_minionjump_sel_name == "buttonWithSprite:selectImage:target:selector:"
    {
        let sp = env.cpu.regs()[13];
        let sp_ptr = crate::mem::ConstPtr::<u8>::from_bits(sp);

        // r0=self/class, r1=_cmd, r2=sprite, r3=selectImage.
        // stack[0]=target, stack[1]=selector.
        ultrahle_minionjump_factory_target =
            u32::from_le_bytes(env.mem.bytes_at(sp_ptr, 4).try_into().unwrap());
        ultrahle_minionjump_factory_sel =
            u32::from_le_bytes(env.mem.bytes_at(sp_ptr + 4, 4).try_into().unwrap());

        if ultrahle_minionjump_factory_target != 0 && ultrahle_minionjump_factory_sel != 0 {
            let callback_sel_ptr =
                crate::mem::ConstPtr::<u8>::from_bits(ultrahle_minionjump_factory_sel);
            let callback_sel: crate::objc::SEL = unsafe { std::mem::transmute(callback_sel_ptr) };
            let callback_name = callback_sel.as_str(&env.mem).to_string();

            ultrahle_minionjump_factory_should_map = true;
            ultrahle_minionjump_factory_note = callback_name.clone();

            if callback_name == "playAction"
                || callback_name == "backAction"
                || callback_name == "selPause"
            {
                ultrahle_minionjump_factory_clears_scene_map = true;
            }

            log!(
                "UltraHLE MinionJump factory GrowButton target=0x{:08x} selector={}",
                ultrahle_minionjump_factory_target,
                callback_name
            );
        }
    }

    if ultrahle_minionjump_active
        && ultrahle_minionjump_class_name == "GrowStarButton"
        && ultrahle_minionjump_sel_name
            == "buttonWithSpriteFrame:selectframeName:stageNumber:starCount:locked:tag:target:selector:"
    {
        let sp = env.cpu.regs()[13];
        let sp_ptr = crate::mem::ConstPtr::<u8>::from_bits(sp);

        // r0=self/class, r1=_cmd, r2=spriteFrame, r3=selectframeName.
        // stack[0]=stageNumber, stack[1]=starCount, stack[2]=locked,
        // stack[3]=tag, stack[4]=target, stack[5]=selector.
        let stage_number = u32::from_le_bytes(env.mem.bytes_at(sp_ptr, 4).try_into().unwrap());
        let _star_count = u32::from_le_bytes(env.mem.bytes_at(sp_ptr + 4, 4).try_into().unwrap());
        let mut locked = u32::from_le_bytes(env.mem.bytes_at(sp_ptr + 8, 4).try_into().unwrap());
        let _tag = u32::from_le_bytes(env.mem.bytes_at(sp_ptr + 12, 4).try_into().unwrap());

        if stage_number <= 25 && locked != 0 {
            let locked_arg_ptr = crate::mem::MutPtr::<u8>::from_bits(sp + 8);
            env.mem
                .bytes_at_mut(locked_arg_ptr, 4)
                .copy_from_slice(&0u32.to_le_bytes());
            locked = 0;
            log!(
                "UltraHLE MinionJump: forced stage {} unlocked in GrowStarButton factory",
                stage_number
            );
        }

        ultrahle_minionjump_factory_target =
            u32::from_le_bytes(env.mem.bytes_at(sp_ptr + 16, 4).try_into().unwrap());
        ultrahle_minionjump_factory_sel =
            u32::from_le_bytes(env.mem.bytes_at(sp_ptr + 20, 4).try_into().unwrap());

        if ultrahle_minionjump_factory_target != 0 && ultrahle_minionjump_factory_sel != 0 {
            let callback_sel_ptr =
                crate::mem::ConstPtr::<u8>::from_bits(ultrahle_minionjump_factory_sel);
            let callback_sel: crate::objc::SEL = unsafe { std::mem::transmute(callback_sel_ptr) };
            let callback_name = callback_sel.as_str(&env.mem).to_string();

            log!(
                "UltraHLE MinionJump factory GrowStarButton stage={} locked={} target=0x{:08x} selector={}",
                stage_number,
                locked,
                ultrahle_minionjump_factory_target,
                callback_name
            );

            // ULTRAHLE_MINIONJUMP_CLEAR_ON_STAGE1_BEGIN
            if stage_number == 1 && callback_name == "selectLVAction:" {
                let map = ULTRAHLE_MINIONJUMP_BUTTON_CALLBACKS
                    .get_or_init(|| std::sync::Mutex::new(Vec::new()));
                map.lock().unwrap().clear();
                log!("UltraHLE MinionJump: cleared stale level-select callback map at stage 1 factory");
            }
            // ULTRAHLE_MINIONJUMP_CLEAR_ON_STAGE1_END

            if locked == 0 && callback_name == "selectLVAction:" {
                ultrahle_minionjump_factory_should_map = true;
                ultrahle_minionjump_factory_stage = stage_number;
                ultrahle_minionjump_factory_note =
                    format!("stage{}:{}", stage_number, callback_name);
            }
        }
    }

    if ultrahle_minionjump_active
        && (ultrahle_minionjump_sel_name == "getCurrentStage"
            || ultrahle_minionjump_sel_name == "currentstage"
            || ultrahle_minionjump_sel_name == "currentStage")
    {
        let selected_stage_index: u32 = std::env::var("ULTRAHLE_MINIONJUMP_SELECTED_STAGE_INDEX")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        log!(
            "UltraHLE MinionJump: overriding {} -> {}",
            ultrahle_minionjump_sel_name,
            selected_stage_index
        );

        env.cpu.regs_mut()[0] = selected_stage_index;
        env.cpu.regs_mut()[1] = 0;
        return;
    }
    // ULTRAHLE_MINIONJUMP_TAP_BRIDGE_END

    // Traverse the chain of superclasses to find the method implementation.
    let mut class = orig_class;
    loop {
        if class == nil {
            assert!(class != orig_class);
            let (class_name_for_log, is_metaclass) = {
                let class_host_object = env.objc.get_host_object(orig_class).unwrap();
                let class_host_object = class_host_object
                    .as_any()
                    .downcast_ref::<super::ClassHostObject>()
                    .unwrap();
                (
                    class_host_object.name.clone(),
                    class_host_object.is_metaclass,
                )
            };

            // `+[Class self]` must return the class itself. The real objc
            // runtime resolves `self` via the metaclass root NSObject chain,
            // but a bare metaclass registered for an unimplemented class
            // falls through here and previously returned 0 — breaking e.g.
            // the Burstly ad SDK which calls `+[BurstlyCurrency... self]`.
            if selector.as_str(&env.mem) == "self" {
                env.cpu.regs_mut()[0] = receiver.to_bits();
                env.cpu.regs_mut()[1] = 0;
                return;
            }

            let missing_selector_name = selector.as_str(&env.mem).to_owned();

            if try_cocos_missing_selector_compat(
                env,
                receiver,
                &missing_selector_name,
                &class_name_for_log,
                is_metaclass,
            ) {
                return;
            }

            // --- ИСПРАВЛЕНИЕ ЗДЕСЬ: заменили panic! на log! (мягкий фейл
            // форка) ---
            //
            // Rate-limit per unique (class, selector) pair. Some apps poll an
            // unimplemented selector (e.g. `-[CMDeviceMotion attitude]`) every
            // single frame; logging unconditionally formatted a string and
            // wrote it to the log file 60+ times per second, which by itself
            // tanked emulation performance and bloated the log to megabytes.
            // We log the first few occurrences of each distinct pair so the
            // warning is still discoverable, then go quiet for that pair.
            {
                use std::collections::HashMap;
                use std::sync::Mutex;
                use std::sync::OnceLock;

                const PER_PAIR_LOG_LIMIT: usize = 3;
                static SEEN: OnceLock<Mutex<HashMap<(String, String), usize>>> = OnceLock::new();

                let sel_str = selector.as_str(&env.mem).to_owned();
                let key = (class_name_for_log.clone(), sel_str.clone());
                let seen = SEEN.get_or_init(|| Mutex::new(HashMap::new()));
                let count = {
                    let mut map = seen.lock().unwrap();
                    let entry = map.entry(key).or_insert(0);
                    *entry += 1;
                    *entry
                };

                if count <= PER_PAIR_LOG_LIMIT {
                    log!(
                        "Warning: {} {:?} ({}class \"{}\", {:?}){} does not respond to selector \"{}\"! Returning 0.{}",
                        if is_metaclass { "Class" } else { "Object" },
                        receiver,
                        if is_metaclass { "meta" } else { "" },
                        class_name_for_log,
                        orig_class,
                        if super2.is_some() {
                            "'s superclass"
                        } else {
                            ""
                        },
                        sel_str,
                        if count == PER_PAIR_LOG_LIMIT {
                            " (suppressing further warnings for this selector)"
                        } else {
                            ""
                        },
                    );
                }
            }

            // Имитируем возврат nil/0, чтобы приложение продолжило работу
            env.cpu.regs_mut()[0..2].fill(0);
            return;
            // ------------------------------------------------------------
        }

        let Some(host_object) = env.objc.get_host_object(class) else {
            log_dbg!(
                "Warning: class {:?} in superclass chain of {:?} has no host object — stopping dispatch",
                class, receiver
            );
            env.cpu.regs_mut()[0..2].fill(0);
            return;
        };

        if let Some(&super::ClassHostObject {
            superclass,
            ref methods,
            ref name,
            ..
        }) = host_object.as_any().downcast_ref()
        {
            // Skip method lookup on first iteration if this is the super-call
            // variant of objc_msgSend (look up the superclass first)
            if super2.is_some() && class == orig_class {
                class = superclass;
                continue;
            }

            if let Some(imp) = methods.get(&selector) {
                log_dbg!("Found method on: {}", name);
                match imp {
                    IMP::Host(host_imp) => {
                        // TODO: do type checks when calling GuestIMPs too.
                        // That requires using Objective-C type strings,
                        // rather than Rust types, and should probably
                        // warn rather than panicking,
                        // because apps might rely on type punning.
                        if let Some((sent_type_id, sent_type_desc)) = message_type_info {
                            let (expected_type_id, expected_type_desc) = host_imp.type_info();
                            if sent_type_id != expected_type_id {
                                let msg = format!(
                                    "\
Type mismatch when sending message {} to {:?}!
- Message has type: {:?} / {}
- Method expects type: {:?} / {}",
                                    selector.as_str(&env.mem),
                                    receiver,
                                    sent_type_id,
                                    sent_type_desc,
                                    expected_type_id,
                                    expected_type_desc
                                );
                                if tolerate_type_mismatch {
                                    log!("Warning: {}", msg);
                                } else {
                                    log_dbg!("{}", msg); // Мягкий фейл, чтобы не падать
                                }
                            }
                        }
                        host_imp.call_from_guest(env)
                    }
                    // We can't create a new stack frame, because that would
                    // interfere with pass-through of stack arguments.
                    IMP::Guest(guest_imp) => {
                        if crate::env_flag_cached!("TOUCHHLE_TRACE_COCOS_SCALE") {
                            let selector_name = selector.as_str(&env.mem);
                            let class_name = env.objc.get_class_name(class).to_owned();
                            if class_name.starts_with("CC")
                                && matches!(
                                    selector_name.as_ref(),
                                    "setScale:"
                                        | "setScaleX:"
                                        | "setScaleY:"
                                        | "setScaleX:Y:"
                                        | "setPosition:"
                                        | "setAnchorPoint:"
                                        | "setContentSize:"
                                        | "setRelativeScaleX:Y:type:propertyName:"
                                        | "setRelativePosition:type:parentSize:propertyName:"
                                )
                            {
                                use std::sync::atomic::{AtomicU32, Ordering};
                                static SEEN: AtomicU32 = AtomicU32::new(0);
                                let n = SEEN.fetch_add(1, Ordering::Relaxed);
                                if n < 120 {
                                    let regs = env.cpu.regs();
                                    log!(
                                        "COCOS-SCALE #{} {} {:?} {} r2={:#x}({}) r3={:#x}({}) r4={:#x}({})",
                                        n,
                                        class_name,
                                        receiver,
                                        selector_name,
                                        regs[2],
                                        f32::from_bits(regs[2]),
                                        regs[3],
                                        f32::from_bits(regs[3]),
                                        regs[4],
                                        f32::from_bits(regs[4])
                                    );
                                }
                            }
                        }
                        guest_imp.call_without_pushing_stack_frame(env)
                    }
                }

                // ULTRAHLE_MINIONJUMP_TAP_POSTCALL_BEGIN
                if ultrahle_minionjump_active && ultrahle_minionjump_factory_should_map {
                    let returned_button = env.cpu.regs()[0];
                    if returned_button != 0 {
                        let map = ULTRAHLE_MINIONJUMP_BUTTON_CALLBACKS
                            .get_or_init(|| std::sync::Mutex::new(Vec::new()));
                        let mut map = map.lock().unwrap();

                        if ultrahle_minionjump_factory_clears_scene_map {
                            map.clear();
                            log!(
                                "UltraHLE MinionJump: cleared button callback map for selector={}",
                                ultrahle_minionjump_factory_note
                            );
                        }

                        map.retain(|(button, _, _, _)| *button != returned_button);
                        map.push((
                            returned_button,
                            ultrahle_minionjump_factory_target,
                            ultrahle_minionjump_factory_sel,
                            ultrahle_minionjump_factory_stage,
                        ));

                        log!(
                            "UltraHLE MinionJump: mapped button=0x{:08x} -> target=0x{:08x} selector={} stage={}",
                            returned_button,
                            ultrahle_minionjump_factory_target,
                            ultrahle_minionjump_factory_note,
                            ultrahle_minionjump_factory_stage
                        );
                    }
                }

                if ultrahle_minionjump_active
                    && (ultrahle_minionjump_class_name == "GrowButton"
                        || ultrahle_minionjump_class_name == "GrowStarButton")
                    && ultrahle_minionjump_sel_name == "animateFocusLoseMenuItem:"
                {
                    let receiver_bits = receiver.to_bits();
                    let arg_button_bits = ultrahle_minionjump_release_arg0;
                    let mapped = {
                        let map = ULTRAHLE_MINIONJUMP_BUTTON_CALLBACKS
                            .get_or_init(|| std::sync::Mutex::new(Vec::new()));
                        let map = map.lock().unwrap();
                        map.iter()
                            .find(|(button, _, _, _)| {
                                *button == receiver_bits || *button == arg_button_bits
                            })
                            .copied()
                    };

                    if let Some((mapped_button, target_raw, sel_raw, mapped_stage)) = mapped {
                        if target_raw != 0
                            && sel_raw != 0
                            && !ULTRAHLE_MINIONJUMP_BUTTON_CALLBACK_REENTRY
                                .swap(true, std::sync::atomic::Ordering::Relaxed)
                        {
                            let callback_sel_ptr = crate::mem::ConstPtr::<u8>::from_bits(sel_raw);
                            let callback_sel: crate::objc::SEL =
                                unsafe { std::mem::transmute(callback_sel_ptr) };
                            let callback_name = callback_sel.as_str(&env.mem).to_string();

                            let sender_bits =
                                if mapped_stage != 0 && callback_name == "selectLVAction:" {
                                    mapped_button
                                } else {
                                    ultrahle_minionjump_release_arg0
                                };

                            if mapped_stage != 0 && callback_name == "selectLVAction:" {
                                let stage_index = mapped_stage.saturating_sub(1);
                                ULTRAHLE_MINIONJUMP_CURRENT_STAGE
                                    .store(mapped_stage, std::sync::atomic::Ordering::Relaxed);

                                std::env::set_var(
                                    "ULTRAHLE_MINIONJUMP_SELECTED_STAGE_INDEX",
                                    format!("{}", stage_index),
                                );

                                // ULTRAHLE_MINIONJUMP_DUAL_TAG_BEGIN
                                let set_tag_sel = env
                                    .objc
                                    .register_host_selector("setTag:".to_string(), &mut env.mem);
                                let level_tag = stage_index as i32;

                                // Force the tag onto both the original release sender and the
                                // mapped GrowStarButton. On rebuilt level-select scenes, one can
                                // have the stale tag while the other is the object selectLVAction
                                // actually reads.
                                let release_sender =
                                    id::from_bits(ultrahle_minionjump_release_arg0);
                                let mapped_sender = id::from_bits(mapped_button);

                                let _: () = msg_send_no_type_checking(
                                    env,
                                    (release_sender, set_tag_sel, level_tag),
                                );
                                if mapped_sender != release_sender {
                                    let _: () = msg_send_no_type_checking(
                                        env,
                                        (mapped_sender, set_tag_sel, level_tag),
                                    );
                                }

                                log!(
                                    "UltraHLE MinionJump: selected stage {} index {} mapped_sender=0x{:08x} release_sender=0x{:08x} tag={}",
                                    mapped_stage,
                                    stage_index,
                                    mapped_button,
                                    ultrahle_minionjump_release_arg0,
                                    level_tag
                                );
                                // ULTRAHLE_MINIONJUMP_DUAL_TAG_END
                            }

                            log!(
                                "UltraHLE MinionJump: queued mapped button=0x{:08x} target=0x{:08x} selector={} sender=0x{:08x} stage={}",
                                mapped_button,
                                target_raw,
                                callback_name,
                                sender_bits,
                                mapped_stage
                            );

                            std::env::set_var(
                                "ULTRAHLE_MINIONJUMP_PENDING_TARGET",
                                format!("{}", target_raw),
                            );
                            std::env::set_var(
                                "ULTRAHLE_MINIONJUMP_PENDING_SEL",
                                format!("{}", sel_raw),
                            );
                            std::env::set_var(
                                "ULTRAHLE_MINIONJUMP_PENDING_SENDER",
                                format!("{}", sender_bits),
                            );
                            std::env::set_var(
                                "ULTRAHLE_MINIONJUMP_PENDING_CALLBACK",
                                callback_name,
                            );
                            std::env::set_var(
                                "ULTRAHLE_MINIONJUMP_PENDING_STAGE",
                                format!("{}", mapped_stage),
                            );

                            ULTRAHLE_MINIONJUMP_BUTTON_CALLBACK_REENTRY
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                        }
                    } else {
                        log!(
                            "UltraHLE MinionJump: no mapped callback for released {} receiver=0x{:08x} arg0=0x{:08x}",
                            ultrahle_minionjump_class_name,
                            receiver_bits,
                            arg_button_bits
                        );
                    }
                }
                // ULTRAHLE_MINIONJUMP_TAP_POSTCALL_END

                return;
            } else {
                class = superclass;
            }
        } else if let Some((class_name_for_log, is_metaclass)) = host_object
            .as_any()
            .downcast_ref::<super::UnimplementedClass>()
            .map(|co| (co.name.clone(), co.is_metaclass))
        {
            let sel_name = selector.as_str(&env.mem).to_owned();
            if try_cocos_missing_selector_compat(
                env,
                receiver,
                &sel_name,
                &class_name_for_log,
                is_metaclass,
            ) {
                return;
            }
            // XaView A8 fix (2e1549e3): messages to the unimplemented
            // GCController class behave as if sent to nil, instead of
            // panicking the guest.
            if class_name_for_log == "GCController" {
                env.cpu.regs_mut()[0..2].fill(0);
                return;
            }
            log!(
                "Class \"{}\" ({:?}) is unimplemented. Call to {} method \"{}\".",
                class_name_for_log,
                class,
                if is_metaclass { "class" } else { "instance" },
                sel_name,
            );
            env.cpu.regs_mut()[0..2].fill(0);
            return;
        } else if let Some((class_name_for_log, is_metaclass)) = host_object
            .as_any()
            .downcast_ref::<super::FakeClass>()
            .map(|co| (co.name.clone(), co.is_metaclass))
        {
            let sel_name = selector.as_str(&env.mem).to_owned();
            if let Some(result) =
                fake_openfeint_should_autorotate(env, &class_name_for_log, is_metaclass, &sel_name)
            {
                env.cpu.regs_mut()[0] = result.into();
                env.cpu.regs_mut()[1] = 0;
                return;
            }
            if try_cocos_missing_selector_compat(
                env,
                receiver,
                &sel_name,
                &class_name_for_log,
                is_metaclass,
            ) {
                return;
            }
            if let Some(result) =
                fake_class_message_result(&class_name_for_log, is_metaclass, &sel_name)
            {
                log!(
                    "Call to faked class \"{}\" ({:?}) {} method \"{}\". Returning {}.",
                    class_name_for_log,
                    class,
                    if is_metaclass { "class" } else { "instance" },
                    sel_name,
                    result,
                );
                env.cpu.regs_mut()[0] = result;
                env.cpu.regs_mut()[1] = 0;
                return;
            }
            log!(
                "Call to faked class \"{}\" ({:?}) {} method \"{}\". Behaving as if message was sent to nil.",
                class_name_for_log,
                class,
                if is_metaclass { "class" } else { "instance" },
                sel_name,
            );
            env.cpu.regs_mut()[0..2].fill(0);
            return;
        } else {
            let sel_name = selector.as_str(&env.mem).to_owned();
            let class_name = env.objc.try_get_class_name(class).map(|s| s.to_owned());
            let orig_name = env.objc.try_get_class_name(orig_class).map(|s| s.to_owned());
            let type_name = host_object.type_name();
            log!(
                "Item {class:?} ({:?}) in superclass chain of object {receiver:?}'s class {orig_class:?} ({:?}) has an unexpected host object type ({}). Selector: \"{}\". Behaving as if message was sent to nil.",
                class_name, orig_name, type_name, sel_name
            );
            env.cpu.regs_mut()[0..2].fill(0);
            return;
        }
    }
}

// ============================================================================
// Broad Cocos2D / Cocos2d-x / Unity missing-selector compatibility
// ============================================================================
// Old iOS Cocos and Unity-era iOS games often mix framework versions, categories, optional
// protocols, and per-game subclasses. Returning a sane default for a missing
// selector is usually closer to Objective-C's loose runtime behavior than
// panicking or spamming the log. These shims are deliberately conservative:
// they only trigger for Cocos/Unity-looking classes/selectors or harmless UIKit-style
// lifecycle/property selectors.

fn objc_ret_zero(env: &mut Environment) {
    env.cpu.regs_mut()[0..2].fill(0);
}

fn objc_ret_u32(env: &mut Environment, value: u32) {
    env.cpu.regs_mut()[0] = value;
    env.cpu.regs_mut()[1] = 0;
}

/// Return value for a message to a faked third-party SDK class (see
/// `substitute_classes`), when behaving as if it were sent to nil (returning
/// 0) would put the app into a state the real SDK never leaves it in.
fn fake_class_message_result(class_name: &str, is_metaclass: bool, selector: &str) -> Option<u32> {
    match (class_name, is_metaclass, selector) {
        // Act as if the player has already answered OpenFeint's opt-in
        // prompt and declined (so `hasUserApprovedFeint` stays NO). "Not
        // answered yet" makes games bring up their OpenFeint approval
        // screen and wait for it to be dismissed, which never happens with
        // OpenFeint faked: Atlantis Sky Patrol spins its run loop forever
        // inside a frame, freezing on the splash screen.
        ("OpenFeint", true, "hasUserSetFeintAccess") => Some(1),
        _ => None,
    }
}

/// `+[OpenFeint shouldAutorotateToInterfaceOrientation:
/// withSupportedOrientations:andCount:]` answers whether the orientation is
/// in the list the app passes. OpenFeint games forward their view
/// controller's `shouldAutorotateToInterfaceOrientation:` to it (the cocos2d
/// RootViewController template does), so answering NO, as a nil receiver
/// would, stops UIKit from ever rotating them to landscape. Reiner Knizia's
/// Ra then draws its landscape scenes into a portrait view.
fn fake_openfeint_should_autorotate(
    env: &mut Environment,
    class_name: &str,
    is_metaclass: bool,
    selector: &str,
) -> Option<bool> {
    if (class_name, is_metaclass, selector)
        != (
            "OpenFeint",
            true,
            "shouldAutorotateToInterfaceOrientation:withSupportedOrientations:andCount:",
        )
    {
        return None;
    }
    let regs = env.cpu.regs();
    let (orientation, list, sp) = (regs[2], regs[3], regs[crate::cpu::Cpu::SP]);
    let count: u32 = env.mem.read(ConstPtr::from_bits(sp));
    // Real apps pass the four interface orientations at most; the bound only
    // guards against reading garbage if the arguments are bogus.
    let supported = list != 0
        && (0..count.min(8)).any(|i| {
            let item: u32 = env.mem.read(ConstPtr::from_bits(list + i * 4));
            item == orientation
        });
    Some(supported)
}

fn objc_ret_f32(env: &mut Environment, value: f32) {
    env.cpu.regs_mut()[0] = value.to_bits();
    env.cpu.regs_mut()[1] = 0;
}

fn objc_ret_f64(env: &mut Environment, value: f64) {
    let bits = value.to_bits();
    env.cpu.regs_mut()[0] = bits as u32;
    env.cpu.regs_mut()[1] = (bits >> 32) as u32;
}

fn objc_ret_id(env: &mut Environment, value: id) {
    env.cpu.regs_mut()[0] = value.to_bits();
    env.cpu.regs_mut()[1] = 0;
}

fn cocos_selector_log_once(class_name: &str, selector: &str, note: &str) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<(String, String, String)>>> = OnceLock::new();
    let key = (
        class_name.to_string(),
        selector.to_string(),
        note.to_string(),
    );
    let seen = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    if seen.lock().unwrap().insert(key) {
        log_dbg!(
            "Engine compat: class={} missing selector={} -> {}",
            class_name,
            selector,
            note
        );
    }
}

fn is_cocos_like_class_name(name: &str) -> bool {
    name.starts_with("CC")
        || name.starts_with("CCT")
        || name.starts_with("Cocos")
        || name.contains("Cocos")
        || name.contains("cocos")
        || name.contains("EAGL")
        || name.contains("GLView")
        || name.contains("Unity")
        || name.contains("UnityView")
        || name.contains("UnityAppController")
        || name.contains("UnityViewController")
        || name.contains("UnityDefaultViewController")
        || name.contains("UnityKeyboard")
        || name.contains("UnityDisplay")
        || name.contains("DisplayConnection")
        || name.contains("DisplayManager")
        || name.contains("RenderView")
        || name.contains("RenderingView")
        || name.contains("Director")
        || name.contains("Scene")
        || name.contains("Layer")
        || name.contains("Sprite")
        || name.contains("Menu")
        || name.contains("Button")
        || name.contains("Label")
        || name.contains("Texture")
        || name.contains("Atlas")
        || name.contains("TileMap")
        || name.contains("TMX")
        || name.contains("Particle")
        || name.contains("Action")
        || name.contains("Scheduler")
        || name.contains("TouchDispatcher")
        || name.contains("GameLayer")
        || name.contains("GameScene")
        || name.contains("RootView")
        || name.contains("GameView")
        || name.contains("GameController")
        || name.contains("AppController")
}

fn is_unity_like_selector_name(sel: &str) -> bool {
    sel.contains("Unity")
        || sel.contains("unity")
        || sel.contains("Render")
        || sel.contains("render")
        || sel.contains("DisplayLink")
        || sel.contains("displayLink")
        || sel.contains("repaint")
        || sel.contains("orientation")
        || sel.contains("Orientation")
        || sel.contains("keyboard")
        || sel.contains("Keyboard")
        || sel.starts_with("touches")
        || sel.starts_with("motion")
        || sel.starts_with("accelerometer")
}

fn is_cocos_selector_name(sel: &str) -> bool {
    sel.starts_with("cc")
        || sel.starts_with("CCTouch")
        || sel.starts_with("touchDispatcher")
        || sel.contains("Touch")
        || sel.contains("touch")
        || sel.starts_with("touches")
        || is_unity_like_selector_name(sel)
        || sel.contains("Action")
        || sel.contains("action")
        || sel.contains("Scheduler")
        || sel.contains("schedule")
        || sel.contains("unschedule")
        || sel.contains("Animation")
        || sel.contains("animation")
        || sel.contains("Texture")
        || sel.contains("texture")
        || sel.contains("Sprite")
        || sel.contains("sprite")
        || sel.contains("BlendFunc")
        || sel.contains("Opacity")
        || sel.contains("Color")
        || sel.contains("AnchorPoint")
        || sel.contains("Position")
        || sel.contains("ZOrder")
        || sel.contains("VertexZ")
        || sel.contains("Scale")
        || sel.contains("Rotation")
        || sel.contains("Skew")
        || sel.contains("Flip")
        || sel.contains("Visible")
        || sel.contains("Cascade")
        || sel.contains("contentSize")
        || sel.contains("ContentSize")
}

fn is_safe_uikit_lifecycle_selector(sel: &str) -> bool {
    matches!(
        sel,
        "viewDidLoad"
            | "viewDidUnload"
            | "viewWillAppear:"
            | "viewDidAppear:"
            | "viewWillDisappear:"
            | "viewDidDisappear:"
            | "viewWillLayoutSubviews"
            | "viewDidLayoutSubviews"
            | "didReceiveMemoryWarning"
            | "loadView"
            | "willMoveToSuperview:"
            | "didMoveToSuperview"
            | "willMoveToWindow:"
            | "didMoveToWindow"
            | "didAddSubview:"
            | "willRemoveSubview:"
            | "layoutSubviews"
            | "setNeedsLayout"
            | "layoutIfNeeded"
            | "setNeedsDisplay"
            | "setNeedsDisplayInRect:"
            | "drawRect:"
            | "applicationWillResignActive:"
            | "applicationDidBecomeActive:"
            | "applicationDidEnterBackground:"
            | "applicationWillEnterForeground:"
            | "applicationWillTerminate:"
            | "applicationDidReceiveMemoryWarning:"
            | "application:didFinishLaunchingWithOptions:"
            | "applicationDidFinishLaunching:"
            | "applicationWillResignActive"
            | "applicationDidBecomeActive"
            | "applicationDidEnterBackground"
            | "applicationWillEnterForeground"
            | "willRotateToInterfaceOrientation:duration:"
            | "didRotateFromInterfaceOrientation:"
            | "willAnimateRotationToInterfaceOrientation:duration:"
            | "touchesBegan:withEvent:"
            | "touchesMoved:withEvent:"
            | "touchesEnded:withEvent:"
            | "touchesCancelled:withEvent:"
    )
}

fn try_cocos_missing_selector_compat(
    env: &mut Environment,
    receiver: id,
    selector_name: &str,
    class_name: &str,
    is_metaclass: bool,
) -> bool {
    let cocosish = is_cocos_like_class_name(class_name) || is_cocos_selector_name(selector_name);
    let unityish = class_name.contains("Unity")
        || class_name.contains("RenderView")
        || class_name.contains("DisplayConnection")
        || is_unity_like_selector_name(selector_name);
    let engineish = cocosish || unityish;
    let safe_uikit = is_safe_uikit_lifecycle_selector(selector_name);
    if !engineish && !safe_uikit {
        return false;
    }

    // Lifecycle / scheduler / optional callback selectors: missing means no-op.
    if safe_uikit
        || selector_name.starts_with("set")
        || selector_name.starts_with("add")
        || selector_name.starts_with("remove")
        || selector_name.starts_with("schedule")
        || selector_name.starts_with("unschedule")
        || selector_name.starts_with("register")
        || selector_name.starts_with("unregister")
        || selector_name.starts_with("resume")
        || selector_name.starts_with("pause")
        || selector_name.starts_with("stop")
        || selector_name.starts_with("cleanup")
        || selector_name.starts_with("reorder")
        || selector_name.starts_with("sort")
        || selector_name.starts_with("visit")
        || selector_name.starts_with("draw")
        || selector_name.starts_with("update")
        || selector_name.starts_with("onEnter")
        || selector_name.starts_with("onExit")
        || selector_name.starts_with("onEnterTransitionDidFinish")
        || selector_name.starts_with("onExitTransitionDidStart")
        || selector_name.starts_with("ccTouches")
        || selector_name == "ccTouchMoved:withEvent:"
        || selector_name == "ccTouchEnded:withEvent:"
        || selector_name == "ccTouchCancelled:withEvent:"
        || selector_name == "accelerometer:didAccelerate:"
        || selector_name == "startUnity:"
        || selector_name == "startUnity"
        || selector_name == "preStartUnity"
        || selector_name == "quitUnity"
        || selector_name == "pauseUnity:"
        || selector_name == "resumeUnity"
        || selector_name == "createUI"
        || selector_name == "createViewHierarchy"
        || selector_name == "createViewHierarchyImpl"
        || selector_name == "createDisplayLink"
        || selector_name == "destroyDisplayLink"
        || selector_name == "repaint"
        || selector_name == "repaintDisplayLink"
        || selector_name == "render"
        || selector_name == "renderFrame"
        || selector_name == "keyboardWillShow:"
        || selector_name == "keyboardWillHide:"
        || selector_name == "keyboardDidShow:"
        || selector_name == "keyboardDidHide:"
        || selector_name == "keyBackClicked"
        || selector_name == "keyMenuClicked"
    {
        cocos_selector_log_once(class_name, selector_name, "void/no-op");
        objc_ret_zero(env);
        return true;
    }

    // Targeted touch began returns BOOL. Returning YES lets old CCTargetedTouch
    // delegates claim the touch when the selector is optional/missing on a shim.
    if selector_name == "ccTouchBegan:withEvent:" || selector_name == "containsTouchLocation:" {
        cocos_selector_log_once(class_name, selector_name, "YES");
        objc_ret_u32(env, 1);
        return true;
    }

    // Common YES defaults that keep active Cocos nodes alive.
    if matches!(
        selector_name,
        "isRunning"
            | "running"
            | "isVisible"
            | "visible"
            | "isEnabled"
            | "enabled"
            | "isTouchEnabled"
            | "touchEnabled"
            | "isAccelerometerEnabled"
            | "accelerometerEnabled"
            | "isKeyboardEnabled"
            | "keyboardEnabled"
            | "isUserInteractionEnabled"
            | "userInteractionEnabled"
            | "isRelativeAnchorPoint"
            | "relativeAnchorPoint"
            | "shouldAutorotate"
            | "shouldAutorotateToInterfaceOrientation:"
            | "prefersStatusBarHidden"
            | "isViewLoaded"
            | "shouldAutorotateToInterfaceOrientation"
            | "shouldUseAutorotation"
            | "unityIsReady"
            | "isUnityReady"
            | "requiresOpenGLES2"
            | "shouldAttachRenderDelegate"
            | "isActive"
            | "active"
            | "acceptsFirstResponder"
            | "canBecomeFirstResponder"
    ) {
        cocos_selector_log_once(class_name, selector_name, "YES");
        objc_ret_u32(env, 1);
        return true;
    }

    // Common NO defaults for optional Cocos flags.
    if matches!(
        selector_name,
        "isCascadeColorEnabled"
            | "cascadeColorEnabled"
            | "isCascadeOpacityEnabled"
            | "cascadeOpacityEnabled"
            | "isOpacityModifyRGB"
            | "doesOpacityModifyRGB"
            | "ignoreAnchorPointForPosition"
            | "isIgnoreAnchorPointForPosition"
            | "isFlipX"
            | "flipX"
            | "isFlipY"
            | "flipY"
            | "isTextureRectRotated"
            | "textureRectRotated"
            | "isDirty"
            | "dirty"
            | "isClosed"
            | "closed"
            | "isPaused"
            | "paused"
            | "isSelected"
            | "selected"
            | "hasActions"
            | "hasVisibleParents"
            | "isKeyboardShown"
            | "isKeyboardVisible"
            | "keyboardVisible"
            | "isShowingKeyboard"
            | "useAnimatedAutorotation"
            | "isRenderingPaused"
            | "renderingPaused"
    ) {
        cocos_selector_log_once(class_name, selector_name, "NO");
        objc_ret_zero(env);
        return true;
    }

    // Integer-ish defaults.
    if matches!(
        selector_name,
        "tag"
            | "zOrder"
            | "getZOrder"
            | "vertexZ"
            | "orderOfArrival"
            | "priority"
            | "touchPriority"
            | "handlerPriority"
            | "atlasIndex"
            | "numberOfRunningActions"
            | "runningActions"
            | "count"
            | "childrenCount"
            | "opacity"
            | "displayedOpacity"
            | "getOpacity"
            | "getDisplayedOpacity"
            | "level"
            | "stage"
            | "state"
            | "orientation"
            | "supportedInterfaceOrientations"
            | "application:supportedInterfaceOrientationsForWindow:"
            | "supportedInterfaceOrientationsForWindow:"
            | "interfaceOrientation"
            | "statusBarOrientation"
            | "currentOrientation"
            | "targetFrameRate"
            | "preferredFramesPerSecond"
            | "renderingAPI"
            | "graphicsDeviceType"
            | "displayIndex"
    ) {
        let value = match selector_name {
            "opacity" | "displayedOpacity" | "getOpacity" | "getDisplayedOpacity" => 255,
            "supportedInterfaceOrientations"
            | "application:supportedInterfaceOrientationsForWindow:"
            | "supportedInterfaceOrientationsForWindow:" => 0x18, // landscape left/right mask on old UIKit-style callers.
            "targetFrameRate" | "preferredFramesPerSecond" => 60,
            _ => 0,
        };
        cocos_selector_log_once(class_name, selector_name, "integer default");
        objc_ret_u32(env, value);
        return true;
    }

    // Float / CGFloat defaults.
    if matches!(
        selector_name,
        "scale"
            | "scaleX"
            | "scaleY"
            | "getScale"
            | "getScaleX"
            | "getScaleY"
            | "contentScaleFactor"
            | "screenScale"
            | "getContentScaleFactor"
            | "nativeScale"
            | "renderScale"
            | "dpiScale"
    ) {
        cocos_selector_log_once(class_name, selector_name, "1.0f");
        objc_ret_f32(env, 1.0);
        return true;
    }
    if matches!(
        selector_name,
        "rotation"
            | "rotationX"
            | "rotationY"
            | "skewX"
            | "skewY"
            | "getRotation"
            | "getRotationX"
            | "getRotationY"
            | "getSkewX"
            | "getSkewY"
            | "anchorPointInPoints"
            | "positionX"
            | "positionY"
            | "getPositionX"
            | "getPositionY"
            | "cameraEyeX"
            | "cameraEyeY"
            | "cameraEyeZ"
    ) {
        cocos_selector_log_once(class_name, selector_name, "0.0f");
        objc_ret_f32(env, 0.0);
        return true;
    }
    if matches!(
        selector_name,
        "animationInterval" | "dt" | "delta" | "timeScale" | "fixedDeltaTime"
    ) {
        let value = if selector_name == "animationInterval" {
            1.0 / 60.0
        } else if selector_name == "timeScale" {
            1.0
        } else {
            0.0
        };
        cocos_selector_log_once(class_name, selector_name, "double default");
        objc_ret_f64(env, value);
        return true;
    }

    // Return self for chainable init/autorelease-ish Cocos factories when they
    // are missing on a fake/unimplemented class. For class methods, returning
    // the class itself is worse than nil, so only do this for instances.
    if !is_metaclass
        && matches!(
            selector_name,
            "init" | "initWithCoder:" | "autorelease" | "retain"
        )
    {
        cocos_selector_log_once(class_name, selector_name, "self");
        objc_ret_id(env, receiver);
        return true;
    }

    // Object defaults. Nil is better than falling through to a noisy missing
    // selector path for optional properties. Existing generic fallback also
    // returns nil, but this keeps logs cleaner for high-frequency game loops.
    if matches!(
        selector_name,
        "parent"
            | "children"
            | "scheduler"
            | "actionManager"
            | "touchDispatcher"
            | "texture"
            | "spriteFrame"
            | "batchNode"
            | "camera"
            | "grid"
            | "shaderProgram"
            | "userData"
            | "userObject"
            | "delegate"
            | "target"
            | "selector"
            | "nextResponder"
            | "window"
            | "view"
            | "rootViewController"
            | "navigationController"
            | "scene"
            | "runningScene"
            | "unityView"
            | "rootView"
            | "mainDisplay"
            | "displayLink"
            | "renderDelegate"
            | "renderingView"
            | "glView"
            | "eaglView"
            | "unityKeyboard"
            | "keyboard"
            | "textInput"
            | "inputView"
            | "displayManager"
            | "displayConnection"
            | "viewController"
            | "controller"
            | "screens"
            | "currentScreen"
            | "mainScreen"
    ) {
        cocos_selector_log_once(class_name, selector_name, "nil object");
        objc_ret_zero(env);
        return true;
    }

    // Very broad final Cocos safety net for setters/callbacks with arguments.
    // This catches lots of minor version drift like setDisplayFrame: or
    // registerScriptHandler: without hiding truly unknown zero-argument getters.
    if engineish && selector_name.ends_with(':') {
        cocos_selector_log_once(class_name, selector_name, "argument selector no-op");
        objc_ret_zero(env);
        return true;
    }

    false
}

// ============================================================================

fn try_nsarray_indexed_subscript_interpose(
    env: &mut Environment,
    receiver: id,
    selector: SEL,
    orig_class: Class,
) -> bool {
    // PERF: this probe runs on *every* guest message send, before the real
    // method lookup. Reject with a zero-allocation, early-exiting byte compare
    // (the usual case mismatches on the first byte) instead of reading and
    // UTF-8-validating the whole selector name every time.
    if !selector.name_eq(&env.mem, b"objectAtIndexedSubscript:") {
        return false;
    }

    let class_name = env.objc.get_class_name(orig_class);
    if !class_name.contains("NSArray") && !class_name.contains("NSMutableArray") {
        return false;
    }
    let class_name = class_name.to_owned();

    let index = env.cpu.regs()[2];

    let sel_object_at_index = env
        .objc
        .register_host_selector("objectAtIndex:".to_string(), &mut env.mem);

    let obj: id = msg_send_no_type_checking(env, (receiver, sel_object_at_index, index));

    log_dbg!(
        "NSArray compat: [{} objectAtIndexedSubscript:{}] => {:?}",
        class_name,
        index,
        obj
    );

    env.cpu.regs_mut()[0] = obj.to_bits();
    env.cpu.regs_mut()[1] = 0;
    true
}

// GDataXML compatibility layer
// ============================================================================
//
// Some games bundle Google's GDataXML classes. Those classes directly
// dereference libxml2's xmlNode/xmlAttr structs, but touchHLE's libxml2 shim
// intentionally gives the guest opaque handle IDs instead of guest-visible
// structs. Trying to fake libxml2 structs in libxml2.rs can make the guest walk
// bad/cyclic XML graphs. Intercepting the tiny GDataXML surface the game uses
// is safer: parse XML into a small Rust DOM and return real Objective-C objects
// for GDataXMLDocument/GDataXMLElement/GDataXMLNode/GDataXMLAttribute methods.

#[derive(Clone)]
struct GDataCompatNode {
    name: String,
    text: String,
    attrs: Vec<(String, String)>,
    children: Vec<usize>,
    parent: Option<usize>,
}

#[derive(Clone)]
enum GDataCompatObject {
    Document { root: usize },
    Element { node: usize },
    Attribute { name: String, value: String },
}

#[derive(Default)]
struct GDataCompatState {
    nodes: Vec<GDataCompatNode>,
    objects: std::collections::HashMap<u32, GDataCompatObject>,
}

static GDATA_COMPAT_STATE: std::sync::Mutex<Option<GDataCompatState>> = std::sync::Mutex::new(None);

fn gdata_state<R>(f: impl FnOnce(&mut GDataCompatState) -> R) -> R {
    let mut guard = GDATA_COMPAT_STATE.lock().unwrap();
    let state = guard.get_or_insert_with(GDataCompatState::default);
    f(state)
}

fn gdata_class_name(env: &Environment, class: Class) -> Option<String> {
    let host_object = env.objc.get_host_object(class)?;
    if let Some(co) = host_object
        .as_any()
        .downcast_ref::<super::ClassHostObject>()
    {
        return Some(co.name.clone());
    }
    if let Some(co) = host_object
        .as_any()
        .downcast_ref::<super::UnimplementedClass>()
    {
        return Some(co.name.clone());
    }
    if let Some(co) = host_object.as_any().downcast_ref::<super::FakeClass>() {
        return Some(co.name.clone());
    }
    None
}

fn gdata_is_class(name: &str) -> bool {
    matches!(
        name,
        "GDataXMLDocument" | "GDataXMLElement" | "GDataXMLNode"
    )
}

fn gdata_set_ret_id(env: &mut Environment, value: id) {
    env.cpu.regs_mut()[0] = value.to_bits();
    env.cpu.regs_mut()[1] = 0;
}

fn gdata_set_ret_u32(env: &mut Environment, value: u32) {
    env.cpu.regs_mut()[0] = value;
    env.cpu.regs_mut()[1] = 0;
}

fn gdata_make_string(env: &mut Environment, s: String) -> id {
    crate::frameworks::foundation::ns_string::from_rust_string(env, s)
}

fn gdata_to_string(env: &mut Environment, s: id) -> String {
    crate::frameworks::foundation::ns_string::to_rust_string(env, s).to_string()
}

fn gdata_make_array(env: &mut Environment, objects: Vec<id>) -> id {
    crate::frameworks::foundation::ns_array::from_vec(env, objects)
}

fn gdata_alloc_object(env: &mut Environment, class_name: &str, object: GDataCompatObject) -> id {
    let class = env.objc.get_known_class(class_name, &mut env.mem);
    let obj = env
        .objc
        .alloc_object(class, Box::new(super::TrivialHostObject), &mut env.mem);
    gdata_state(|s| {
        s.objects.insert(obj.to_bits(), object);
    });
    obj
}

fn gdata_store_object(obj: id, object: GDataCompatObject) {
    gdata_state(|s| {
        s.objects.insert(obj.to_bits(), object);
    });
}

fn gdata_lookup_object(obj: id) -> Option<GDataCompatObject> {
    gdata_state(|s| s.objects.get(&obj.to_bits()).cloned())
}

fn gdata_node_name(node: usize) -> String {
    gdata_state(|s| {
        s.nodes
            .get(node)
            .map(|n| n.name.clone())
            .unwrap_or_default()
    })
}

fn gdata_node_string_value(node: usize) -> String {
    fn collect(nodes: &[GDataCompatNode], idx: usize, out: &mut String) {
        let Some(n) = nodes.get(idx) else {
            return;
        };
        if !n.text.is_empty() {
            out.push_str(&n.text);
        }
        for &child in &n.children {
            collect(nodes, child, out);
        }
    }
    gdata_state(|s| {
        let mut out = String::new();
        collect(&s.nodes, node, &mut out);
        out
    })
}

fn gdata_parse_xml(xml: &str) -> Option<usize> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_reader(xml.as_bytes());
    reader.config_mut().check_end_names = false;
    reader.config_mut().trim_markup_names_in_closing_tags = true;

    gdata_state(|state| {
        let mut stack: Vec<usize> = Vec::new();
        let mut root: Option<usize> = None;
        let mut errors = 0usize;

        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => {
                    let local = e.local_name();
                    let name = String::from_utf8_lossy(local.as_ref()).into_owned();
                    let mut attrs = Vec::new();
                    for attr in e.attributes().flatten() {
                        let key_local = attr.key.local_name();
                        let key = String::from_utf8_lossy(key_local.as_ref()).into_owned();
                        let val = match attr.unescape_value() {
                            Ok(v) => v.to_string(),
                            Err(_) => String::from_utf8_lossy(attr.value.as_ref()).into_owned(),
                        };
                        attrs.push((key, val));
                    }

                    let idx = state.nodes.len();
                    state.nodes.push(GDataCompatNode {
                        name,
                        text: String::new(),
                        attrs,
                        children: Vec::new(),
                        parent: stack.last().copied(),
                    });
                    if let Some(&parent) = stack.last() {
                        if let Some(p) = state.nodes.get_mut(parent) {
                            p.children.push(idx);
                        }
                    } else if root.is_none() {
                        root = Some(idx);
                    }
                    stack.push(idx);
                }
                Ok(Event::Empty(e)) => {
                    let local = e.local_name();
                    let name = String::from_utf8_lossy(local.as_ref()).into_owned();
                    let mut attrs = Vec::new();
                    for attr in e.attributes().flatten() {
                        let key_local = attr.key.local_name();
                        let key = String::from_utf8_lossy(key_local.as_ref()).into_owned();
                        let val = match attr.unescape_value() {
                            Ok(v) => v.to_string(),
                            Err(_) => String::from_utf8_lossy(attr.value.as_ref()).into_owned(),
                        };
                        attrs.push((key, val));
                    }

                    let idx = state.nodes.len();
                    state.nodes.push(GDataCompatNode {
                        name,
                        text: String::new(),
                        attrs,
                        children: Vec::new(),
                        parent: stack.last().copied(),
                    });
                    if let Some(&parent) = stack.last() {
                        if let Some(p) = state.nodes.get_mut(parent) {
                            p.children.push(idx);
                        }
                    } else if root.is_none() {
                        root = Some(idx);
                    }
                }
                Ok(Event::Text(e)) => {
                    if let Ok(text) = e.decode() {
                        let text = text.to_string();
                        if !text.trim().is_empty() {
                            if let Some(&idx) = stack.last() {
                                if let Some(n) = state.nodes.get_mut(idx) {
                                    n.text.push_str(&text);
                                }
                            }
                        }
                    }
                }
                Ok(Event::CData(e)) => {
                    if let Ok(text) = e.decode() {
                        if let Some(&idx) = stack.last() {
                            if let Some(n) = state.nodes.get_mut(idx) {
                                n.text.push_str(&text);
                            }
                        }
                    }
                }
                Ok(Event::End(_)) => {
                    if !stack.is_empty() {
                        stack.pop();
                    }
                }
                Ok(Event::Eof) => break,
                Err(err) => {
                    errors += 1;
                    if errors <= 4 {
                        log!("GDataXML compat: XML parse error: {}", err);
                    }
                    if errors > 64 {
                        break;
                    }
                }
                _ => {}
            }
        }

        root
    })
}

fn gdata_data_to_string(env: &mut Environment, data: id) -> String {
    let Some(sel_bytes) = env.objc.lookup_selector("bytes") else {
        return String::new();
    };
    let Some(sel_length) = env.objc.lookup_selector("length") else {
        return String::new();
    };
    let bytes: ConstPtr<u8> = msg_send_no_type_checking(env, (data, sel_bytes));
    let length: u32 = msg_send_no_type_checking(env, (data, sel_length));
    if bytes.is_null() || length == 0 {
        return String::new();
    }
    String::from_utf8_lossy(env.mem.bytes_at(bytes, length)).into_owned()
}

fn gdata_children_array(
    env: &mut Environment,
    node: usize,
    name_filter: Option<String>,
    deep: bool,
) -> id {
    fn visit(
        env: &mut Environment,
        node_idx: usize,
        filter: Option<&str>,
        deep: bool,
        out: &mut Vec<id>,
    ) {
        let children = gdata_state(|s| {
            s.nodes
                .get(node_idx)
                .map(|n| n.children.clone())
                .unwrap_or_default()
        });
        for child in children {
            let name = gdata_node_name(child);
            if filter.map(|f| f == name).unwrap_or(true) {
                out.push(gdata_alloc_object(
                    env,
                    "GDataXMLElement",
                    GDataCompatObject::Element { node: child },
                ));
            }
            if deep {
                visit(env, child, filter, deep, out);
            }
        }
    }

    let mut out = Vec::new();
    visit(env, node, name_filter.as_deref(), deep, &mut out);
    gdata_make_array(env, out)
}

fn gdata_attrs_array(env: &mut Environment, node: usize) -> id {
    let attrs = gdata_state(|s| {
        s.nodes
            .get(node)
            .map(|n| n.attrs.clone())
            .unwrap_or_default()
    });
    let mut out = Vec::new();
    for (name, value) in attrs {
        out.push(gdata_alloc_object(
            env,
            "GDataXMLNode",
            GDataCompatObject::Attribute { name, value },
        ));
    }
    gdata_make_array(env, out)
}

fn gdata_attribute_for_name(env: &mut Environment, node: usize, wanted: String) -> id {
    let found = gdata_state(|s| {
        s.nodes
            .get(node)
            .and_then(|n| n.attrs.iter().find(|(k, _)| k == &wanted).cloned())
    });
    if let Some((name, value)) = found {
        gdata_alloc_object(
            env,
            "GDataXMLNode",
            GDataCompatObject::Attribute { name, value },
        )
    } else {
        nil
    }
}

fn gdata_parse_libxml_node_handle(env: &mut Environment, node_handle: u32) -> Option<usize> {
    let xml = crate::frameworks::libxml2::compat_node_handle_to_xml_string(env, node_handle)?;
    let root = gdata_parse_xml(&xml);
    if root.is_none() {
        log!(
            "GDataXML compat: failed to parse XML dumped from libxml node handle {:#x}",
            node_handle
        );
    } else {
        log!(
            "GDataXML compat: parsed XML dumped from libxml node handle {:#x}",
            node_handle
        );
    }
    root
}

fn try_gdataxml_interpose(
    env: &mut Environment,
    receiver: id,
    selector: SEL,
    orig_class: Class,
) -> bool {
    // PERF: this probe runs on *every* guest message send. Reject non-GData
    // classes with a zero-allocation borrowed-name check first; only actual
    // GDataXML classes fall through to the name clones below (the clones
    // exist so the borrow of `env.objc` ends before handler arms use
    // `&mut env`).
    {
        let class_ref = env.objc.get_host_object(orig_class).and_then(|ho| {
            if let Some(co) = ho.as_any().downcast_ref::<super::ClassHostObject>() {
                Some(co.name.as_str())
            } else if let Some(co) = ho.as_any().downcast_ref::<super::UnimplementedClass>() {
                Some(co.name.as_str())
            } else if let Some(co) = ho.as_any().downcast_ref::<super::FakeClass>() {
                Some(co.name.as_str())
            } else {
                None
            }
        });
        let is_gdata = class_ref.is_some_and(gdata_is_class);
        if !is_gdata {
            return false;
        }
    }

    let sel = selector.as_str(&env.mem).to_string();
    let class_name = gdata_class_name(env, orig_class).unwrap_or_default();

    // Pull raw objc_msgSend argument registers before any nested host call can
    // clobber them.
    let arg2 = env.cpu.regs()[2];
    let arg3 = env.cpu.regs()[3];

    match (class_name.as_str(), sel.as_str()) {
        ("GDataXMLDocument", "initWithXMLString:error:")
        | ("GDataXMLDocument", "initWithXMLString:options:error:") => {
            let xml_obj: id = Ptr::from_bits(arg2);
            let xml = gdata_to_string(env, xml_obj);
            if let Some(root) = gdata_parse_xml(&xml) {
                gdata_store_object(receiver, GDataCompatObject::Document { root });
                log!(
                    "GDataXML compat: parsed XML string for document {:?}",
                    receiver
                );
                gdata_set_ret_id(env, receiver);
            } else {
                log!("GDataXML compat: failed to parse XML string");
                gdata_set_ret_id(env, nil);
            }
            return true;
        }

        ("GDataXMLDocument", "initWithData:")
        | ("GDataXMLDocument", "initWithData:options:error:") => {
            let data_obj: id = Ptr::from_bits(arg2);
            let xml = gdata_data_to_string(env, data_obj);
            if let Some(root) = gdata_parse_xml(&xml) {
                gdata_store_object(receiver, GDataCompatObject::Document { root });
                log!(
                    "GDataXML compat: parsed XML data for document {:?}",
                    receiver
                );
                gdata_set_ret_id(env, receiver);
            } else {
                log!("GDataXML compat: failed to parse XML data");
                gdata_set_ret_id(env, nil);
            }
            return true;
        }

        ("GDataXMLDocument", "initWithRootElement:") => {
            let elem_obj: id = Ptr::from_bits(arg2);
            if let Some(GDataCompatObject::Element { node }) = gdata_lookup_object(elem_obj) {
                gdata_store_object(receiver, GDataCompatObject::Document { root: node });
                gdata_set_ret_id(env, receiver);
            } else {
                gdata_set_ret_id(env, nil);
            }
            return true;
        }

        ("GDataXMLDocument", "rootElement") => {
            if let Some(GDataCompatObject::Document { root }) = gdata_lookup_object(receiver) {
                let obj = gdata_alloc_object(
                    env,
                    "GDataXMLElement",
                    GDataCompatObject::Element { node: root },
                );
                gdata_set_ret_id(env, obj);
            } else {
                gdata_set_ret_id(env, nil);
            }
            return true;
        }

        (_, "nodeConsumingXMLNode:") | (_, "nodeBorrowingXMLNode:") => {
            let node_handle = arg2;
            if let Some(root) = gdata_parse_libxml_node_handle(env, node_handle) {
                let obj = gdata_alloc_object(
                    env,
                    "GDataXMLElement",
                    GDataCompatObject::Element { node: root },
                );
                gdata_set_ret_id(env, obj);
            } else {
                gdata_set_ret_id(env, nil);
            }
            return true;
        }

        (_, "initConsumingXMLNode:") | (_, "initBorrowingXMLNode:") => {
            let node_handle = arg2;
            if let Some(root) = gdata_parse_libxml_node_handle(env, node_handle) {
                gdata_store_object(receiver, GDataCompatObject::Element { node: root });
                gdata_set_ret_id(env, receiver);
            } else {
                gdata_set_ret_id(env, nil);
            }
            return true;
        }

        ("GDataXMLElement", "elementWithName:stringValue:") => {
            let name: id = Ptr::from_bits(arg2);
            let value: id = Ptr::from_bits(arg3);
            let node = gdata_state(|s| {
                let idx = s.nodes.len();
                s.nodes.push(GDataCompatNode {
                    name: gdata_to_string(env, name),
                    text: gdata_to_string(env, value),
                    attrs: Vec::new(),
                    children: Vec::new(),
                    parent: None,
                });
                idx
            });
            let obj =
                gdata_alloc_object(env, "GDataXMLElement", GDataCompatObject::Element { node });
            gdata_set_ret_id(env, obj);
            return true;
        }

        ("GDataXMLNode", "attributeWithName:stringValue:")
        | ("GDataXMLElement", "attributeWithName:stringValue:") => {
            let name: id = Ptr::from_bits(arg2);
            let value: id = Ptr::from_bits(arg3);
            let attr_name = gdata_to_string(env, name);
            let attr_value = gdata_to_string(env, value);
            let obj = gdata_alloc_object(
                env,
                "GDataXMLNode",
                GDataCompatObject::Attribute {
                    name: attr_name,
                    value: attr_value,
                },
            );
            gdata_set_ret_id(env, obj);
            return true;
        }

        (_, "name") => {
            let out = match gdata_lookup_object(receiver) {
                Some(GDataCompatObject::Element { node }) => gdata_node_name(node),
                Some(GDataCompatObject::Attribute { name, .. }) => name,
                _ => String::new(),
            };
            let ret = gdata_make_string(env, out);
            gdata_set_ret_id(env, ret);
            return true;
        }

        (_, "stringValue") => {
            let out = match gdata_lookup_object(receiver) {
                Some(GDataCompatObject::Element { node }) => gdata_node_string_value(node),
                Some(GDataCompatObject::Attribute { value, .. }) => value,
                _ => String::new(),
            };
            let ret = gdata_make_string(env, out);
            gdata_set_ret_id(env, ret);
            return true;
        }

        ("GDataXMLElement", "children") | ("GDataXMLNode", "children") => {
            if let Some(GDataCompatObject::Element { node }) = gdata_lookup_object(receiver) {
                let arr = gdata_children_array(env, node, None, false);
                gdata_set_ret_id(env, arr);
            } else {
                let ret = gdata_make_array(env, Vec::new());
                gdata_set_ret_id(env, ret);
            }
            return true;
        }

        ("GDataXMLElement", "attributes") | ("GDataXMLNode", "attributes") => {
            if let Some(GDataCompatObject::Element { node }) = gdata_lookup_object(receiver) {
                let arr = gdata_attrs_array(env, node);
                gdata_set_ret_id(env, arr);
            } else {
                let ret = gdata_make_array(env, Vec::new());
                gdata_set_ret_id(env, ret);
            }
            return true;
        }

        ("GDataXMLElement", "elementsForName:") | ("GDataXMLNode", "elementsForName:") => {
            let name_obj: id = Ptr::from_bits(arg2);
            let wanted = gdata_to_string(env, name_obj);
            if let Some(GDataCompatObject::Element { node }) = gdata_lookup_object(receiver) {
                let arr = gdata_children_array(env, node, Some(wanted), false);
                gdata_set_ret_id(env, arr);
            } else {
                let ret = gdata_make_array(env, Vec::new());
                gdata_set_ret_id(env, ret);
            }
            return true;
        }

        ("GDataXMLElement", "attributeForName:") | ("GDataXMLNode", "attributeForName:") => {
            let name_obj: id = Ptr::from_bits(arg2);
            let wanted = gdata_to_string(env, name_obj);
            if let Some(GDataCompatObject::Element { node }) = gdata_lookup_object(receiver) {
                let obj = gdata_attribute_for_name(env, node, wanted);
                gdata_set_ret_id(env, obj);
            } else {
                gdata_set_ret_id(env, nil);
            }
            return true;
        }

        (_, "nodesForXPath:error:") | (_, "nodesForXPath:namespaces:error:") => {
            let xpath_obj: id = Ptr::from_bits(arg2);
            let xpath = gdata_to_string(env, xpath_obj);
            let result = if let Some(GDataCompatObject::Element { node }) =
                gdata_lookup_object(receiver)
            {
                if let Some(name) = xpath.strip_prefix("//") {
                    gdata_children_array(env, node, Some(name.to_string()), true)
                } else if let Some(name) = xpath.strip_prefix("./") {
                    gdata_children_array(env, node, Some(name.to_string()), false)
                } else {
                    gdata_make_array(env, Vec::new())
                }
            } else if let Some(GDataCompatObject::Document { root }) = gdata_lookup_object(receiver)
            {
                if let Some(name) = xpath.strip_prefix("//") {
                    gdata_children_array(env, root, Some(name.to_string()), true)
                } else {
                    gdata_make_array(env, Vec::new())
                }
            } else {
                gdata_make_array(env, Vec::new())
            };
            gdata_set_ret_id(env, result);
            return true;
        }

        (_, "dealloc") => {
            gdata_state(|s| {
                s.objects.remove(&receiver.to_bits());
            });
            env.cpu.regs_mut()[0..2].fill(0);
            return true;
        }

        (_, "shouldFreeXMLNode") => {
            gdata_set_ret_u32(env, 0);
            return true;
        }

        (_, "setShouldFreeXMLNode:") => {
            env.cpu.regs_mut()[0..2].fill(0);
            return true;
        }

        (_, "XMLNode") | (_, "XMLNodeCopy") => {
            gdata_set_ret_u32(env, 0);
            return true;
        }

        (_, "isEqual:") => {
            let other: id = Ptr::from_bits(arg2);
            gdata_set_ret_u32(env, if other == receiver { 1 } else { 0 });
            return true;
        }

        _ => {}
    }

    false
}
/// Standard variant of `objc_msgSend`. See [objc_msgSend_inner].
#[allow(non_snake_case)]
pub(super) fn objc_msgSend(env: &mut Environment, receiver: id, selector: SEL) {
    objc_msgSend_inner(
        env, receiver, selector, /* super2: */ None, /* tolerate_type_mismatch: */ false,
        /* skip_initialize: */ false,
    )
}

#[allow(non_snake_case)]
pub(crate) fn _touchHLE_objc_msgSend_tolerant(env: &mut Environment, receiver: id, selector: SEL) {
    objc_msgSend_inner(
        env, receiver, selector, /* super2: */ None, /* tolerate_type_mismatch: */ true,
        /* skip_initialize: */ false,
    )
}

/// Variant of `objc_msgSend` that does not trigger `+initialize`.
#[allow(non_snake_case)]
pub(crate) fn _touchHLE_objc_msgSend_no_initialize(
    env: &mut Environment,
    receiver: id,
    selector: SEL,
) {
    objc_msgSend_inner(
        env, receiver, selector, /* super2: */ None, /* tolerate_type_mismatch: */ false,
        /* skip_initialize: */ true,
    )
}

/// Variant of `objc_msgSend` for methods that return a struct via a pointer.
/// See [objc_msgSend_inner].
///
/// The first parameter here is the pointer for the struct return.
/// This is an
/// ABI detail that is usually hidden and handled behind-the-scenes by
/// [crate::abi], but `objc_msgSend` is a special case because of the
/// pass-through behaviour.
/// Of course, the pass-through only works if the [IMP]
/// also has the pointer parameter.
/// The caller therefore has to pick the
/// appropriate `objc_msgSend` variant depending on the method it wants to call.
pub(super) fn objc_msgSend_stret(
    env: &mut Environment,
    _stret: MutVoidPtr,
    receiver: id,
    selector: SEL,
) {
    objc_msgSend_inner(
        env, receiver, selector, /* super2: */ None, /* tolerate_type_mismatch: */ false,
        /* skip_initialize: */ false,
    )
}

#[allow(non_snake_case)]
pub(crate) fn _touchHLE_objc_msgSend_stret_tolerant(
    env: &mut Environment,
    _stret: MutVoidPtr,
    receiver: id,
    selector: SEL,
) {
    objc_msgSend_inner(
        env, receiver, selector, /* super2: */ None, /* tolerate_type_mismatch: */ true,
        /* skip_initialize: */ false,
    )
}

#[repr(C, packed)]
/// A pointer to this struct replaces the normal receiver parameter for
/// `objc_msgSendSuper2` and [msg_send_super2].
pub struct objc_super {
    pub receiver: id,
    /// If this is used with `objc_msgSendSuper` (not implemented here, TODO),
    /// this is a pointer to the superclass to look up the method on.
    /// If this is used with `objc_msgSendSuper2`, this is a pointer to a class
    /// and the superclass will be looked up from it.
    pub class: Class,
}
unsafe impl SafeRead for objc_super {}

/// Variant of `objc_msgSend` for supercalls. See [objc_msgSend_inner].
///
/// This variant has a weird ABI because it needs to receive an additional piece
/// of information (a class pointer), but it can't actually take this as an
/// extra parameter, because that would take one of the argument slots reserved
/// for arguments passed onto the method implementation.
/// Hence the [objc_super]
/// pointer in place of the normal [id].
#[allow(non_snake_case)]
pub(super) fn objc_msgSendSuper2(
    env: &mut Environment,
    super_ptr: ConstPtr<objc_super>,
    selector: SEL,
) {
    let objc_super { receiver, class } = env.mem.read(super_ptr);
    // Rewrite first argument to match the normal ABI.
    crate::abi::write_next_arg(&mut 0, env.cpu.regs_mut(), &mut env.mem, receiver);
    objc_msgSend_inner(
        env,
        receiver,
        selector,
        /* super2: */ Some(class),
        /* tolerate_type_mismatch: */ false,
        /* skip_initialize: */ false,
    )
}

#[allow(non_snake_case)]
pub(super) fn objc_msgSendSuper2_stret(
    env: &mut Environment,
    super_ptr: ConstPtr<objc_super>,
    selector: SEL,
) {
    let objc_super { receiver, class } = env.mem.read(super_ptr);
    // Rewrite first argument to match the normal ABI.
    crate::abi::write_next_arg(&mut 0, env.cpu.regs_mut(), &mut env.mem, receiver);
    objc_msgSend_inner(
        env,
        receiver,
        selector,
        /* super2: */ Some(class),
        /* tolerate_type_mismatch: */ false,
        /* skip_initialize: */ false,
    )
}

/// `void objc_enumerationMutation(id)` is called by compiler-generated fast
/// enumeration code when it detects that the collection was mutated while being
/// enumerated. On Apple's runtime the default handler raises an
/// `NSGenericException`, which aborts the enumeration. touchHLE previously had
/// no implementation, so the symbol resolved to a silent stub and the guest's
/// enumeration loop could spin forever when a buggy/edge-case path mutated the
/// collection mid-enumeration (e.g. Drive Ahead's first-frame render path).
///
/// We log the offending collection (throttled) for diagnostics. Raising a real
/// Objective-C exception from here would require unwinding guest frames, which
/// touchHLE does not support, so we only warn.
#[allow(non_snake_case)]
pub(super) fn objc_enumerationMutation(env: &mut Environment, obj: id) {
    use std::sync::Mutex;
    static SEEN: Mutex<Option<std::collections::HashSet<Class>>> = Mutex::new(None);
    let class = if obj != nil {
        ObjC::read_isa(obj, &env.mem)
    } else {
        nil
    };
    let mut guard = SEEN.lock().unwrap();
    let seen = guard.get_or_insert_with(Default::default);
    if seen.insert(class) {
        let name = env.objc.get_class_name(class).to_owned();
        let count: crate::frameworks::foundation::NSUInteger = if obj != nil {
            msg![env; obj count]
        } else {
            0
        };
        log!(
            "objc_enumerationMutation: collection {:?} (class {}) mutated while \
             being enumerated (count now {})",
            obj,
            name,
            count,
        );
    }
}

/// Trait that assists with type-checking of [msg_send]'s arguments.
///
/// - Statically constrains the types of [msg_send]'s arguments so that the
///   first two are always [id] and [SEL].
/// - Provides the type ID to enable dynamic type checking of subsequent
///   arguments and the return type.
///
/// See `impl_HostIMP` for implementations. See also [MsgSendSuperSignature].
pub trait MsgSendSignature: 'static {
    /// Get the [TypeId] and a human-readable description for this signature.
    fn type_info() -> (TypeId, &'static str) {
        #[cfg(debug_assertions)]
        let type_name = std::any::type_name::<Self>();
        // Avoid wasting space on type names in release builds.
        // At the time of writing this saves about 36KB.
        #[cfg(not(debug_assertions))]
        let type_name = "[description unavailable in release builds]";
        (TypeId::of::<Self>(), type_name)
    }
}

// --- Extended implementations for higher number of arguments (7, 8, 9
// parameters) ---
impl<
        R: 'static,
        P1: 'static,
        P2: 'static,
        P3: 'static,
        P4: 'static,
        P5: 'static,
        P6: 'static,
        P7: 'static,
    > MsgSendSignature for (R, (id, SEL, P1, P2, P3, P4, P5, P6, P7))
{
}
impl<
        R: 'static,
        P1: 'static,
        P2: 'static,
        P3: 'static,
        P4: 'static,
        P5: 'static,
        P6: 'static,
        P7: 'static,
        P8: 'static,
    > MsgSendSignature for (R, (id, SEL, P1, P2, P3, P4, P5, P6, P7, P8))
{
}
impl<
        R: 'static,
        P1: 'static,
        P2: 'static,
        P3: 'static,
        P4: 'static,
        P5: 'static,
        P6: 'static,
        P7: 'static,
        P8: 'static,
        P9: 'static,
    > MsgSendSignature for (R, (id, SEL, P1, P2, P3, P4, P5, P6, P7, P8, P9))
{
}

/// Wrapper around [objc_msgSend] which, together with [msg], makes it easy to
/// send messages in host code.
/// Warning: all types are inferred from the
/// call-site and they may not be checked, so be very sure you get them correct!
pub fn msg_send<R, P>(env: &mut Environment, args: P) -> R
where
    fn(&mut Environment, id, SEL): CallFromHost<R, P>,
    fn(&mut Environment, MutVoidPtr, id, SEL): CallFromHost<R, P>,
    (R, P): MsgSendSignature,
    R: GuestRet,
{
    // Provide type info for dynamic type checking.
    env.objc.message_type_info = Some(<(R, P) as MsgSendSignature>::type_info());
    if R::SIZE_IN_MEM.is_some() {
        (objc_msgSend_stret as fn(&mut Environment, MutVoidPtr, id, SEL)).call_from_host(env, args)
    } else {
        (objc_msgSend as fn(&mut Environment, id, SEL)).call_from_host(env, args)
    }
}

pub fn msg_send_no_type_checking<R, P>(env: &mut Environment, args: P) -> R
where
    fn(&mut Environment, id, SEL): CallFromHost<R, P>,
    fn(&mut Environment, MutVoidPtr, id, SEL): CallFromHost<R, P>,
    (R, P): MsgSendSignature,
    R: GuestRet,
{
    if R::SIZE_IN_MEM.is_some() {
        (_touchHLE_objc_msgSend_stret_tolerant as fn(&mut Environment, MutVoidPtr, id, SEL))
            .call_from_host(env, args)
    } else {
        (_touchHLE_objc_msgSend_tolerant as fn(&mut Environment, id, SEL)).call_from_host(env, args)
    }
}

/// Variant of [msg_send] which does not trigger `+initialize` on the receiver.
///
/// This is meant for sending `+load`: the Objective-C runtime guarantees that
/// `+load` runs before `+initialize`, so it must not go through the normal
/// [msg_send] path (which would call `+initialize` first).
pub fn msg_send_no_initialize<R, P>(env: &mut Environment, args: P) -> R
where
    fn(&mut Environment, id, SEL): CallFromHost<R, P>,
    (R, P): MsgSendSignature,
    R: GuestRet,
{
    assert!(
        R::SIZE_IN_MEM.is_none(),
        "msg_send_no_initialize does not support struct returns"
    );
    // Provide type info for dynamic type checking.
    env.objc.message_type_info = Some(<(R, P) as MsgSendSignature>::type_info());
    (_touchHLE_objc_msgSend_no_initialize as fn(&mut Environment, id, SEL))
        .call_from_host(env, args)
}

/// Counterpart of [MsgSendSignature] for [msg_send_super2].
pub trait MsgSendSuperSignature: 'static {
    /// Signature with the [objc_super] pointer replaced by [id].
    type WithoutSuper: MsgSendSignature;
}

// --- Extended super-call implementations for higher number of arguments ---
impl<
        R: 'static,
        P1: 'static,
        P2: 'static,
        P3: 'static,
        P4: 'static,
        P5: 'static,
        P6: 'static,
        P7: 'static,
    > MsgSendSuperSignature for (R, (ConstPtr<objc_super>, SEL, P1, P2, P3, P4, P5, P6, P7))
{
    type WithoutSuper = (R, (id, SEL, P1, P2, P3, P4, P5, P6, P7));
}
impl<
        R: 'static,
        P1: 'static,
        P2: 'static,
        P3: 'static,
        P4: 'static,
        P5: 'static,
        P6: 'static,
        P7: 'static,
        P8: 'static,
    > MsgSendSuperSignature
    for (
        R,
        (ConstPtr<objc_super>, SEL, P1, P2, P3, P4, P5, P6, P7, P8),
    )
{
    type WithoutSuper = (R, (id, SEL, P1, P2, P3, P4, P5, P6, P7, P8));
}
impl<
        R: 'static,
        P1: 'static,
        P2: 'static,
        P3: 'static,
        P4: 'static,
        P5: 'static,
        P6: 'static,
        P7: 'static,
        P8: 'static,
        P9: 'static,
    > MsgSendSuperSignature
    for (
        R,
        (
            ConstPtr<objc_super>,
            SEL,
            P1,
            P2,
            P3,
            P4,
            P5,
            P6,
            P7,
            P8,
            P9,
        ),
    )
{
    type WithoutSuper = (R, (id, SEL, P1, P2, P3, P4, P5, P6, P7, P8, P9));
}

/// [msg_send] but for super-calls (calls [objc_msgSendSuper2]). You probably
/// want to use [msg_super] rather than calling this directly.
pub fn msg_send_super2<R, P>(env: &mut Environment, args: P) -> R
where
    fn(&mut Environment, ConstPtr<objc_super>, SEL): CallFromHost<R, P>,
    fn(&mut Environment, MutVoidPtr, ConstPtr<objc_super>, SEL): CallFromHost<R, P>,
    (R, P): MsgSendSuperSignature,
    R: GuestRet,
{
    // Provide type info for dynamic type checking.
    env.objc.message_type_info = Some(<(R, P) as MsgSendSuperSignature>::WithoutSuper::type_info());
    if R::SIZE_IN_MEM.is_some() {
        // Struct returns (stret) for super-calls aren't implemented yet.
        // Log this clearly and fall through to the non-stret path so the
        // host process keeps running and the caller will simply observe
        // the default-constructed return value via to_regs/to_mem below.
        log!(
            "Warning: msg_send_super2: struct-return (stret) super-call is not implemented; falling back to non-stret dispatch. Result may be unreliable.",
        );
        (objc_msgSendSuper2 as fn(&mut Environment, ConstPtr<objc_super>, SEL))
            .call_from_host(env, args)
    } else {
        (objc_msgSendSuper2 as fn(&mut Environment, ConstPtr<objc_super>, SEL))
            .call_from_host(env, args)
    }
}

/// Macro for sending a message which imitates the Objective-C messaging syntax.
///
/// See [msg_send] for the underlying implementation. Warning: all types are
/// inferred from the call-site and they may not be checked, so be very sure you
/// get them correct!
///
/// ```ignore
/// msg![env; foo setBar:bar withQux:qux];
/// ```
///
/// desugars to:
///
/// ```ignore
/// {
///     let sel = env.objc.lookup_selector("setFoo:withBar").unwrap();
///     msg_send(env, (foo, sel, bar, qux))
/// }
/// ```
///
/// Note that argument values that aren't a bare single identifier like `foo`
/// need to be bracketed.
///
/// See also [msg_class], if you want to send a message to a class.
#[macro_export]
macro_rules! msg {
    [$env:expr; $receiver:tt $name:ident $(: $arg1:tt $($($namen:ident)?: $argn:tt)*)?] => {
        {
            let sel = $crate::objc::selector!($($arg1;)? $name $($(, $($namen)?)*)?);
            let sel = $env.objc.lookup_selector(sel)
                .expect("Unknown selector");
            let args = ($receiver, sel, $($arg1, $($argn),*)?);
            $crate::objc::msg_send($env, args)
        }
    }
}
pub use crate::msg;
// #[macro_export] is weird...

/// Variant of [msg] for super-calls.
///
/// Unlike the other variants, this macro can only be used within
/// [crate::objc::objc_classes], because it relies on that macro defining a
/// constant containing the name of the current class.
///
/// ```ignore
/// msg_super![env; this init]
/// ```
///
/// desugars to something like this, if the current class is `SomeClass`:
///
/// ```ignore
/// {
///     let super_arg_ptr = push_to_stack(env, objc_super {
///         receiver: this,
///         class: env.objc.get_known_class("SomeClass", &mut env.mem),
///     });
///     let sel = env.objc.lookup_selector("init").unwrap();
///     let res = msg_send_super2(env, (super_arg_ptr, sel));
///     pop_from_stack::<objc_super>(env);
///     res
/// }
/// ```
#[macro_export]
macro_rules! msg_super {
    [$env:expr; $receiver:tt $name:ident $(: $arg1:tt $($($namen:ident)?: $argn:tt)*)?] => {
        {
            let class = $env.objc.get_known_class(
                _OBJC_CURRENT_CLASS,
                &mut $env.mem
            );
            let sel = $crate::objc::selector!($($arg1;)? $name $($(, $($namen)?)*)?);
            let sel = $env.objc.lookup_selector(sel)
                .expect("Unknown selector");
            let sp = &mut $env.cpu.regs_mut()[$crate::cpu::Cpu::SP];
            let old_sp = *sp;
            *sp -= $crate::mem::guest_size_of::<$crate::objc::objc_super>();
            let super_ptr = $crate::mem::Ptr::from_bits(*sp);
            $env.mem.write(super_ptr, $crate::objc::objc_super {
                receiver: $receiver,
                class,
            });
            let args = (super_ptr.cast_const(), sel, $($arg1, $($argn),*)?);
            let res = $crate::objc::msg_send_super2($env, args);

            $env.cpu.regs_mut()[$crate::cpu::Cpu::SP] = old_sp;
            res
        }
    }
}
pub use crate::msg_super;
// #[macro_export] is weird...

/// Variant of [msg] for sending a message to a named class.
/// Useful for calling class methods, especially `new`.
///
/// ```ignore
/// msg_class![env; SomeClass alloc]
/// ```
///
/// desugars to:
///
/// ```ignore
/// msg![env; (env.objc.get_known_class("SomeClass", &mut env.mem)) alloc]
/// ```
#[macro_export]
macro_rules! msg_class {
    [$env:expr; $receiver_class:ident $name:ident $(: $arg1:tt $($($namen:ident)?: $argn:tt)*)?] => {
        {
            let class = $env.objc.get_known_class(
                stringify!($receiver_class),
                &mut $env.mem
            );
            $crate::objc::msg![$env; class $name $(: $arg1 $($($namen)?: $argn)*)?]
        }
    }
}
pub use crate::msg_class;
// #[macro_export] is weird...

/// Shorthand for `let _: id = msg![env; object retain];`
pub fn retain(env: &mut Environment, object: id) -> id {
    if object == nil {
        // fast path
        return nil;
    }
    msg![env; object retain]
}

/// Shorthand for `() = msg![env; object release];`
pub fn release(env: &mut Environment, object: id) {
    if object == nil {
        // fast path
        return;
    }
    msg![env; object release]
}

/// Shorthand for `let _: id = msg![env; object autorelease];`
pub fn autorelease(env: &mut Environment, object: id) -> id {
    if object == nil {
        // fast path
        return nil;
    }
    msg![env; object autorelease]
}

#[cfg(test)]
mod fake_class_tests {
    use super::fake_class_message_result;

    #[test]
    fn openfeint_reports_that_the_player_declined_it() {
        assert_eq!(
            fake_class_message_result("OpenFeint", true, "hasUserSetFeintAccess"),
            Some(1)
        );
        // Everything else, including whether OpenFeint was approved, still
        // behaves as if sent to nil.
        assert_eq!(
            fake_class_message_result("OpenFeint", true, "hasUserApprovedFeint"),
            None
        );
        assert_eq!(
            fake_class_message_result("OpenFeint", false, "hasUserSetFeintAccess"),
            None
        );
        assert_eq!(
            fake_class_message_result("FlurryAPI", true, "hasUserSetFeintAccess"),
            None
        );
    }
}
