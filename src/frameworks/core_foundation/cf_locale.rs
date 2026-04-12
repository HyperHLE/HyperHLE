/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//!
//! `CFLocale`

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::cf_array::CFArrayRef;
use super::cf_string::CFStringRef;
use super::{CFRelease, CFRetain, CFTypeRef};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::foundation::ns_string;
use crate::frameworks::foundation::NSUInteger;
use crate::objc::{autorelease, id, msg, msg_class, nil};
use crate::Environment;

type CFLocaleIdentifier = CFStringRef;
/// `NSLocale` is toll-free bridged with `CFLocaleRef`
pub(super) type CFLocaleRef = CFTypeRef;
type CFLocaleKey = CFStringRef;

// MARK: - Constants

pub const kCFLocaleIdentifier:        &str = "kCFLocaleIdentifierKey";
pub const kCFLocaleLanguageCode:      &str = "kCFLocaleLanguageCode";
pub const kCFLocaleCountryCode:       &str = "kCFLocaleCountryCodeKey";
pub const kCFLocaleScriptCode:        &str = "kCFLocaleScriptCode";
pub const kCFLocaleVariantCode:       &str = "kCFLocaleVariantCode";
pub const kCFLocaleCalendarIdentifier: &str = "kCFLocaleCalendarIdentifier";
pub const kCFLocaleCalendar:          &str = "kCFLocaleCalendar";
pub const kCFLocaleCollationIdentifier: &str = "kCFLocaleCollationIdentifier";
pub const kCFLocaleUsesMetricSystem:  &str = "kCFLocaleUsesMetricSystem";
pub const kCFLocaleMeasurementSystem: &str = "kCFLocaleMeasurementSystem";
pub const kCFLocaleDecimalSeparator:  &str = "kCFLocaleDecimalSeparator";
pub const kCFLocaleGroupingSeparator: &str = "kCFLocaleGroupingSeparator";
pub const kCFLocaleCurrencySymbol:    &str = "kCFLocaleCurrencySymbol";
pub const kCFLocaleCurrencyCode:      &str = "kCFLocaleCurrencyCode";
pub const kCFLocaleCollatorIdentifier: &str = "kCFLocaleCollatorIdentifier";

// Calendar identifier constants
pub const kCFGregorianCalendar:       &str = "gregorian";
pub const kCFBuddhistCalendar:        &str = "buddhist";
pub const kCFChineseCalendar:         &str = "chinese";
pub const kCFHebrewCalendar:          &str = "hebrew";
pub const kCFIslamicCalendar:         &str = "islamic";
pub const kCFIslamicCivilCalendar:    &str = "islamic-civil";
pub const kCFJapaneseCalendar:        &str = "japanese";
pub const kCFRepublicOfChinaCalendar: &str = "roc";
pub const kCFPersianCalendar:         &str = "persian";
pub const kCFIndianCalendar:          &str = "indian";
pub const kCFISO8601Calendar:         &str = "iso8601";

