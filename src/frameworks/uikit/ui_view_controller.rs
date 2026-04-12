/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIViewController`.
//!
//! Resources:
//! - [View Controller Programming Guide for iOS (Legacy)](https://developer.apple.com/library/archive/documentation/WindowsViews/Conceptual/ViewControllerPGforiOSLegacy/BasicViewControllers/BasicViewControllers.html)

use crate::frameworks::core_graphics::CGRect;
use crate::frameworks::foundation::NSUInteger;
use crate::frameworks::foundation::ns_objc_runtime::NSStringFromClass;
use crate::frameworks::foundation::ns_string::{from_rust_string, get_static_str, to_rust_string};
use crate::frameworks::uikit::ui_application::{
    UIInterfaceOrientation, UIInterfaceOrientationPortrait,
};
use crate::frameworks::uikit::ui_view::set_view_controller;
use crate::objc::{
    id, msg, msg_class, msg_super, nil, objc_classes, release, retain, todo_objc_setter, Class, ClassExports,
    HostObject, NSZonePtr,
};
use crate::Environment;

pub mod ui_navigation_controller;

pub type UIModalTransitionStyle = i32;
pub type UIModalPresentationStyle = i32;

#[derive(Default)]
struct UIViewControllerHostObject {
    /// The root view.
    /// `UIView*`
    view: id,
    /// Nib name to be used at the load
    /// of the root view, may be nil.
    /// `NSString*`
    nib_name: id,
    /// Bundle to be used for load
    /// of the nib by name, may be nil.
    /// `NSBundle*`
    bundle: id,
    title: id,                   // Для хранения строки заголовка
    parent_view_controller: id,  // Для связи с родительским VC
    navigation_controller: id,   // Для фикса ошибки "does not respond to selector"
    // ---------------------------
    modal_transition_style: UIModalTransitionStyle,
    modal_presentation_style: UIModalPresentationStyle,
}
impl HostObject for UIViewControllerHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIViewController: UIResponder

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UIViewControllerHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)navigationController {
    env.objc.borrow::<UIViewControllerHostObject>(this).navigation_controller
}

- (id)parentViewController {
    env.objc.borrow::<UIViewControllerHostObject>(this).parent_view_controller
}

// TODO: this should be a designated initializer
- (id)initWithNibName:(id)nib_name // NSString *
               bundle:(id)bundle { // NSBundle *
    retain(env, nib_name);
    retain(env, bundle);

    log_dbg!("[(UIViewController*){:?} initWithNibName:{:?} bundle:{:?}]", this, nib_name, bundle);

    env.objc.borrow_mut::<UIViewControllerHostObject>(this).nib_name = nib_name;
    env.objc.borrow_mut::<UIViewControllerHostObject>(this).bundle = bundle;

    this
}

- (id)initWithCoder:(id)coder {
    let key_ns_string = get_static_str(env, "UIView");
    let view: id = msg![env; coder decodeObjectForKey:key_ns_string];

    () = msg![env; this setView:view];

    // Читаем и сохраняем имя NIB-файла, чтобы контроллер знал, откуда грузить EAGLView
    let nib_name_key = get_static_str(env, "UINibName");
    let nib_name: id = msg![env; coder decodeObjectForKey:nib_name_key];
    if nib_name != nil {
        retain(env, nib_name);
        let host_obj = env.objc.borrow_mut::<UIViewControllerHostObject>(this);
        let old_nib_name = host_obj.nib_name;
        host_obj.nib_name = nib_name;
        release(env, old_nib_name);
    }
    
    // Также на всякий случай вытягиваем Bundle
    let bundle_key = get_static_str(env, "UIBundleName");
    let bundle: id = msg![env; coder decodeObjectForKey:bundle_key];
    if bundle != nil {
        retain(env, bundle);
        let host_obj = env.objc.borrow_mut::<UIViewControllerHostObject>(this);
        let old_bundle = host_obj.bundle;
        host_obj.bundle = bundle;
        release(env, old_bundle);
    }

    this
}

- (())dealloc {
    let &UIViewControllerHostObject { view, nib_name, bundle, title, .. } = env.objc.borrow(this);

    release(env, view);
    release(env, nib_name);
    release(env, bundle);
    release(env, title); // предотвращаем утечку памяти

    env.objc.dealloc_object(this, &mut env.mem);
}

- (())setParentViewController:(id)parent {
    env.objc.borrow_mut::<UIViewControllerHostObject>(this).parent_view_controller = parent;
}

- (())setNavigationController:(id)nav_controller {
    // Используем прямое присваивание (weak reference в iOS),
    // чтобы не создавать циклов сильных ссылок
    env.objc.borrow_mut::<UIViewControllerHostObject>(this).navigation_controller = nav_controller;
}

- (())loadView {
    let bundle: id = env.objc.borrow::<UIViewControllerHostObject>(this).bundle;
    let bundle: id = if bundle == nil {
        msg_class![env; NSBundle mainBundle]
    } else {
        bundle
    };

    let nib_name: id = get_nib_name(env, this, bundle);
    if nib_name != nil {
        // If we do have nib name, try to load it!
        log_dbg!(
            "Load {:?} view controller's view by nib, using name {}", this, to_rust_string(env, nib_name)
        );

        let nib: id = msg_class![env; UINib nibWithNibName:nib_name bundle:bundle];
        release(env, nib_name);

        // The NIB's File's Owner will be substituted by `this`,
        // implicitly loading the view as well
        let _: id = msg![env; nib instantiateWithOwner:this options:nil];

        let view = env.objc.borrow::<UIViewControllerHostObject>(this).view;
        // Having nil view at this point probably mean that
        // out nib's parsing is wrong.
        // Also we assume here the case of a "detached nib file"
        // TODO: support "integrated nib file"
        assert!(view != nil);

        return;
    };

    // As a last resort, use plain UIVIew for the root view.
    // Use full screen bounds (not applicationFrame) so that the view
    // covers the entire window — games using OpenGL ES expect a
    // full-screen surface, and using applicationFrame would shift the
    // view down by the status-bar height (20 px), breaking hit-testing
    // and touch-coordinate calculations.
    let class: Class = msg![env; this class];
    log!("Unable to load {:?} {} view controller's view by nib, using plain UIView", this, env.objc.get_class_name(class).to_string());
    let view: id = msg_class![env; UIView alloc];
    let screen: id = msg_class![env; UIScreen mainScreen];
    // FIX: use bounds (full 320×480) instead of applicationFrame (320×460
    // starting at y=20) so OpenGL games get a correctly-sized root view
    // and touch coordinates are not offset by the status-bar height.
    let screen_bounds: CGRect = msg![env; screen bounds];
    let view: id = msg![env; view initWithFrame:screen_bounds];
    () = msg![env; this setView:view];
}

- (())setView:(id)new_view { // UIView*
    let host_obj = env.objc.borrow_mut::<UIViewControllerHostObject>(this);
    let old_view = std::mem::replace(&mut host_obj.view, new_view);
    if old_view != nil {
        set_view_controller(env, old_view, nil);
    }
    if new_view != nil {
        set_view_controller(env, new_view, this);
    }
    retain(env, new_view);
    release(env, old_view);
}

- (id)view {
    let view = env.objc.borrow_mut::<UIViewControllerHostObject>(this).view;
    if view == nil {
        () = msg![env; this loadView];
        let view = env.objc.borrow_mut::<UIViewControllerHostObject>(this).view;
        () = msg![env; this viewDidLoad];
        view
    } else {
        view
    }
}

// Перехватываем NIB-соединение (KVC) для свойства view
- (())setValue:(id)value forKey:(id)key {
    let key_str = to_rust_string(env, key);
    if key_str == "view" {
        () = msg![env; this setView:value];
    } else {
        () = msg_super![env; this setValue:value forKey:key];
    }
}

// Usually overridden by the application
- (())viewDidLoad {
    log_dbg!("[(UIViewController*){:?} viewDidLoad]", this);
}
- (())viewWillAppear:(bool)animated {
    log_dbg!("[(UIViewController*){:?} viewWillAppear:{}]", this, animated);
}
- (())viewDidAppear:(bool)animated {
    log_dbg!("[(UIViewController*){:?} viewDidAppear:{}]", this, animated);
}
- (())viewWillDisappear:(bool)animated {
    log_dbg!("[(UIViewController*){:?} viewWillDisappear:{}]", this, animated);
}
- (())viewDidDisappear:(bool)animated {
    log_dbg!("[(UIViewController*){:?} viewDidDisappear:{}]", this, animated);
}

- (())setTitle:(id)title {
    let old_title = env.objc.borrow::<UIViewControllerHostObject>(this).title;
    release(env, old_title); // Освобождаем старую строку
    retain(env, title);      // Удерживаем новую
    env.objc.borrow_mut::<UIViewControllerHostObject>(this).title = title;
}
- (())setEditing:(bool)editing {
    todo_objc_setter!(this, editing);
}
- (())setWantsFullScreenLayout:(bool)wants {
    todo_objc_setter!(this, wants);
}

- (())dismissModalViewControllerAnimated:(bool)animated {
    log!("TODO: [(UIViewController*){:?} dismissModalViewControllerAnimated:{}]", this, animated); // TODO
}
- (())dismissMoviePlayerViewControllerAnimated {
    log!("TODO: [(UIViewController*){:?} dismissMoviePlayerViewControllerAnimated]", this); // TODO
}

- (bool)shouldAutorotateToInterfaceOrientation:(UIInterfaceOrientation)interface_orientation {
    // ИСПРАВЛЕНИЕ: 3 = LandscapeRight, 4 = LandscapeLeft
    interface_orientation == 3 || interface_orientation == 4
}

// UIResponder implementation
// From the Apple UIView docs regarding [UIResponder nextResponder]:
// "UIViewController similarly implements the method
// and returns its view's superview."
// https://developer.apple.com/documentation/uikit/uiresponder/next?language=objc
- (id)nextResponder {
    let view = msg![env; this view];
    let next_responder = msg![env; view superview];
    log_dbg!("[(UIView*){:?} nextResponder] => {:?}", this, next_responder);
    next_responder
}

- (id)title {
    let stored_title = env.objc.borrow::<UIViewControllerHostObject>(this).title;
    if stored_title != nil {
        stored_title
    } else {
        let class: Class = msg![env; this class];
        NSStringFromClass(env, class)
    }
}

- (bool)isViewLoaded {
    env.objc.borrow::<UIViewControllerHostObject>(this).view != nil
}

- (id)nibName {
    env.objc.borrow::<UIViewControllerHostObject>(this).nib_name
}

- (id)nibBundle {
    env.objc.borrow::<UIViewControllerHostObject>(this).bundle
}

- (())viewDidUnload {
    log_dbg!("[(UIViewController*){:?} viewDidUnload]", this);
}

- (())viewWillLayoutSubviews {
    log_dbg!("[(UIViewController*){:?} viewWillLayoutSubviews]", this);
}

- (())viewDidLayoutSubviews {
    log_dbg!("[(UIViewController*){:?} viewDidLayoutSubviews]", this);
}

- (bool)isEditing {
    false
}

- (())setEditing:(bool)editing animated:(bool)_animated {
    msg![env; this setEditing:editing]
}

- (id)editButtonItem {
    // Return a stub bar button item.
    msg_class![env; UIBarButtonItem new]
}

- (())presentModalViewController:(id)modal_vc animated:(bool)animated {
    log_dbg!(
        "[(UIViewController*){:?} presentModalViewController:{:?} animated:{}]",
        this, modal_vc, animated
    );
    () = msg![env; modal_vc viewWillAppear:animated];
    () = msg![env; modal_vc viewDidAppear:animated];
}

- (id)modalViewController {
    nil
}

- (id)presentingViewController {
    nil
}

- (id)presentedViewController {
    nil
}

- (())presentViewController:(id)vc
                  animated:(bool)animated
                completion:(id)_completion {
    msg![env; this presentModalViewController:vc animated:animated]
}

- (())dismissViewControllerAnimated:(bool)animated
                         completion:(id)_completion {
    msg![env; this dismissModalViewControllerAnimated:animated]
}

- (bool)wantsFullScreenLayout {
    false
}

- (bool)hidesBottomBarWhenPushed {
    false
}

- (())setHidesBottomBarWhenPushed:(bool)_value {
    // TODO
}

- (id)tabBarItem {
    msg_class![env; UITabBarItem new]
}

- (())setTabBarItem:(id)_item {
    // TODO
}

- (id)tabBarController {
    nil
}

- (id)navigationItem {
    // Return a stub navigation item with the VC's class name as title.
    let class: Class = msg![env; this class];
    let class_name: id = NSStringFromClass(env, class);
    let item: id = msg_class![env; UINavigationItem alloc];
    let item: id = msg![env; item initWithTitle:class_name];
    crate::objc::autorelease(env, item)
}

- (())didReceiveMemoryWarning {
    log_dbg!("[(UIViewController*){:?} didReceiveMemoryWarning]", this);
    // Release view if it doesn't have a superview.
    let view = env.objc.borrow::<UIViewControllerHostObject>(this).view;
    if view != nil {
        let superview: id = msg![env; view superview];
        if superview == nil {
            () = msg![env; this viewDidUnload];
            () = msg![env; this setView:nil];
        }
    }
}

- (bool)shouldAutorotate {
    true
}

- (NSUInteger)supportedInterfaceOrientations {
    // UIInterfaceOrientationMaskAll = 0xFF
    0xFF
}

- (UIInterfaceOrientation)preferredInterfaceOrientationForPresentation {
    // ИСПРАВЛЕНИЕ: Форсируем ландшафтный режим
    3
}

- (id)childViewControllers {
    msg_class![env; NSArray new]
}

- (())addChildViewController:(id)_child {
    log_dbg!("[(UIViewController*){:?} addChildViewController:]", this);
}

- (())removeFromParentViewController {
    log_dbg!("[(UIViewController*){:?} removeFromParentViewController]", this);
}

- (())willMoveToParentViewController:(id)_parent {
    // TODO
}

- (())didMoveToParentViewController:(id)_parent {
    // TODO
}

- (())beginAppearanceTransition:(bool)_appearing animated:(bool)_animated {
    // TODO
}

- (())endAppearanceTransition {
    // TODO
}

- (bool)automaticallyForwardAppearanceAndRotationMethodsToChildViewControllers {
    true
}

- (bool)shouldAutomaticallyForwardAppearanceMethods {
    true
}

@end

// --- ИСПРАВЛЕНИЕ: Умные заглушки для пропуска видео и камеры ---

@implementation VideoViewController: UIViewController

- (())viewDidAppear:(bool)animated {
    () = msg_super![env; this viewDidAppear:animated];
    log!("[HACK] VideoViewController auto-closing!");
    
    // Закрываем модальное окно
    () = msg![env; this dismissModalViewControllerAnimated:false];
    
    // На всякий случай удаляем View, если оно было добавлено напрямую
    let view: id = msg![env; this view];
    if view != nil {
        () = msg![env; view removeFromSuperview];
    }
}

@end

@implementation BarcodeReaderViewController: UIViewController

- (())viewDidAppear:(bool)animated {
    () = msg_super![env; this viewDidAppear:animated];
    log!("[HACK] BarcodeReaderViewController auto-closing!");
    
    () = msg![env; this dismissModalViewControllerAnimated:false];
    let view: id = msg![env; this view];
    if view != nil {
        () = msg![env; view removeFromSuperview];
    }
}

@end

};

/// A helper function to resolve suitable NIB name for a `view_controller`
/// in the `bundle`. Returns nil if fails.
///
/// Note: It's a responsibility of a caller to release the returned name
/// if not-nil!
fn get_nib_name(env: &mut Environment, view_controller: id, bundle: id) -> id {
    let provider_nib_name: id = env
        .objc
        .borrow::<UIViewControllerHostObject>(view_controller)
        .nib_name;
        
    if provider_nib_name != nil {
        let resolved = check_and_resolve_nib(env, bundle, provider_nib_name);
        if resolved != nil {
            return resolved;
        }
    }

    let class: Class = msg![env; view_controller class];
    let class_name: id = NSStringFromClass(env, class);
    let class_name_str = to_rust_string(env, class_name);

    if let Some(name) = class_name_str.strip_suffix("Controller") {
        let ns_name: id = from_rust_string(env, name.to_string());
        let resolved = check_and_resolve_nib(env, bundle, ns_name);
        if resolved != nil {
            release(env, class_name);
            return resolved;
        }
    }

    let resolved = check_and_resolve_nib(env, bundle, class_name);
    if resolved != nil {
        // FIX: release class_name before returning — previously leaked here
        release(env, class_name);
        return resolved;
    }

    release(env, class_name);
    nil
}

/// Умный помощник для поиска NIB файлов. Учитывает регистр букв (важно для
/// Android) и все возможные варианты суффиксов устройств
/// (~iphone, -iPhone и т.д.)
fn check_and_resolve_nib(env: &mut Environment, bundle: id, base_name: id) -> id {
    if base_name == nil {
        return nil;
    }
    let type_: id = get_static_str(env, "nib");
    let base_name_str = to_rust_string(env, base_name);
    
    // ИСПРАВЛЕНИЕ: Используем .to_string() вместо .clone(),
    // чтобы типы в массиве совпадали
    let bases = [base_name_str.to_string(), base_name_str.to_lowercase()];
    
    // Перебираем все варианты окончаний, которые использовали старые игры
    let suffixes = ["", "~iphone", "~ipad", "-iPhone", "-iPad", "_iPhone", "_iPad"];
    
    for base in &bases {
        for suffix in &suffixes {
            let candidate = format!("{}{}", base, suffix);
            let candidate_ns: id = from_rust_string(env, candidate);
            let path: id = msg![env; bundle pathForResource:candidate_ns ofType:type_];
            
            if path != nil {
                retain(env, candidate_ns);
                return candidate_ns;
            }
        }
    }
    
    nil
}

