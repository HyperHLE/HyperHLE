use crate::objc_classes;
use crate::frameworks::uikit::UIViewController;

objc_classes! {
    (env, this, _cmd);

    @implementation MainViewController : UIViewController
    @end
}
