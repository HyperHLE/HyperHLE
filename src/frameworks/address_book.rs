/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#![allow(dead_code)]
//! AddressBook.framework stub

use crate::dyld::{export_c_func, FunctionExports, HostDylib};
use crate::frameworks::core_foundation::CFIndex;
use crate::frameworks::core_foundation::cf_string::CFStringRef;
use crate::mem::{MutVoidPtr, Ptr};
use crate::Environment;

type ABAddressBookRef = MutVoidPtr;
type ABRecordRef = MutVoidPtr;
type ABMultiValueRef = MutVoidPtr;
type CFErrorRef = MutVoidPtr;
type ABPropertyID = i32;
type ABRecordType = u32;

// ABAuthorizationStatus
const kABAuthorizationStatusNotDetermined: i32 = 0;
const kABAuthorizationStatusAuthorized: i32 = 3;

// MARK: - Address book lifecycle

fn ABAddressBookCreate(_env: &mut Environment) -> ABAddressBookRef {
    log!("ABAddressBookCreate stubbed, returning NULL");
    Ptr::null()
}

fn ABAddressBookCreateWithOptions(
    _env: &mut Environment,
    _options: MutVoidPtr,
    _error: MutVoidPtr,
) -> ABAddressBookRef {
    log!("ABAddressBookCreateWithOptions stubbed, returning NULL");
    Ptr::null()
}

fn ABAddressBookSave(
    _env: &mut Environment,
    _address_book: ABAddressBookRef,
    _error: MutVoidPtr,
) -> bool {
    log!("ABAddressBookSave stubbed, returning false");
    false
}

fn ABAddressBookHasUnsavedChanges(
    _env: &mut Environment,
    _address_book: ABAddressBookRef,
) -> bool {
    false
}

fn ABAddressBookRevert(_env: &mut Environment, _address_book: ABAddressBookRef) {
    log!("ABAddressBookRevert stubbed");
}

fn ABAddressBookRequestAccessWithCompletion(
    _env: &mut Environment,
    _address_book: ABAddressBookRef,
    _completion: MutVoidPtr,
) {
    log!("ABAddressBookRequestAccessWithCompletion stubbed (access never granted)");
}

fn ABAddressBookGetAuthorizationStatus(_env: &mut Environment) -> i32 {
    log!("ABAddressBookGetAuthorizationStatus stubbed, returning NotDetermined");
    kABAuthorizationStatusNotDetermined
}

// MARK: - Person count / lookup

fn ABAddressBookGetPersonCount(
    _env: &mut Environment,
    _address_book: ABAddressBookRef,
) -> i32 {
    0
}

fn ABAddressBookGetPersonWithRecordID(
    _env: &mut Environment,
    _address_book: ABAddressBookRef,
    _record_id: i32,
) -> ABRecordRef {
    Ptr::null()
}

fn ABAddressBookCopyArrayOfAllPeople(
    _env: &mut Environment,
    _address_book: ABAddressBookRef,
) -> MutVoidPtr {
    log!("ABAddressBookCopyArrayOfAllPeople stubbed, returning NULL");
    Ptr::null()
}

fn ABAddressBookCopyArrayOfAllPeopleInSource(
    _env: &mut Environment,
    _address_book: ABAddressBookRef,
    _source: ABRecordRef,
) -> MutVoidPtr {
    log!("ABAddressBookCopyArrayOfAllPeopleInSource stubbed, returning NULL");
    Ptr::null()
}

fn ABAddressBookCopyPeopleWithName(
    _env: &mut Environment,
    _address_book: ABAddressBookRef,
    _name: CFStringRef,
) -> MutVoidPtr {
    log!("ABAddressBookCopyPeopleWithName stubbed, returning NULL");
    Ptr::null()
}

// MARK: - Record add / remove

fn ABAddressBookAddRecord(
    _env: &mut Environment,
    _address_book: ABAddressBookRef,
    _record: ABRecordRef,
    _error: MutVoidPtr,
) -> bool {
    log!("ABAddressBookAddRecord stubbed, returning false");
    false
}

