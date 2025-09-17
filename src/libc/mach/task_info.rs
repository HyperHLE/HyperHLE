/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `mach/task_info.h`

use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::libc::mach::semaphore::task_t;
use crate::libc::mach::thread_info::kern_return_t;
use crate::mem::{GuestUSize, MutVoidPtr};

fn task_info(
    _env: &mut Environment,
    _task_name: task_t,
    _task_flavor: u32,
    _task_info: MutVoidPtr,
    _task_info_count: GuestUSize,
) -> kern_return_t {
    log!("idk");
    -1
}
/*
const TASK_THREAD_TIMES_INFO: u32 = 3;
const TASK_BASIC_INFO: u32 = 4;

fn task_info(
    env: &mut Environment,
    task_name: task_t,
    task_flavor: u32,
    _task_info: MutVoidPtr,
    _task_info_count: GuestUSize,
) -> kern_return_t {
    assert!(task_name == MACH_TASK_SELF);
    match task_flavor {

        _ => {
            log!("Unsupported task_info flavor {task_flavor}");
            -1
        }
    }
}
*/
pub const FUNCTIONS: FunctionExports = &[export_c_func!(task_info(_, _, _, _))];
