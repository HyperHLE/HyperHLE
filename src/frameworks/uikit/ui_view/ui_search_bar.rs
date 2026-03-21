/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UISearchBar`.

use crate::frameworks::core_graphics::CGRect;
use crate::objc::{id, msg_super, nil, objc_classes, ClassExports};

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UISearchBar: UIView

- (id)initWithFrame:(CGRect)frame {
    msg_super![env; this initWithFrame:frame]
}

- (id)delegate { nil }
- (())setDelegate:(id)_delegate {}
- (id)text { nil }
- (())setText:(id)_text {}
- (id)placeholder { nil }
- (())setPlaceholder:(id)_placeholder {}
- (id)prompt { nil }
- (())setPrompt:(id)_prompt {}
- (())setBarStyle:(i32)_style {}
- (i32)barStyle { 0 }
- (())setSearchBarStyle:(i32)_style {}
- (i32)searchBarStyle { 0 }
- (())setShowsCancelButton:(bool)_shows {}
- (())setShowsCancelButton:(bool)_shows animated:(bool)_animated {}
- (bool)showsCancelButton { false }
- (())setShowsBookmarkButton:(bool)_shows {}
- (bool)showsBookmarkButton { false }
- (())setShowsSearchResultsButton:(bool)_shows {}
- (bool)showsSearchResultsButton { false }
- (())setSearchResultsButtonSelected:(bool)_selected {}
- (bool)isSearchResultsButtonSelected { false }
- (())setAutocorrectionType:(i32)_type {}
- (())setAutocapitalizationType:(i32)_type {}
- (())setKeyboardType:(i32)_type {}
- (())setReturnKeyType:(i32)_type {}
- (())setSpellCheckingType:(i32)_type {}
- (())setEnablesReturnKeyAutomatically:(bool)_enables {}
- (())setTintColor:(id)_color {}
- (())setBarTintColor:(id)_color {}
- (())setTranslucent:(bool)_translucent {}
- (bool)isTranslucent { false }
- (())setInputAccessoryView:(id)_view {}
- (id)inputAccessoryView { nil }
- (())setInputView:(id)_view {}
- (id)inputView { nil }
- (())setScopeButtonTitles:(id)_titles {}
- (id)scopeButtonTitles { nil }
- (())setSelectedScopeButtonIndex:(i32)_index {}
- (i32)selectedScopeButtonIndex { 0 }
- (())setShowsScopeBar:(bool)_shows {}
- (bool)showsScopeBar { false }
- (())setBackgroundImage:(id)_image {}
- (id)backgroundImage { nil }
- (())setScopeBarBackgroundImage:(id)_image {}
- (())setSearchFieldBackgroundImage:(id)_image forState:(i32)_state {}
- (())setImage:(id)_image forSearchBarIcon:(i32)_icon state:(i32)_state {}
- (())setScopeBarButtonBackgroundImage:(id)_image forState:(i32)_state {}
- (())setScopeBarButtonDividerImage:(id)_image forLeftSegmentState:(i32)_left rightSegmentState:(i32)_right {}
- (())setScopeBarButtonTitleTextAttributes:(id)_attributes forState:(i32)_state {}
- (())setSearchFieldBackgroundPositionAdjustment:(id)_offset {}
- (())setSearchTextPositionAdjustment:(id)_offset {}
- (())setPositionAdjustment:(id)_offset forSearchBarIcon:(i32)_icon {}
- (bool)becomeFirstResponder { false }
- (bool)resignFirstResponder { true }
- (bool)isFirstResponder { false }

@end

};
