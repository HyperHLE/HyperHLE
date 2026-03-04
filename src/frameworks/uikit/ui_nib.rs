/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UINib` and loading of nib files.
//!
//! Resources:
//! - Apple's [Resource Programming Guide](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/LoadingResources/CocoaNibs/CocoaNibs.html) is very helpful.
//! - GitHub user 0xced's [reverse-engineering of UIClassSwapper](https://gist.github.com/0xced/45daf79b62ad6a20be1c).

use crate::frameworks::foundation::ns_string::{get_static_str, to_rust_string};
use crate::frameworks::foundation::{ns_string, NSUInteger};
use crate::frameworks::uikit::ui_view::ui_control::UIControlEvents;
use crate::fs::GuestPathBuf;
use crate::objc::{
    autorelease, id, impl_HostObject_with_superclass, msg, msg_class, msg_super, nil, objc_classes,
    release, retain, Class, ClassExports, HostObject,
};
use crate::Environment;

#[derive(Default)]
struct UINibHostObject {
    /// `NSString*`
    nib_name: id,
    /// `NSBundle*`
    bundle: id,
    /// File's Owner
    /// (weak, non-retaining)
    file_owner: id,
}
impl HostObject for UINibHostObject {}

#[derive(Default)]
struct UIRuntimeConnectionHostObject {
    destination: id,
    label: id,
    source: id,
}
impl HostObject for UIRuntimeConnectionHostObject {}

#[derive(Default)]
struct UIRuntimeEventConnectionHostObject {
    superclass: UIRuntimeConnectionHostObject,
    event_mask: UIControlEvents,
}
impl_HostObject_with_superclass!(UIRuntimeEventConnectionHostObject);

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UINib: NSObject

+ (id)nibWithNibName:(id)nib_name bundle:(id)bundle {
    let main_bundle = msg_class![env; NSBundle mainBundle];
    let bundle: id = if bundle == nil { main_bundle } else { bundle };
    retain(env, nib_name);
    retain(env, bundle);
    let host_object = Box::new(UINibHostObject {
        nib_name,
        bundle,
        file_owner: nil
    });
    let new = env.objc.alloc_object(this, host_object, &mut env.mem);
    autorelease(env, new)
}

- (())dealloc {
    let &UINibHostObject { nib_name, bundle, .. } = env.objc.borrow(this);
    release(env, nib_name);
    release(env, bundle);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)instantiateWithOwner:(id)owner options:(id)options {
    assert!(owner != nil);
    assert!(options == nil);
    let nib = env.objc.borrow_mut::<UINibHostObject>(this);
    nib.file_owner = owner;

    let bundle = nib.bundle;
    let nib_name = nib.nib_name;
    let path: id = msg![env; bundle pathForResource:nib_name ofType:get_static_str(env, "nib")];
    assert!(path != nil && msg![env; path isAbsolutePath]);

    let unarchiver = load_nib_file(env, this, GuestPathBuf::from(to_rust_string(env, path))).unwrap();
    let top_level_objects_key = get_static_str(env, "UINibTopLevelObjectsKey");
    let top_level_objects = msg![env; unarchiver decodeObjectForKey:top_level_objects_key];
    release(env, unarchiver);
    nib.file_owner = nil;

    top_level_objects
}

@end

@implementation UIProxyObject: NSObject

- (id)initWithCoder:(id)coder {
    let id_key = get_static_str(env, "UIProxiedObjectIdentifier");
    let id_nss: id = msg![env; coder decodeObjectForKey:id_key];
    let id_str = to_rust_string(env, id_nss);

    if id_str == "IBFilesOwner" {
        let delegate: id = msg![env; coder delegate];
        assert!(delegate != nil);
        env.objc.borrow::<UINibHostObject>(delegate).file_owner
    } else if id_str == "IBFirstResponder" {
        nil
    } else {
        this
    }
}

@end

@implementation UIClassSwapper: NSObject

- (id)initWithCoder:(id)coder {
    let name_nss: id = msg![env; coder decodeObjectForKey:get_static_str(env, "UIClassName")];
    let name = to_rust_string(env, name_nss);
    let orig_nss: id = msg![env; coder decodeObjectForKey:get_static_str(env, "UIOriginalClassName")];
    let orig = to_rust_string(env, orig_nss);

    let class = env.objc.get_known_class(&name, &mut env.mem);
    let object: id = msg![env; class alloc];
    let object: id = if orig == "UICustomObject" { msg![env; object init] } else { msg![env; object initWithCoder:coder] };
    release(env, this);
    object
}

@end

@implementation UIRuntimeConnection: NSObject

+ (id)alloc {
    let host_object = Box::<UIRuntimeConnectionHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithCoder:(id)coder {
    let host_obj = env.objc.borrow_mut::<UIRuntimeConnectionHostObject>(this);
    host_obj.destination = msg![env; coder decodeObjectForKey:get_static_str(env, "UIDestination")];
    host_obj.label = msg![env; coder decodeObjectForKey:get_static_str(env, "UILabel")];
    host_obj.source = msg![env; coder decodeObjectForKey:get_static_str(env, "UISource")];
    this
}

- (())connect {
    let host = env.objc.borrow::<UIRuntimeConnectionHostObject>(this);
    let _ = std::panic::catch_unwind(|| { () = msg![env; host.source setValue:host.destination forKey:host.label]; });
}

- (())dealloc {
    let host = env.objc.borrow(this);
    release(env, host.destination);
    release(env, host.label);
    release(env, host.source);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation UIRuntimeEventConnection: UIRuntimeConnection

+ (id)alloc {
    let host_object = Box::<UIRuntimeEventConnectionHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())connect {
    let host = env.objc.borrow::<UIRuntimeConnectionHostObject>(this);
    let _ = std::panic::catch_unwind(|| { () = msg![env; host.source setValue:host.destination forKey:host.label]; });
}

- (id)initWithCoder:(id)coder {
    let this: id = msg_super![env; this initWithCoder:coder];
    let host_obj = env.objc.borrow_mut::<UIRuntimeEventConnectionHostObject>(this);
    host_obj.event_mask = msg![env; coder decodeIntForKey:get_static_str(env, "UIEventMask")] as UIControlEvents;
    this
}

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation UIRuntimeOutletConnection: UIRuntimeConnection

- (())connect {
    let host = env.objc.borrow::<UIRuntimeConnectionHostObject>(this);
    () = msg![env; host.source setValue:host.destination forKey:host.label];
}

@end

};

/// Takes a [GuestPathBuf] where a nib file is located and deserializes it.
/// Returns an empty [Err] if the file couldn't be loaded or an [Ok] wrapping
/// an NSKeyedUnarchiver.
/// The unarchiver should later be manually [release]d
fn load_nib_file(env: &mut Environment, ui_nib: id, path: GuestPathBuf) -> Result<id, ()> {
    let path = ns_string::from_rust_string(env, path.as_str().to_string());
    assert!(msg![env; path isAbsolutePath]);
    let ns_data: id = msg_class![env; NSData dataWithContentsOfFile:path];
    if ns_data == nil {
        // Apparently it's permitted to specify the nib file key in the
        // Info.plist, yet not have it point to a valid nib file?!
        log!("Warning: couldn't load nib file {:?}", path);
        return Err(());
    };

    let unarchiver = msg_class![env; NSKeyedUnarchiver alloc];
    let unarchiver = msg![env; unarchiver initForReadingWithData:ns_data];

    // ui_nib will hold a file's owner,
    // which will replace corresponding UIProxyObject
    () = msg![env; unarchiver setDelegate:ui_nib];

    // The top-level keys in a nib file's keyed archive appear to be
    // UINibAccessibilityConfigurationsKey, UINibConnectionsKey,
    // UINibObjectsKey, UINibTopLevelObjectsKey and UINibVisibleWindowsKey.
    // Each corresponds to an NSArray.

    // We don't need to do anything with the list of objects, but deserializing
    // it ensures everything else is deserialized.
    let objects_key = get_static_str(env, "UINibObjectsKey");
    let _objects: id = msg![env; unarchiver decodeObjectForKey:objects_key];

    // Connect all the outlets with UIRuntimeOutletConnection
    let conns_key = get_static_str(env, "UINibConnectionsKey");
    let conns: id = msg![env; unarchiver decodeObjectForKey:conns_key];
    let conns_count: NSUInteger = msg![env; conns count];
    for i in 0..conns_count {
        let conn: id = msg![env; conns objectAtIndex:i];
        () = msg![env; conn connect];
    }

    // Make visible windows visible
    let visibles_key = get_static_str(env, "UINibVisibleWindowsKey");
    let visibles: id = msg![env; unarchiver decodeObjectForKey:visibles_key];
    let visibles_count: NSUInteger = msg![env; visibles count];
    for i in 0..visibles_count {
        let visible: id = msg![env; visibles objectAtIndex:i];
        () = msg![env; visible setHidden:false];
    }

    Ok(unarchiver)
}
