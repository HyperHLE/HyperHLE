use crate::objc::{class, msg, runtime};
use crate::frameworks::uikit::ui_view::UIView;

pub fn register_uISearchBar() {
    class!(
        pub struct UISearchBar: UIView {}

        unsafe impl ClassType for UISearchBar {
            fn class_name() -> &'static str {
                "UISearchBar"
            }
        }

        impl UISearchBar {
            #[msg(send = alloc)]
            fn alloc() -> *mut Self {
                unsafe { runtime::alloc_object::<Self>() }
            }

            #[msg(send = init)]
            fn init(&mut self) -> *mut Self {
                self
            }
        }
    );
}