pub const CONSTANTS: ConstantExports = &[
    ("_kCFLocaleIdentifier",          HostConstant::NSString(kCFLocaleIdentifier)),
    ("_kCFLocaleLanguageCode",        HostConstant::NSString(kCFLocaleLanguageCode)),
    ("_kCFLocaleCountryCode",         HostConstant::NSString(kCFLocaleCountryCode)),
    ("_kCFLocaleScriptCode",          HostConstant::NSString(kCFLocaleScriptCode)),
    ("_kCFLocaleVariantCode",         HostConstant::NSString(kCFLocaleVariantCode)),
    ("_kCFLocaleCalendarIdentifier",  HostConstant::NSString(kCFLocaleCalendarIdentifier)),
    ("_kCFLocaleCalendar",            HostConstant::NSString(kCFLocaleCalendar)),
    ("_kCFLocaleCollationIdentifier", HostConstant::NSString(kCFLocaleCollationIdentifier)),
    ("_kCFLocaleUsesMetricSystem",    HostConstant::NSString(kCFLocaleUsesMetricSystem)),
    ("_kCFLocaleMeasurementSystem",   HostConstant::NSString(kCFLocaleMeasurementSystem)),
    ("_kCFLocaleDecimalSeparator",    HostConstant::NSString(kCFLocaleDecimalSeparator)),
    ("_kCFLocaleGroupingSeparator",   HostConstant::NSString(kCFLocaleGroupingSeparator)),
    ("_kCFLocaleCurrencySymbol",      HostConstant::NSString(kCFLocaleCurrencySymbol)),
    ("_kCFLocaleCurrencyCode",        HostConstant::NSString(kCFLocaleCurrencyCode)),
    ("_kCFLocaleCollatorIdentifier",  HostConstant::NSString(kCFLocaleCollatorIdentifier)),
    // Calendar identifiers
    ("_kCFGregorianCalendar",         HostConstant::NSString(kCFGregorianCalendar)),
    ("_kCFBuddhistCalendar",          HostConstant::NSString(kCFBuddhistCalendar)),
    ("_kCFChineseCalendar",           HostConstant::NSString(kCFChineseCalendar)),
    ("_kCFHebrewCalendar",            HostConstant::NSString(kCFHebrewCalendar)),
    ("_kCFIslamicCalendar",           HostConstant::NSString(kCFIslamicCalendar)),
    ("_kCFIslamicCivilCalendar",      HostConstant::NSString(kCFIslamicCivilCalendar)),
    ("_kCFJapaneseCalendar",          HostConstant::NSString(kCFJapaneseCalendar)),
    ("_kCFRepublicOfChinaCalendar",   HostConstant::NSString(kCFRepublicOfChinaCalendar)),
    ("_kCFPersianCalendar",           HostConstant::NSString(kCFPersianCalendar)),
    ("_kCFIndianCalendar",            HostConstant::NSString(kCFIndianCalendar)),
    ("_kCFISO8601Calendar",           HostConstant::NSString(kCFISO8601Calendar)),
];

// MARK: - Retain / Release

pub fn CFLocaleRetain(env: &mut Environment, locale: CFLocaleRef) -> CFLocaleRef {
    if !locale.is_null() { CFRetain(env, locale) } else { locale }
}

pub fn CFLocaleRelease(env: &mut Environment, locale: CFLocaleRef) {
    if !locale.is_null() { CFRelease(env, locale); }
}

// MARK: - Constructors

fn CFLocaleCopyCurrent(env: &mut Environment) -> CFLocaleRef {
    let locale: id = msg_class![env; NSLocale currentLocale];
    msg![env; locale copy]
}

fn CFLocaleCreateCopy(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    locale: CFLocaleRef,
) -> CFLocaleRef {
    if locale.is_null() { return nil; }
    msg![env; locale copy]
}

fn CFLocaleCopyPreferredLanguages(env: &mut Environment) -> CFArrayRef {
    let arr: id = msg_class![env; NSLocale preferredLanguages];
    msg![env; arr copy]
}

fn CFLocaleCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    locale_identifier: CFLocaleIdentifier,
) -> CFLocaleRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    let new: id = msg_class![env; NSLocale alloc];
    msg![env; new initWithLocaleIdentifier:locale_identifier]
}

fn CFLocaleGetSystem(env: &mut Environment) -> CFLocaleRef {
    msg_class![env; NSLocale systemLocale]
}

// MARK: - Identifier helpers

fn CFLocaleCreateCanonicalLocaleIdentifierFromString(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    locale_identifier: CFStringRef,
) -> CFLocaleIdentifier {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    if locale_identifier.is_null() {
        return nil;
    }
    // Return a copy — canonicalisation is a locale-library concern we skip.
    let ns: id = msg_class![env; NSString alloc];
    msg![env; ns initWithString:locale_identifier]
}

fn CFLocaleCreateCanonicalLanguageIdentifierFromString(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    locale_identifier: CFStringRef,
) -> CFLocaleIdentifier {
    if locale_identifier.is_null() {
        return nil;
    }
    // Strip country suffix if present: "en_US" → "en".
    let s = ns_string::to_rust_string(env, locale_identifier).into_owned();
    let lang = s.split('_').next().unwrap_or(&s).split('-').next().unwrap_or(&s);
    let ns = ns_string::from_rust_string(env, lang.to_string());
    autorelease(env, ns)
}

fn CFLocaleCreateLocaleIdentifierFromComponents(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    dictionary: CFTypeRef, // CFDictionaryRef
) -> CFLocaleIdentifier {
    // Delegate to NSLocale.
    let ns = msg_class![env; NSLocale localeIdentifierFromComponents:dictionary];
    msg![env; ns copy]
}

