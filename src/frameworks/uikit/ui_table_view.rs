use crate::objc::{id, msg, msg_class};
use crate::environment::Environment; // ✅

pub fn register_table_view_classes(env: &mut Environment) {
    // Пример регистрации UITableView
    env.objc.register_class("UITableView", Some("UIView"), |env, this| {
        // инициализация экземпляра
        let _self: id = msg![env; this init];
        _self
    });

    // Пример регистрации UITableViewCell
    env.objc.register_class("UITableViewCell", Some("UIView"), |env, this| {
        let _self: id = msg![env; this init];
        _self
    });
}