fn ABAddressBookRemoveRecord(
    _env: &mut Environment,
    _address_book: ABAddressBookRef,
    _record: ABRecordRef,
    _error: MutVoidPtr,
) -> bool {
    log!("ABAddressBookRemoveRecord stubbed, returning false");
    false
}

// MARK: - ABRecord

fn ABRecordGetRecordID(_env: &mut Environment, _record: ABRecordRef) -> i32 {
    -1
}

fn ABRecordGetRecordType(_env: &mut Environment, _record: ABRecordRef) -> ABRecordType {
    0
}

fn ABRecordCopyValue(
    _env: &mut Environment,
    _record: ABRecordRef,
    _property: ABPropertyID,
) -> MutVoidPtr {
    Ptr::null()
}

fn ABRecordCopyCompositeName(_env: &mut Environment, _record: ABRecordRef) -> MutVoidPtr {
    Ptr::null()
}

fn ABRecordSetValue(
    _env: &mut Environment,
    _record: ABRecordRef,
    _property: ABPropertyID,
    _value: MutVoidPtr,
    _error: MutVoidPtr,
) -> bool {
    log!("ABRecordSetValue stubbed, returning false");
    false
}

fn ABRecordRemoveValue(
    _env: &mut Environment,
    _record: ABRecordRef,
    _property: ABPropertyID,
    _error: MutVoidPtr,
) -> bool {
    false
}

// MARK: - ABPerson

fn ABPersonCreate(_env: &mut Environment) -> ABRecordRef {
    log!("ABPersonCreate stubbed, returning NULL");
    Ptr::null()
}

fn ABPersonCreateInSource(
    _env: &mut Environment,
    _source: ABRecordRef,
) -> ABRecordRef {
    log!("ABPersonCreateInSource stubbed, returning NULL");
    Ptr::null()
}

fn ABPersonGetTypeOfProperty(
    _env: &mut Environment,
    _property: ABPropertyID,
) -> i32 {
    0
}

fn ABPersonCopyLocalizedPropertyName(
    _env: &mut Environment,
    _property: ABPropertyID,
) -> MutVoidPtr {
    Ptr::null()
}

fn ABPersonGetSortOrdering(_env: &mut Environment) -> i32 {
    0 // kABPersonSortByFirstName
}

fn ABPersonGetCompositeNameFormat(_env: &mut Environment) -> i32 {
    0 // kABPersonCompositeNameFormatFirstNameFirst
}

fn ABPersonCopyImageData(_env: &mut Environment, _person: ABRecordRef) -> MutVoidPtr {
    Ptr::null()
}

fn ABPersonHasImageData(_env: &mut Environment, _person: ABRecordRef) -> bool {
    false
}

fn ABPersonSetImageData(
    _env: &mut Environment,
    _person: ABRecordRef,
    _image_data: MutVoidPtr,
    _error: MutVoidPtr,
) -> bool {
    false
}

fn ABPersonRemoveImageData(
    _env: &mut Environment,
    _person: ABRecordRef,
    _error: MutVoidPtr,
) -> bool {
    false
}

// MARK: - ABMultiValue

fn ABMultiValueGetCount(_env: &mut Environment, _multi_value: ABMultiValueRef) -> CFIndex {
    0
}

fn ABMultiValueCopyValueAtIndex(
    _env: &mut Environment,
    _multi_value: ABMultiValueRef,
    _index: CFIndex,
) -> MutVoidPtr {
    Ptr::null()
}

fn ABMultiValueCopyLabelAtIndex(
    _env: &mut Environment,
    _multi_value: ABMultiValueRef,
    _index: CFIndex,
) -> MutVoidPtr {
    Ptr::null()
}

fn ABMultiValueGetIdentifierAtIndex(
    _env: &mut Environment,
    _multi_value: ABMultiValueRef,
    _index: CFIndex,
) -> i32 {
    -1
}

