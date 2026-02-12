/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 */

use crate::objc::{id, msg, msg_class, Environment};

pub fn register_table_view_cell(env: &mut Environment) {
    env.objc.register_class("UITableViewCell", Some("UIView"), |env, this| {
        let _self: id = msg![env; this init];
        _self
    });
}
