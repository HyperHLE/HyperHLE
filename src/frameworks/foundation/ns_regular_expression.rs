/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::objc::{id, SEL, ClassExports, objc_classes, NSZonePtr, HostObject};
use crate::log;
use crate::Environment;
use crate::frameworks::foundation::NSRange;

// Define a dummy struct to hold the "nothing" this class needs
struct NSRegularExpressionHostObject;
impl HostObject for NSRegularExpressionHostObject {}

pub const CLASSES: ClassExports = objc_classes! {
    (env, this, _cmd);

    @implementation NSRegularExpression: NSObject

    + (id)allocWithZone:(NSZonePtr)_zone {
        let host_obj = Box::new(NSRegularExpressionHostObject);
        let instance = (*env.objc).alloc_object(this, host_obj, &mut env.mem);
        log!("NSRegularExpression: Created instance {:?}.", instance);
        instance
    }

    - (id)initWithPattern:(id)_pattern options:(u64)_options error:(id)_error {
        log!("NSRegularExpression: initWithPattern stubbed.");
        this
    }

    // THIS IS THE NEW PART
    - (id)firstMatchInString:(id)_string options:(u64)_options range:(NSRange)_range {
        log!("NSRegularExpression: firstMatchInString returning nil.");
        crate::objc::nil
    }

    - (id)matchesInString:(id)_string options:(u64)_options range:(NSRange)_range {
        log!("NSRegularExpression: matchesInString returning nil.");
        crate::objc::nil
    }

    @end
};