fn ABMultiValueGetIndexForIdentifier(
    _env: &mut Environment,
    _multi_value: ABMultiValueRef,
    _identifier: i32,
) -> CFIndex {
    -1
}

fn ABMultiValueGetFirstIndexOfValue(
    _env: &mut Environment,
    _multi_value: ABMultiValueRef,
    _value: MutVoidPtr,
) -> CFIndex {
    -1
}

fn ABMultiValueCreateMutable(_env: &mut Environment, _type_: i32) -> ABMultiValueRef {
    log!("ABMultiValueCreateMutable stubbed, returning NULL");
    Ptr::null()
}

fn ABMultiValueAddValueAndLabel(
    _env: &mut Environment,
    _multi_value: ABMultiValueRef,
    _value: MutVoidPtr,
    _label: MutVoidPtr,
    _out_identifier: MutVoidPtr,
) -> bool {
    false
}

fn ABMultiValueRemoveValueAndLabelAtIndex(
    _env: &mut Environment,
    _multi_value: ABMultiValueRef,
    _index: CFIndex,
) -> bool {
    false
}

pub const FUNCTIONS: FunctionExports = &[
    // Address book
    export_c_func!(ABAddressBookCreate()),
    export_c_func!(ABAddressBookCreateWithOptions(_, _)),
    export_c_func!(ABAddressBookSave(_, _)),
    export_c_func!(ABAddressBookHasUnsavedChanges(_)),
    export_c_func!(ABAddressBookRevert(_)),
    export_c_func!(ABAddressBookRequestAccessWithCompletion(_, _)),
    export_c_func!(ABAddressBookGetAuthorizationStatus()),
    // People
    export_c_func!(ABAddressBookGetPersonCount(_)),
    export_c_func!(ABAddressBookGetPersonWithRecordID(_, _)),
    export_c_func!(ABAddressBookCopyArrayOfAllPeople(_)),
    export_c_func!(ABAddressBookCopyArrayOfAllPeopleInSource(_, _)),
    export_c_func!(ABAddressBookCopyPeopleWithName(_, _)),
    export_c_func!(ABAddressBookAddRecord(_, _, _)),
    export_c_func!(ABAddressBookRemoveRecord(_, _, _)),
    // ABRecord
    export_c_func!(ABRecordGetRecordID(_)),
    export_c_func!(ABRecordGetRecordType(_)),
    export_c_func!(ABRecordCopyValue(_, _)),
    export_c_func!(ABRecordCopyCompositeName(_)),
    export_c_func!(ABRecordSetValue(_, _, _, _)),
    export_c_func!(ABRecordRemoveValue(_, _, _)),
    // ABPerson
    export_c_func!(ABPersonCreate()),
    export_c_func!(ABPersonCreateInSource(_)),
    export_c_func!(ABPersonGetTypeOfProperty(_)),
    export_c_func!(ABPersonCopyLocalizedPropertyName(_)),
    export_c_func!(ABPersonGetSortOrdering()),
    export_c_func!(ABPersonGetCompositeNameFormat()),
    export_c_func!(ABPersonCopyImageData(_)),
    export_c_func!(ABPersonHasImageData(_)),
    export_c_func!(ABPersonSetImageData(_, _, _)),
    export_c_func!(ABPersonRemoveImageData(_, _)),
    // ABMultiValue
    export_c_func!(ABMultiValueGetCount(_)),
    export_c_func!(ABMultiValueCopyValueAtIndex(_, _)),
    export_c_func!(ABMultiValueCopyLabelAtIndex(_, _)),
    export_c_func!(ABMultiValueGetIdentifierAtIndex(_, _)),
    export_c_func!(ABMultiValueGetIndexForIdentifier(_, _)),
    export_c_func!(ABMultiValueGetFirstIndexOfValue(_, _)),
    export_c_func!(ABMultiValueCreateMutable(_)),
    export_c_func!(ABMultiValueAddValueAndLabel(_, _, _, _)),
    export_c_func!(ABMultiValueRemoveValueAndLabelAtIndex(_, _)),
];

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/AddressBook.framework/AddressBook",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};

