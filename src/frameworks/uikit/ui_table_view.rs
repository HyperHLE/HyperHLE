/*
 * Mozilla Public License v2.0
 */

use crate::frameworks::core_graphics::cg_geometry::{CGRect, CGSize};
use crate::frameworks::foundation::ns_string::from_rust_string;
use crate::objc::{
    autorelease, id, msg, msg_class, msg_send, nil, objc_classes, retain, Class,
    ClassExports, ObjC, SEL,
};

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

#pragma mark - UITableViewCell

@implementation UITableViewCell : UIView {
    _reuseIdentifier: id,
    _contentView: id,
    _selected: bool,
    _highlighted: bool,
}

+ (id)alloc {
    msg![env; this allocWithZone:0]
}

- (id)initWithFrame:(CGRect)frame {
    let this: id = msg![env; super initWithFrame:frame];

    if this.is_null() {
        return nil;
    }

    let content_view_class = env.objc.get_known_class("UIView", &mut env.mem);
    let content: id = msg![env; content_view_class alloc];
    let content: id = msg![env; content initWithFrame:frame];

    if let Some(ivar) = env.objc.object_lookup_ivar(&env.mem, this, "_contentView") {
        retain(env, content);
        env.mem.write(ivar.cast(), content);
    }

    () = msg![env; this addSubview:content];

    this
}

- (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)reuseIdentifier {
    let this: id = msg![env; this initWithFrame:frame];

    if !reuseIdentifier.is_null() {
        if let Some(ivar) = env.objc.object_lookup_ivar(&env.mem, this, "_reuseIdentifier") {
            retain(env, reuseIdentifier);
            env.mem.write(ivar.cast(), reuseIdentifier);
        }
    }

    this
}

- (id)reuseIdentifier {
    env.objc
        .object_lookup_ivar(&env.mem, this, "_reuseIdentifier")
        .map(|p| env.mem.read(p.cast()))
        .unwrap_or(nil)
}

- (id)contentView {
    env.objc
        .object_lookup_ivar(&env.mem, this, "_contentView")
        .map(|p| env.mem.read(p.cast()))
        .unwrap_or(nil)
}

- (())setSelected:(bool)selected animated:(bool)_animated {
    if let Some(ivar) = env.objc.object_lookup_ivar(&env.mem, this, "_selected") {
        env.mem.write(ivar.cast(), selected);
    }
}

- (())setHighlighted:(bool)highlighted animated:(bool)_animated {
    if let Some(ivar) = env.objc.object_lookup_ivar(&env.mem, this, "_highlighted") {
        env.mem.write(ivar.cast(), highlighted);
    }
}

@end


#pragma mark - UITableView

@implementation UITableView : UIScrollView {
    _dataSource: id,
    _delegate: id,
    _rowHeight: f32,
}

+ (id)alloc {
    msg![env; this allocWithZone:0]
}

- (id)initWithFrame:(CGRect)frame {
    let this: id = msg![env; super initWithFrame:frame];

    if let Some(ivar) = env.objc.object_lookup_ivar(&env.mem, this, "_rowHeight") {
        env.mem.write(ivar.cast(), 44.0f32);
    }

    this
}

- (())setDataSource:(id)ds {
    if let Some(ivar) = env.objc.object_lookup_ivar(&env.mem, this, "_dataSource") {
        retain(env, ds);
        env.mem.write(ivar.cast(), ds);
    }
}

- (id)dataSource {
    env.objc
        .object_lookup_ivar(&env.mem, this, "_dataSource")
        .map(|p| env.mem.read(p.cast()))
        .unwrap_or(nil)
}

- (())setDelegate:(id)delegate {
    if let Some(ivar) = env.objc.object_lookup_ivar(&env.mem, this, "_delegate") {
        retain(env, delegate);
        env.mem.write(ivar.cast(), delegate);
    }
}

- (id)delegate {
    env.objc
        .object_lookup_ivar(&env.mem, this, "_delegate")
        .map(|p| env.mem.read(p.cast()))
        .unwrap_or(nil)
}

- (())reloadData {
    // iOS 2.x compatible no-op
}

- (f32)rowHeight {
    env.objc
        .object_lookup_ivar(&env.mem, this, "_rowHeight")
        .map(|p| env.mem.read(p.cast()))
        .unwrap_or(44.0)
}

- (())setRowHeight:(f32)h {
    if let Some(ivar) = env.objc.object_lookup_ivar(&env.mem, this, "_rowHeight") {
        env.mem.write(ivar.cast(), h);
    }
}

@end

};
