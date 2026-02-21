use crate::objc_classes;
use crate::frameworks::uikit::UIViewController;
use crate::Environment;

pub fn register_classes(env: &mut Environment) {
    objc_classes! {
        (env, this, _cmd);

        @implementation MainViewController : UIViewController
        @end
    }
}
