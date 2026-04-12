/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UINavigationController`.

use crate::frameworks::foundation::{ns_array, NSUInteger};
use crate::objc::{
    autorelease, id, impl_HostObject_with_superclass, msg, nil, objc_classes, release, retain,
    ClassExports, NSZonePtr, SEL,
};

// TODO: navigation bar and toolbar
// TODO: animations

#[derive(Default)]
struct UINavigationControllerHostObject {
    superclass: super::UIViewControllerHostObject,
    delegate: id,
    navigation_stack: Vec<id>,
    // Ссылка на панель навигации
    navigation_bar: id,
}
impl_HostObject_with_superclass!(UINavigationControllerHostObject);

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UINavigationController: UIViewController

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UINavigationControllerHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithRootViewController:(id)root_vc {
    let this: id = msg![env; this init];
    () = msg![env; this pushViewController:root_vc animated:false];
    this
}

- (())setDelegate:(id)delegate {
    log_dbg!(
        "[(UINavigationController*){:?} setDelegate:{:?}]",
        this,
        delegate
    );
    let host_object =
        env.objc.borrow_mut::<UINavigationControllerHostObject>(this);
    host_object.delegate = delegate;
}
- (id)delegate {
    env.objc
        .borrow::<UINavigationControllerHostObject>(this)
        .delegate
}

- (())pushViewController:(id)view_controller animated:(bool)_animated {
    let stack = &mut env
        .objc
        .borrow_mut::<UINavigationControllerHostObject>(this)
        .navigation_stack;
    assert!(!stack.contains(&view_controller));
    stack.push(view_controller);
    retain(env, view_controller);

    // Tell the pushed VC who its navigation controller is (weak ref,
    // matches real UIKit behaviour — no retain cycle).
    () = msg![env; view_controller setNavigationController:this];

    let delegate = env
        .objc
        .borrow::<UINavigationControllerHostObject>(this)
        .delegate;
    let sel: SEL = env.objc.register_host_selector(
        "navigationController:willShowViewController:animated:"
            .to_string(),
        &mut env.mem,
    );
    let responds: bool = msg![env; delegate respondsToSelector:sel];
    if responds {
        () = msg![
            env; delegate
            navigationController:this
            willShowViewController:view_controller
            animated:false
        ];
    }
    let self_view: id = msg![env; this view];
    let vc_view: id = msg![env; view_controller view];
    () = msg![env; view_controller viewWillAppear:false];
    () = msg![env; self_view addSubview:vc_view];
    () = msg![env; view_controller viewDidAppear:false];
    let sel: SEL = env.objc.register_host_selector(
        "navigationController:didShowViewController:animated:"
            .to_string(),
        &mut env.mem,
    );
    let responds: bool = msg![env; delegate respondsToSelector:sel];
    if responds {
        () = msg![
            env; delegate
            navigationController:this
            didShowViewController:view_controller
            animated:false
        ];
    }
}

- (())popViewControllerAnimated:(bool)_animated {
    let stack = &mut env
        .objc
        .borrow_mut::<UINavigationControllerHostObject>(this)
        .navigation_stack;
    if let Some(popped) = stack.pop() {
        // Clear the back-reference before releasing.
        () = msg![env; popped setNavigationController:nil];
        release(env, popped);
    }
}

- (id)topViewController {
    if let Some(top_vc) = env
        .objc
        .borrow::<UINavigationControllerHostObject>(this)
        .navigation_stack
        .last()
    {
        *top_vc
    } else {
        nil
    }
}

- (id)viewControllers {
    let vcs = env
        .objc
        .borrow::<UINavigationControllerHostObject>(this)
        .navigation_stack
        .to_vec();
    for vc in &vcs {
        retain(env, *vc);
    }
    let res = ns_array::from_vec(env, vcs);
    autorelease(env, res)
}

- (())setViewControllers:(id)controllers {
    msg![env; this setViewControllers:controllers animated:false]
}

- (())setViewControllers:(id)controllers animated:(bool)animated {
    assert!(!animated);

    let self_view = env
        .objc
        .borrow::<UINavigationControllerHostObject>(this)
        .superclass
        .view;
    let mut stack = std::mem::take(
        &mut env
            .objc
            .borrow_mut::<UINavigationControllerHostObject>(this)
            .navigation_stack,
    );

    for controller in stack.drain(..) {
        let vc_view = env
            .objc
            .borrow::<super::UIViewControllerHostObject>(controller)
            .view;
        let vc_view_superview = msg![env; vc_view superview];
        assert_eq!(self_view, vc_view_superview);
        () = msg![env; vc_view removeFromSuperview];
        // Clear back-reference before releasing.
        () = msg![env; controller setNavigationController:nil];
        release(env, controller);
    }

    let mut tmp_stack: Vec<id> = Vec::new();
    let count: NSUInteger = msg![env; controllers count];
    assert!(count > 0);
    for i in 0..(count - 1) {
        let next: id = msg![env; controllers objectAtIndex:i];
        tmp_stack.push(next);
        retain(env, next);
        // Inform each VC of its navigation controller.
        () = msg![env; next setNavigationController:this];
    }
    env.objc
        .borrow_mut::<UINavigationControllerHostObject>(this)
        .navigation_stack = tmp_stack;

    let last_vc: id = msg![env; controllers objectAtIndex:(count - 1)];
    () = msg![env; this pushViewController:last_vc animated:animated];
}

- (id)navigationBar {
    let existing_bar = env
        .objc
        .borrow::<UINavigationControllerHostObject>(this)
        .navigation_bar;

    if existing_bar != nil {
        return existing_bar;
    }

    // Lazily create a UINavigationBar instance.
    let bar_class =
        env.objc.get_known_class("UINavigationBar", &mut env.mem);
    let bar: id = msg![env; bar_class alloc];
    let bar: id = msg![env; bar init];

    env.objc
        .borrow_mut::<UINavigationControllerHostObject>(this)
        .navigation_bar = bar;

    bar
}

- (())setNavigationBarHidden:(bool)_hidden {
    // TODO
}

- (())setNavigationBarHidden:(bool)_hidden animated:(bool)_animated {
    log!(
        "TODO: UINavigationController -setNavigationBarHidden:animated: stubbed"
    );
}

@end

};
