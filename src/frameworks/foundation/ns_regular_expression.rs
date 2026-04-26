/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::objc::{id, SEL, ClassExports, objc_classes, NSZonePtr};
use crate::log;
use crate::Environment;
use crate::frameworks::foundation::NSRange;

pub const CLASSES: ClassExports = objc_classes! {
    (env, this, _cmd);

    @implementation NSRegularExpression: NSObject

    + (id)allocWithZone:(NSZonePtr)_zone {
        // This is the macro equivalent of class.create_instance()
        let instance = (*env.objc).alloc_object(this, (), &mut env.mem);
        log!("NSRegularExpression: Created instance {:?}.", instance);
        instance
    }

    - (id)initWithPattern:(id)_pattern options:(u64)_options error:(id)_error {
        log!("NSRegularExpression: initWithPattern called.");
        this
    }

    - (id)matchesInString:(id)_string options:(u64)_options range:(NSRange)_range {
        log!("NSRegularExpression: matchesInString called. Returning nil.");
        crate::objc::nil
    }

    @end
};
