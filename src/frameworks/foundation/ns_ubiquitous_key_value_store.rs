/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 */
//! `NSUbiquitousKeyValueStore`

use crate::objc::{id, msg, msg_class, objc_classes, ClassExports};

#[derive(Default)]
pub struct State {
    pub default_store: Option<id>,
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSUbiquitousKeyValueStore: NSObject

+ (id)defaultStore {
    // Паттерн синглтона, как в NSFileManager
    if let Some(existing) = env.framework_state.foundation.ns_ubiquitous_key_value_store.default_store {
        existing
    } else {
        let new: id = msg![env; this new];
        env.framework_state.foundation.ns_ubiquitous_key_value_store.default_store = Some(new);
        new
    }
}

- (bool)synchronize {
    // Используем локальное хранилище вместо iCloud
    let defaults: id = msg_class![env; NSUserDefaults standardUserDefaults];
    msg![env; defaults synchronize]
}

- (id)objectForKey:(id)key {
    let defaults: id = msg_class![env; NSUserDefaults standardUserDefaults];
    msg![env; defaults objectForKey:key]
}

- (())setObject:(id)obj forKey:(id)key {
    let defaults: id = msg_class![env; NSUserDefaults standardUserDefaults];
    msg![env; defaults setObject:obj forKey:key]
}

- (i64)longLongForKey:(id)key {
    let obj: id = msg![env; this objectForKey:key];
    if obj != crate::objc::nil {
        msg![env; obj longLongValue]
    } else {
        0
    }
}

- (())setLongLong:(i64)value forKey:(id)key {
    let num: id = msg_class![env; NSNumber numberWithLongLong:value];
    msg![env; this setObject:num forKey:key]
}

@end

};
