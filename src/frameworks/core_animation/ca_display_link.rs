/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CADisplayLink`

use crate::frameworks::foundation::ns_run_loop::NSRunLoopMode;
use crate::frameworks::foundation::ns_timer::set_time_interval;
use crate::frameworks::foundation::NSInteger;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr, SEL,
};

#[derive(Default)]
struct CADisplayLinkHostObject {
    ns_timer: id,
    paused: bool,
    target: id,
    selector: Option<SEL>,
}
impl HostObject for CADisplayLinkHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation CADisplayLink: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::new(CADisplayLinkHostObject::default()), &mut env.mem)
}

+ (id)displayLinkWithTarget:(id)target selector:(SEL)sel {
    let display_link: id = msg![env; this new];
    
    // Сохраняем целевой объект и селектор (игры) внутри самого CADisplayLink
    {
        let mut host_object = env.objc.borrow_mut::<CADisplayLinkHostObject>(display_link);
        host_object.target = target;
        host_object.selector = Some(sel);
    }
    retain(env, target);

    // Создаем внутренний таймер, но его целью делаем НАШ display_link.
    // Таймер удержит (retain) display_link, предотвращая его удаление и Use-After-Free.
    let fire_sel = env.objc.lookup_selector("_touchHLE_displayLinkFired:").unwrap();
    let ns_timer = msg_class![env; NSTimer timerWithTimeInterval:(1.0/60.0)
                     target:display_link
                   selector:fire_sel
                   userInfo:nil
                    repeats:true];
    retain(env, ns_timer);
    
    {
        let mut host_object = env.objc.borrow_mut::<CADisplayLinkHostObject>(display_link);
        host_object.ns_timer = ns_timer;
    }
    
    log_dbg!("[CADisplayLink displayLinkWithTarget:{:?} selector:{}] => {:?}", target, sel.as_str(&env.mem), display_link);
    autorelease(env, display_link)
}

// Внутренний трамплин: вызывается таймером, перенаправляет вызов в игру
- (())_touchHLE_displayLinkFired:(id)_timer {
    // Берем данные и сразу отпускаем заимствование (borrow), 
    // чтобы игра могла безопасно взаимодействовать с CADisplayLink.
    let (target, sel, paused) = {
        let host = env.objc.borrow::<CADisplayLinkHostObject>(this);
        (host.target, host.selector, host.paused)
    };
    
    // Защита от вызова во время или после invalidate
    if paused || target == nil {
        return;
    }
    
    if let Some(sel) = sel {
        // Передаем this (сам CADisplayLink), как и ожидает игра!
        let sel_str = sel.as_str(&env.mem);
        if sel_str.ends_with(':') {
            let _: u32 = crate::objc::msg_send_no_type_checking(env, (target, sel, this));
        } else {
            let _: u32 = crate::objc::msg_send_no_type_checking(env, (target, sel));
        }
    }
}

// Честный timestamp, необходимый для вычисления Delta Time в играх (уберет черный экран)
- (f64)timestamp {
    msg_class![env; NSDate timeIntervalSinceReferenceDate]
}

// Честный duration
- (f64)duration {
    let ns_timer = env.objc.borrow::<CADisplayLinkHostObject>(this).ns_timer;
    if ns_timer == nil {
        return 1.0 / 60.0;
    }
    msg![env; ns_timer timeInterval]
}

- (bool)isPaused {
    env.objc.borrow::<CADisplayLinkHostObject>(this).paused
}

- (())setPaused:(bool)paused {
    log_dbg!("[(CADisplayLink*){:?} setPaused:{}]", this, paused);
    env.objc.borrow_mut::<CADisplayLinkHostObject>(this).paused = paused;
}
    
- (())setFrameInterval:(NSInteger)frameInterval {
    log_dbg!("[(CADisplayLink*){:?} setFrameInterval:{}]", this, frameInterval);
    
    // Предотвращаем деление на 0 или отрицательные значения (безопаснее, чем assert)
    let safe_interval = frameInterval.max(1);
    let interval = safe_interval as f64 / 60.0;
    
    let ns_timer = env.objc.borrow::<CADisplayLinkHostObject>(this).ns_timer;
    if ns_timer != nil {
        set_time_interval(env, ns_timer, interval);
    }
}

- (())addToRunLoop:(id)run_loop forMode:(NSRunLoopMode)mode {
    log_dbg!("[(CADisplayLink*){:?} addToRunLoop:{:?} forMode:{:?}]", this, run_loop, mode);
    let ns_timer = env.objc.borrow::<CADisplayLinkHostObject>(this).ns_timer;
    if ns_timer != nil {
        () = msg![env; run_loop addTimer:ns_timer forMode:mode];
    }
}

- (())removeFromRunLoop:(id)run_loop forMode:(NSRunLoopMode)mode {
    log_dbg!("[(CADisplayLink*){:?} removeFromRunLoop:{:?} forMode:{:?}]", this, run_loop, mode);
    // В NSTimer нет прямого API для удаления из конкретного RunLoop,
    // игра в любом случае должна вызывать invalidate.
}

- (())invalidate {
    log_dbg!("[(CADisplayLink*){:?} invalidate]", this);
    
    // Освобождаем зависимости и обнуляем их в HostObject, 
    // чтобы разорвать Retain Cycle: (Target -> CADisplayLink -> Target).
    let (ns_timer, target) = {
        let mut host = env.objc.borrow_mut::<CADisplayLinkHostObject>(this);
        let t = host.ns_timer;
        let tgt = host.target;
        host.ns_timer = nil;
        host.target = nil;
        (t, tgt)
    };

    if ns_timer != nil {
        () = msg![env; ns_timer invalidate];
        release(env, ns_timer);
    }
    
    if target != nil {
        release(env, target);
    }
}

- (())dealloc {
    let (ns_timer, target) = {
        let host_object = env.objc.borrow::<CADisplayLinkHostObject>(this);
        (host_object.ns_timer, host_object.target)
    };
    
    if ns_timer != nil {
        release(env, ns_timer);
    }
    if target != nil {
        release(env, target);
    }
    
    env.objc.dealloc_object(this, &mut env.mem);
}

@end

};
