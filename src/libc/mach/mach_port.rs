/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Local Mach port namespace. Receive rights and send user references are
//! tracked per Environment; port sets and send-once rights are not yet supported.

use std::collections::{HashMap, VecDeque};

use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::mach::init::MACH_TASK_SELF;
use crate::libc::mach::thread_info::{
    kern_return_t, KERN_INVALID_ARGUMENT, KERN_NO_SPACE, KERN_SUCCESS,
};
use crate::mem::MutPtr;
use crate::Environment;

const KERN_INVALID_NAME: i32 = 15;
const KERN_INVALID_RIGHT: i32 = 17;
const KERN_UREFS_OVERFLOW: i32 = 19;

#[derive(Default)]
pub(super) struct Port {
    pub send_refs: u32,
    pub messages: VecDeque<Vec<u8>>,
}

pub(crate) struct State {
    next_name: u32,
    pub(super) ports: HashMap<u32, Port>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            next_name: 0x100,
            ports: HashMap::new(),
        }
    }
}

impl State {
    fn allocate(&mut self) -> Option<u32> {
        let name = self.next_name;
        self.next_name = name.checked_add(4)?;
        self.ports.insert(name, Port::default());
        Some(name)
    }
}

fn mach_port_allocate(
    env: &mut Environment,
    task: u32,
    right: u32,
    name: MutPtr<u32>,
) -> kern_return_t {
    if task != MACH_TASK_SELF || name.is_null() || right != 1 {
        return KERN_INVALID_ARGUMENT;
    }
    let Some(port) = env.libc_state.mach_ports.allocate() else {
        return KERN_NO_SPACE;
    };
    env.mem.write(name, port);
    KERN_SUCCESS
}

fn mach_port_deallocate(env: &mut Environment, task: u32, name: u32) -> kern_return_t {
    // Deallocation releases a send user reference, not the receive right.
    mach_port_mod_refs(env, task, name, 0, -1)
}

fn mach_port_insert_right(
    env: &mut Environment,
    task: u32,
    name: u32,
    poly: u32,
    disposition: u32,
) -> kern_return_t {
    if task != MACH_TASK_SELF || name != poly {
        return KERN_INVALID_ARGUMENT;
    }
    let Some(port) = env.libc_state.mach_ports.ports.get_mut(&name) else {
        return KERN_INVALID_NAME;
    };
    match disposition {
        20 => (), // MAKE_SEND from the receive right
        19 if port.send_refs != 0 => (), // COPY_SEND
        17 if port.send_refs != 0 => return KERN_SUCCESS, // MOVE_SEND to the same name
        _ => return KERN_INVALID_RIGHT,
    }
    let Some(refs) = port.send_refs.checked_add(1) else {
        return KERN_UREFS_OVERFLOW;
    };
    port.send_refs = refs;
    KERN_SUCCESS
}

fn mach_port_mod_refs(
    env: &mut Environment,
    task: u32,
    name: u32,
    right: u32,
    delta: i32,
) -> kern_return_t {
    if task != MACH_TASK_SELF {
        return KERN_INVALID_ARGUMENT;
    }
    let Some(port) = env.libc_state.mach_ports.ports.get_mut(&name) else {
        return KERN_INVALID_NAME;
    };
    match right {
        0 if port.send_refs != 0 => {
            let refs = i64::from(port.send_refs) + i64::from(delta);
            if refs < 0 {
                return KERN_INVALID_ARGUMENT;
            }
            if refs > i64::from(u32::MAX) {
                return KERN_UREFS_OVERFLOW;
            }
            port.send_refs = refs as u32;
        }
        1 if delta == 0 => (),
        1 if delta == -1 => {
            env.libc_state.mach_ports.ports.remove(&name);
        }
        1 => return KERN_INVALID_ARGUMENT,
        _ => return KERN_INVALID_RIGHT,
    }
    KERN_SUCCESS
}

fn mach_port_destroy(env: &mut Environment, task: u32, name: u32) -> kern_return_t {
    if task != MACH_TASK_SELF {
        return KERN_INVALID_ARGUMENT;
    }
    if env.libc_state.mach_ports.ports.remove(&name).is_none() {
        return KERN_INVALID_NAME;
    }
    KERN_SUCCESS
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(mach_port_allocate(_, _, _)),
    export_c_func!(mach_port_deallocate(_, _)),
    export_c_func!(mach_port_insert_right(_, _, _, _)),
    export_c_func!(mach_port_mod_refs(_, _, _, _)),
    export_c_func!(mach_port_destroy(_, _)),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_is_environment_local_and_names_are_not_reused() {
        let mut first = State::default();
        let mut second = State::default();
        assert_eq!(first.allocate(), Some(0x100));
        assert_eq!(second.allocate(), Some(0x100));
        first.ports.remove(&0x100);
        assert_eq!(first.allocate(), Some(0x104));
        assert!(second.ports.contains_key(&0x100));
    }
}
