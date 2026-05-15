/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! StoreKit

use crate::dyld::{ConstantExports, HostConstant};

mod sk_payment_queue;
mod sk_product;

const SKStoreProductParameterITunesItemIdentifier: &str =
    "SKStoreProductParameterITunesItemIdentifier";

pub const CONSTANTS: ConstantExports = &[(
    "_SKStoreProductParameterITunesItemIdentifier",
    HostConstant::NSString(SKStoreProductParameterITunesItemIdentifier),
)];

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/StoreKit.framework/StoreKit",
    aliases: &[],
    class_exports: &[sk_payment_queue::CLASSES, sk_product::CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[],
};

#[derive(Default)]
pub struct State {
    pub payment_queue: sk_payment_queue::State,
}
