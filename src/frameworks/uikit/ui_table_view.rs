use crate::{msg};
use crate::objc::{id, Class};
use crate::objc_classes;

objc_classes! {
    (env, this, _cmd);

    class UITableView: UIView {

        - (id)init {
            log!("UITableView init");
            msg![env; this init]
        }

        - (NSUInteger)numberOfRowsInSection:(NSUInteger)section {
            0 // заглушка
        }

        - (id)cellForRowAtIndexPath:(id)indexPath {
            id::null() // заглушка
        }

        - (())reloadData {
            log!("UITableView reloadData called");
        }
    }
}
