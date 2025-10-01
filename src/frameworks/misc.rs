/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::Environment;

fn __mb_cur_max(_env: &mut Environment) -> i32 {
    1
}

pub const FUNCTIONS: FunctionExports = &[export_c_func!(__mb_cur_max())];

pub const CONSTANTS: ConstantExports = &[
    (
        "_AVAudioSessionCategoryAmbient",
        HostConstant::NSString("AVAudioSessionCategoryAmbient"),
    ),
];
