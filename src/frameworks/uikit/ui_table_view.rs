use crate::{msg};
use crate::objc::{id, Class};
use crate::objc_classes;

objc_classes! {
    (env, this, _cmd);

    // методы класса/экземпляра идут прямо здесь
    - (id)init {
        msg![env; this init] // пример вызова суперкласса
    }

    - (NSUInteger)numberOfRowsInSection:(NSUInteger)section {
        0
    }

    - (id)cellForRowAtIndexPath:(id)indexPath {
        id::null()
    }

    - (())reloadData {
        log!("UITableView reloadData called");
    }
}