fn CFLocaleCreateComponentsFromLocaleIdentifier(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    locale_identifier: CFLocaleIdentifier,
) -> CFTypeRef { // CFDictionaryRef
    msg_class![env; NSLocale componentsFromLocaleIdentifier:locale_identifier]
}

fn CFLocaleCreateLocaleIdentifierFromWindowsLocaleCode(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    _lcid: u32,
) -> CFLocaleIdentifier {
    // Stub — return "en_US".
    let ns = ns_string::from_rust_string(env, "en_US".to_string());
    autorelease(env, ns)
}

fn CFLocaleGetWindowsLocaleCodeFromLocaleIdentifier(
    env: &mut Environment,
    _locale_identifier: CFLocaleIdentifier,
) -> u32 {
    // 0x0409 = en-US LCID.
    0x0409
}

// MARK: - Value accessors

fn CFLocaleGetValue(
    env: &mut Environment,
    locale: CFLocaleRef,
    key: CFLocaleKey,
) -> CFTypeRef {
    msg![env; locale objectForKey:key]
}

fn CFLocaleCopyDisplayNameForPropertyValue(
    env: &mut Environment,
    display_locale: CFLocaleRef,
    key: CFLocaleKey,
    value: CFStringRef,
) -> CFStringRef {
    // Delegate to NSLocale displayNameForKey:value:
    let result: id = msg![env; display_locale displayNameForKey:key value:value];
    if result.is_null() { return nil; }
    msg![env; result copy]
}

fn CFLocaleGetIdentifier(
    env: &mut Environment,
    locale: CFLocaleRef,
) -> CFLocaleIdentifier {
    msg![env; locale localeIdentifier]
}

// MARK: - Available locales

fn CFLocaleCopyAvailableLocaleIdentifiers(env: &mut Environment) -> CFArrayRef {
    let arr: id = msg_class![env; NSLocale availableLocaleIdentifiers];
    msg![env; arr copy]
}

fn CFLocaleCopyISOLanguageCodes(env: &mut Environment) -> CFArrayRef {
    let arr: id = msg_class![env; NSLocale ISOLanguageCodes];
    msg![env; arr copy]
}

fn CFLocaleCopyISOCountryCodes(env: &mut Environment) -> CFArrayRef {
    let arr: id = msg_class![env; NSLocale ISOCountryCodes];
    msg![env; arr copy]
}

fn CFLocaleCopyISOCurrencyCodes(env: &mut Environment) -> CFArrayRef {
    let arr: id = msg_class![env; NSLocale ISOCurrencyCodes];
    msg![env; arr copy]
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFLocaleRetain(_)),
    export_c_func!(CFLocaleRelease(_)),
    export_c_func!(CFLocaleCopyCurrent()),
    export_c_func!(CFLocaleCreateCopy(_, _)),
    export_c_func!(CFLocaleCopyPreferredLanguages()),
    export_c_func!(CFLocaleCreate(_, _)),
    export_c_func!(CFLocaleGetSystem()),
    export_c_func!(CFLocaleCreateCanonicalLocaleIdentifierFromString(_, _)),
    export_c_func!(CFLocaleCreateCanonicalLanguageIdentifierFromString(_, _)),
    export_c_func!(CFLocaleCreateLocaleIdentifierFromComponents(_, _)),
    export_c_func!(CFLocaleCreateComponentsFromLocaleIdentifier(_, _)),
    export_c_func!(CFLocaleCreateLocaleIdentifierFromWindowsLocaleCode(_, _)),
    export_c_func!(CFLocaleGetWindowsLocaleCodeFromLocaleIdentifier(_)),
    export_c_func!(CFLocaleGetValue(_, _)),
    export_c_func!(CFLocaleCopyDisplayNameForPropertyValue(_, _, _)),
    export_c_func!(CFLocaleGetIdentifier(_)),
    export_c_func!(CFLocaleCopyAvailableLocaleIdentifiers()),
    export_c_func!(CFLocaleCopyISOLanguageCodes()),
    export_c_func!(CFLocaleCopyISOCountryCodes()),
    export_c_func!(CFLocaleCopyISOCurrencyCodes()),
];

