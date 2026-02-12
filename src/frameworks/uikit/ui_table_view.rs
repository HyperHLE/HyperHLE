use crate::objc::{id, msg, msg_send, msg_class};
use crate::environment::Environment;
use crate::libc::NSUInteger;

crate::objc_classes! {
    (env, this, _cmd);

    @class UITableView : UIView

    - (id)initWithFrame:(CGRect)frame style:(NSInteger)style {
        // В конструкторе можно вызывать супер-метод
        let super_res: id = msg_send(env, (super(this), "initWithFrame:", frame));
        super_res
    }

    - (NSInteger)numberOfSections {
        1
    }

    - (NSInteger)tableView:(id)tableView numberOfRowsInSection:(NSInteger)section {
        0
    }

    - (id)tableView:(id)tableView cellForRowAtIndexPath:(id)indexPath {
        nil
    }
}
