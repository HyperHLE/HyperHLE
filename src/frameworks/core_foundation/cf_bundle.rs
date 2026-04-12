/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFBundle`.
//!
//! This is not even toll-free bridged to `NSBundle` in Apple's implementation,
//! but here it is the same type.

use super::cf_array::CFArrayRef;
use super::cf_string::CFStringRef;
use super::cf_url::CFURLRef;
use super::CFTypeRef;
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::foundation::ns_bundle::NSBundleHostObject;
use crate::frameworks::foundation::{ns_array, ns_string, NSUInteger};
use crate::mem::MutVoidPtr;
use crate::objc::{autorelease, id, msg, msg_class, nil, retain};
use crate::Environment;

const kCFBundleVersionKey:               &str = "CFBundleVersion";
const kCFBundleNameKey:                  &str = "CFBundleName";
const kCFBundleIdentifierKey:            &str = "CFBundleIdentifier";
const kCFBundleExecutableKey:            &str = "CFBundleExecutable";
const kCFBundleInfoDictionaryVersionKey: &str = "CFBundleInfoDictionaryVersion";
const kCFBundleLocalizationsKey:         &str = "CFBundleLocalizations";
const kCFBundleDevelopmentRegionKey:     &str = "CFBundleDevelopmentRegion";
const kCFBundleDisplayNameKey:           &str = "CFBundleDisplayName";
const kCFBundleSpokenNameKey:            &str = "CFBundleSpokenName";
const kCFBundleGetInfoStringKey:         &str = "CFBundleGetInfoString";
const kCFBundleHelpBookNameKey:          &str = "CFBundleHelpBookName";
const kCFBundleURLTypesKey:              &str = "CFBundleURLTypes";
const kCFBundleDocumentTypesKey:         &str = "CFBundleDocumentTypes";

pub const CONSTANTS: ConstantExports = &[
    ("_kCFBundleVersionKey",               HostConstant::NSString(kCFBundleVersionKey)),
    ("_kCFBundleNameKey",                  HostConstant::NSString(kCFBundleNameKey)),
    ("_kCFBundleIdentifierKey",            HostConstant::NSString(kCFBundleIdentifierKey)),
    ("_kCFBundleExecutableKey",            HostConstant::NSString(kCFBundleExecutableKey)),
    ("_kCFBundleInfoDictionaryVersionKey", HostConstant::NSString(kCFBundleInfoDictionaryVersionKey)),
    ("_kCFBundleLocalizationsKey",         HostConstant::NSString(kCFBundleLocalizationsKey)),
    ("_kCFBundleDevelopmentRegionKey",     HostConstant::NSString(kCFBundleDevelopmentRegionKey)),
    ("_kCFBundleDisplayNameKey",           HostConstant::NSString(kCFBundleDisplayNameKey)),
    ("_kCFBundleSpokenNameKey",            HostConstant::NSString(kCFBundleSpokenNameKey)),
    ("_kCFBundleGetInfoStringKey",         HostConstant::NSString(kCFBundleGetInfoStringKey)),
    ("_kCFBundleHelpBookNameKey",          HostConstant::NSString(kCFBundleHelpBookNameKey)),
    ("_kCFBundleURLTypesKey",              HostConstant::NSString(kCFBundleURLTypesKey)),
    ("_kCFBundleDocumentTypesKey",         HostConstant::NSString(kCFBundleDocumentTypesKey)),
];

pub type CFBundleRef = CFTypeRef;

// =========================================================================
// MARK: - Bundle access
// =========================================================================

fn CFBundleGetMainBundle(env: &mut Environment) -> CFBundleRef {
    let main_bundle: id = msg_class![env; NSBundle mainBundle];
    if main_bundle == nil {
        log!("Error: NSBundle mainBundle returned nil!");
    }
    main_bundle
}

fn CFBundleGetBundleWithIdentifier(
    env: &mut Environment,
    bundle_id: CFStringRef,
) -> CFBundleRef {
    if bundle_id == nil {
        return nil;
    }
    let main: CFBundleRef = CFBundleGetMainBundle(env);
    let main_id: id = CFBundleGetIdentifier(env, main);
    if main_id == nil {
        return nil;
    }
    let equal: bool = msg![env; main_id isEqualToString:bundle_id];
    if equal { main } else { nil }
}

fn CFBundleGetAllBundles(env: &mut Environment) -> CFArrayRef {
    let main = CFBundleGetMainBundle(env);
    let arr: id = msg_class![env; NSMutableArray new];
    let _: () = msg![env; arr addObject:main];
    autorelease(env, arr)
}

fn CFBundleCreate(
    env: &mut Environment,
    _allocator: CFTypeRef,
    bundle_url: CFURLRef,
) -> CFBundleRef {
    if bundle_url == nil {
        log_dbg!("CFBundleCreate: bundle_url is nil");
        return nil;
    }
    let bundle: id = msg_class![env; NSBundle bundleWithURL:bundle_url];
    if bundle == nil {
        log_dbg!("CFBundleCreate: failed to create NSBundle for URL {:?}", bundle_url);
        return nil;
    }
    // "Create" rule — caller owns a reference.
    retain(env, bundle)
}

fn CFBundleCreateBundlesFromDirectory(
    _env: &mut Environment,
    _allocator: CFTypeRef,
    _directory_url: CFURLRef,
    _bundle_type: CFStringRef,
) -> CFArrayRef {
    log!("TODO: CFBundleCreateBundlesFromDirectory — returning nil");
    nil
}

// =========================================================================
// MARK: - Info dictionary
// =========================================================================

fn CFBundleGetValueForInfoDictionaryKey(
    env: &mut Environment,
    bundle: CFBundleRef,
    key: CFStringRef,
) -> CFTypeRef {
    if bundle == nil || key == nil {
        return nil;
    }
    let dict: id = msg![env; bundle infoDictionary];
    if dict == nil {
        return nil;
    }
    msg![env; dict objectForKey:key]
}

fn CFBundleGetInfoDictionary(env: &mut Environment, bundle: CFBundleRef) -> CFTypeRef {
    if bundle == nil { return nil; }
    msg![env; bundle infoDictionary]
}

fn CFBundleGetLocalInfoDictionary(env: &mut Environment, bundle: CFBundleRef) -> CFTypeRef {
    // Return localizedInfoDictionary if available, fall back to plain dict.
    if bundle == nil { return nil; }
    let loc: id = msg![env; bundle localizedInfoDictionary];
    if loc != nil { return loc; }
    msg![env; bundle infoDictionary]
}

fn CFBundleCopyInfoDictionaryForURL(
    env: &mut Environment,
    url: CFURLRef,
) -> CFTypeRef {
    if url == nil { return nil; }
    let bundle: id = msg_class![env; NSBundle bundleWithURL:url];
    if bundle == nil { return nil; }
    let dict: id = msg![env; bundle infoDictionary];
    if dict == nil { return nil; }
    msg![env; dict copy]
}

fn CFBundleCopyInfoDictionaryInDirectory(
    env: &mut Environment,
    bundle_url: CFURLRef,
) -> CFTypeRef {
    CFBundleCopyInfoDictionaryForURL(env, bundle_url)
}

// =========================================================================
// MARK: - Common info-key helpers
// =========================================================================

fn CFBundleGetIdentifier(env: &mut Environment, bundle: CFBundleRef) -> CFStringRef {
    if bundle == nil { return nil; }
    let key: id = ns_string::get_static_str(env, kCFBundleIdentifierKey);
    CFBundleGetValueForInfoDictionaryKey(env, bundle, key)
}

fn CFBundleCopyBundleIdentifier(env: &mut Environment, bundle: CFBundleRef) -> CFStringRef {
    let s = CFBundleGetIdentifier(env, bundle);
    if s == nil { return nil; }
    msg![env; s copy]
}

fn CFBundleGetVersionNumber(env: &mut Environment, bundle: CFBundleRef) -> u32 {
    let version_key: id = ns_string::get_static_str(env, kCFBundleVersionKey);
    let vers: id = CFBundleGetValueForInfoDictionaryKey(env, bundle, version_key);
    if vers == nil { return 0; }
    let vers_str = ns_string::to_rust_string(env, vers);
    log_dbg!("CFBundleGetVersionNumber {}", vers_str);
    let parts: Vec<&str> = vers_str.split('.').collect();
    if parts.is_empty() || parts.len() > 3 { return 0; }
    let mut result: u32 = 1 << 15;
    let major: u32 = parts[0].parse().unwrap_or(0);
    if major > 99 { return 0; }
    result |= (major / 10) << 28;
    result |= (major % 10) << 24;
    if parts.len() >= 2 {
        let minor: u32 = parts[1].parse().unwrap_or(0);
        if minor <= 9 { result |= minor << 20; }
    }
    if parts.len() == 3 {
        let bug_fix: u32 = parts[2].parse().unwrap_or(0);
        if bug_fix <= 9 { result |= bug_fix << 16; }
    }
    result
}

fn CFBundleCopyShortVersionString(env: &mut Environment, bundle: CFBundleRef) -> CFStringRef {
    let key: id = ns_string::get_static_str(env, "CFBundleShortVersionString");
    let val: id = CFBundleGetValueForInfoDictionaryKey(env, bundle, key);
    if val == nil { return nil; }
    msg![env; val copy]
}

fn CFBundleCopyBundleDevelopmentRegion(
    env: &mut Environment,
    bundle: CFBundleRef,
) -> CFStringRef {
    let key: id = ns_string::get_static_str(env, kCFBundleDevelopmentRegionKey);
    let val: id = CFBundleGetValueForInfoDictionaryKey(env, bundle, key);
    if val != nil {
        return msg![env; val copy];
    }
    let en = ns_string::get_static_str(env, "en");
    msg![env; en copy]
}

/// `CFStringRef CFBundleCopyDisplayName(CFBundleRef bundle)`
///
/// Returns CFBundleDisplayName if present, falls back to CFBundleName, then
/// CFBundleExecutable, then the empty string.
fn CFBundleCopyDisplayName(env: &mut Environment, bundle: CFBundleRef) -> CFStringRef {
    for key in &[kCFBundleDisplayNameKey, kCFBundleNameKey, kCFBundleExecutableKey] {
        let k: id = ns_string::get_static_str(env, key);
        let val: id = CFBundleGetValueForInfoDictionaryKey(env, bundle, k);
        if val != nil {
            return msg![env; val copy];
        }
    }
    let empty = ns_string::get_static_str(env, "");
    msg![env; empty copy]
}

/// `CFStringRef CFBundleCopyHelpBookName(CFBundleRef bundle)`
fn CFBundleCopyHelpBookName(env: &mut Environment, bundle: CFBundleRef) -> CFStringRef {
    let key: id = ns_string::get_static_str(env, kCFBundleHelpBookNameKey);
    let val: id = CFBundleGetValueForInfoDictionaryKey(env, bundle, key);
    if val == nil { return nil; }
    msg![env; val copy]
}

// =========================================================================
// MARK: - URLs
// =========================================================================

fn CFBundleCopyBundleURL(env: &mut Environment, bundle: CFBundleRef) -> CFURLRef {
    if bundle == nil { return nil; }
    let url: CFURLRef = msg![env; bundle bundleURL];
    if url == nil { return nil; }
    msg![env; url copy]
}

fn CFBundleCopyResourcesDirectoryURL(env: &mut Environment, bundle: CFBundleRef) -> CFURLRef {
    if bundle == nil { return nil; }
    let url: CFURLRef = msg![env; bundle resourceURL];
    if url == nil { return nil; }
    msg![env; url copy]
}

fn CFBundleCopyExecutableURL(env: &mut Environment, bundle: CFBundleRef) -> CFURLRef {
    if bundle == nil { return nil; }
    let url: CFURLRef = msg![env; bundle executableURL];
    if url == nil { return nil; }
    msg![env; url copy]
}

fn CFBundleCopyAuxiliaryExecutableURL(
    env: &mut Environment,
    bundle: CFBundleRef,
    executable_name: CFStringRef,
) -> CFURLRef {
    log_dbg!(
        "CFBundleCopyAuxiliaryExecutableURL({:?}, {:?}) — returning nil",
        bundle, executable_name
    );
    nil
}

fn CFBundleCopyPrivateFrameworksURL(env: &mut Environment, bundle: CFBundleRef) -> CFURLRef {
    if bundle == nil { return nil; }
    let url: CFURLRef = msg![env; bundle privateFrameworksURL];
    if url == nil { return nil; }
    msg![env; url copy]
}

fn CFBundleCopySharedFrameworksURL(env: &mut Environment, bundle: CFBundleRef) -> CFURLRef {
    if bundle == nil { return nil; }
    let url: CFURLRef = msg![env; bundle sharedFrameworksURL];
    if url == nil { return nil; }
    msg![env; url copy]
}

fn CFBundleCopySharedSupportURL(env: &mut Environment, bundle: CFBundleRef) -> CFURLRef {
    if bundle == nil { return nil; }
    let url: CFURLRef = msg![env; bundle sharedSupportURL];
    if url == nil { return nil; }
    msg![env; url copy]
}

fn CFBundleCopyBuiltInPlugInsURL(env: &mut Environment, bundle: CFBundleRef) -> CFURLRef {
    if bundle == nil { return nil; }
    let url: CFURLRef = msg![env; bundle builtInPlugInsURL];
    if url == nil { return nil; }
    msg![env; url copy]
}

fn CFBundleCopyResourceURL(
    env: &mut Environment,
    bundle: CFBundleRef,
    resource_name: CFStringRef,
    resource_type: CFStringRef,
    sub_dir_name: CFStringRef,
) -> CFURLRef {
    if bundle == nil || resource_name == nil { return nil; }
    let url: CFURLRef = msg![env; bundle URLForResource:resource_name
                                          withExtension:resource_type
                                           subdirectory:sub_dir_name];
    if url == nil { return nil; }
    msg![env; url copy]
}

fn CFBundleCopyResourceURLInDirectory(
    env: &mut Environment,
    bundle_url: CFURLRef,
    resource_name: CFStringRef,
    resource_type: CFStringRef,
    sub_dir_name: CFStringRef,
) -> CFURLRef {
    if bundle_url == nil { return nil; }
    let bundle: id = msg_class![env; NSBundle bundleWithURL:bundle_url];
    if bundle == nil { return nil; }
    CFBundleCopyResourceURL(env, bundle, resource_name, resource_type, sub_dir_name)
}

fn CFBundleCopyResourceURLsOfType(
    env: &mut Environment,
    bundle: CFBundleRef,
    resource_type: CFStringRef,
    sub_dir_name: CFStringRef,
) -> CFArrayRef {
    if bundle == nil { return nil; }
    msg![env; bundle URLsForResourcesWithExtension:resource_type subdirectory:sub_dir_name]
}

fn CFBundleCopyResourceURLForLocalization(
    env: &mut Environment,
    bundle: CFBundleRef,
    resource_name: CFStringRef,
    resource_type: CFStringRef,
    sub_dir_name: CFStringRef,
    localization_name: CFStringRef,
) -> CFURLRef {
    if bundle == nil || resource_name == nil { return nil; }
    let url: CFURLRef = msg![env; bundle URLForResource:resource_name
                                          withExtension:resource_type
                                           subdirectory:sub_dir_name
                                           localization:localization_name];
    if url == nil { return nil; }
    msg![env; url copy]
}

fn CFBundleCopyResourceURLsOfTypeForLocalization(
    env: &mut Environment,
    bundle: CFBundleRef,
    resource_type: CFStringRef,
    sub_dir_name: CFStringRef,
    localization_name: CFStringRef,
) -> CFArrayRef {
    if bundle == nil { return nil; }
    msg![env; bundle URLsForResourcesWithExtension:resource_type
                                      subdirectory:sub_dir_name
                                      localization:localization_name]
}

// =========================================================================
// MARK: - Localizations
// =========================================================================

pub fn CFBundleCopyBundleLocalizations(env: &mut Environment, bundle: CFBundleRef) -> CFArrayRef {
    let bundle_localizations = env
        .objc
        .borrow_mut::<NSBundleHostObject>(bundle)
        .bundle
        .as_ref()
        .unwrap_or(&env.bundle)
        .bundle_localizations()
        .iter()
        .map(|value| value.as_string().unwrap().to_string())
        .collect::<Vec<String>>();

    let guest_bundle_localizations = bundle_localizations
        .iter()
        .map(|loc| ns_string::from_rust_string(env, loc.to_owned()))
        .collect::<Vec<id>>();

    let loc_array = ns_array::from_vec(env, guest_bundle_localizations);
    log_dbg!(
        "CFBundleCopyBundleLocalizations({:?}) => {:?} ({})",
        bundle, loc_array, bundle_localizations.join(", ")
    );
    loc_array
}

pub fn CFBundleCopyPreferredLocalizationsFromArray(
    env: &mut Environment,
    loc_array: CFArrayRef,
) -> CFArrayRef {
    let mut result = Vec::new();
    let preferred_languages: id = msg_class![env; NSLocale preferredLanguages];

    let loc_count: NSUInteger = msg![env; loc_array count];
    let pref_loc_count: NSUInteger = msg![env; preferred_languages count];

    for loc_index in 0..loc_count {
        let loc: id = msg![env; loc_array objectAtIndex:loc_index];
        for pref_loc_index in 0..pref_loc_count {
            let pref_loc: id = msg![env; preferred_languages objectAtIndex:pref_loc_index];
            let equal: bool = msg![env; loc isEqualToString:pref_loc];
            if equal {
                result.push(loc);
                retain(env, loc);
                break;
            }
        }
    }

    if loc_count > 0 {
        let first_loc: id = msg![env; loc_array objectAtIndex:(0 as NSUInteger)];
        result.push(first_loc);
        retain(env, first_loc);
    } else {
        let en_loc = ns_string::get_static_str(env, "en");
        result.push(en_loc);
    }

    let result = ns_array::from_vec(env, result);
    log_dbg!(
        "CFBundleCopyPreferredLocalizationsFromArray({:?}) => {:?}",
        loc_array, result
    );
    result
}

fn CFBundleCopyLocalizedString(
    env: &mut Environment,
    bundle: CFBundleRef,
    key: CFStringRef,
    value: CFStringRef,
    table_name: CFStringRef,
) -> CFStringRef {
    if bundle == nil || key == nil { return if value != nil { value } else { key }; }
    let res = msg![env; bundle localizedStringForKey:key value:value table:table_name];
    msg![env; res copy]
}

fn CFBundleCopyLocalizedStringForLocalization(
    env: &mut Environment,
    bundle: CFBundleRef,
    key: CFStringRef,
    value: CFStringRef,
    table_name: CFStringRef,
    _localization_name: CFStringRef,
) -> CFStringRef {
    log_dbg!("CFBundleCopyLocalizedStringForLocalization — ignoring localizationName");
    CFBundleCopyLocalizedString(env, bundle, key, value, table_name)
}

// =========================================================================
// MARK: - Load state
// =========================================================================

fn CFBundleIsExecutableLoaded(_env: &mut Environment, _bundle: CFBundleRef) -> bool {
    true
}

fn CFBundlePreflightExecutable(
    env: &mut Environment,
    bundle: CFBundleRef,
    _error: MutVoidPtr,
) -> bool {
    let main: CFBundleRef = CFBundleGetMainBundle(env);
    bundle == main
}

fn CFBundleLoadExecutable(_env: &mut Environment, bundle: CFBundleRef) -> bool {
    log!("TODO: CFBundleLoadExecutable({:?}) — returning false", bundle);
    false
}

fn CFBundleLoadExecutableAndReturnError(
    env: &mut Environment,
    bundle: CFBundleRef,
    _error: MutVoidPtr,
) -> bool {
    CFBundleLoadExecutable(env, bundle)
}

fn CFBundleUnloadExecutable(_env: &mut Environment, bundle: CFBundleRef) {
    log!("TODO: CFBundleUnloadExecutable({:?}) — ignored", bundle);
}

// =========================================================================
// MARK: - Symbol / function lookup
// =========================================================================

fn CFBundleGetFunctionPointerForName(
    env: &mut Environment,
    bundle: CFBundleRef,
    function_name: CFStringRef,
) -> CFTypeRef {
    if function_name == nil { return nil; }
    let name = ns_string::to_rust_string(env, function_name);
    log!(
        "TODO: CFBundleGetFunctionPointerForName({:?}, \"{}\") — returning NULL",
        bundle, name
    );
    nil
}

fn CFBundleGetFunctionPointersForNames(
    env: &mut Environment,
    bundle: CFBundleRef,
    function_names: CFArrayRef,
    ftbl: MutVoidPtr,
) {
    if ftbl.is_null() { return; }
    let count: NSUInteger = msg![env; function_names count];
    log!(
        "CFBundleGetFunctionPointersForNames({:?}, count={}) — writing NULL for all",
        bundle, count
    );
    let null_ptr: crate::mem::MutPtr<MutVoidPtr> = ftbl.cast();
    for i in 0..count {
        env.mem.write(null_ptr + i, crate::mem::Ptr::null());
    }
}

fn CFBundleGetDataPointerForName(
    env: &mut Environment,
    bundle: CFBundleRef,
    symbol_name: CFStringRef,
) -> CFTypeRef {
    if symbol_name == nil { return nil; }
    let name = ns_string::to_rust_string(env, symbol_name);
    log!(
        "TODO: CFBundleGetDataPointerForName({:?}, \"{}\") — returning NULL",
        bundle, name
    );
    nil
}

fn CFBundleGetDataPointersForNames(
    env: &mut Environment,
    bundle: CFBundleRef,
    symbol_names: CFArrayRef,
    stbl: MutVoidPtr,
) {
    if stbl.is_null() { return; }
    let count: NSUInteger = msg![env; symbol_names count];
    log!(
        "CFBundleGetDataPointersForNames({:?}, count={}) — writing NULL for all",
        bundle, count
    );
    let null_ptr: crate::mem::MutPtr<MutVoidPtr> = stbl.cast();
    for i in 0..count {
        env.mem.write(null_ptr + i, crate::mem::Ptr::null());
    }
}

pub const FUNCTIONS: FunctionExports = &[
    // Bundle access
    export_c_func!(CFBundleGetMainBundle()),
    export_c_func!(CFBundleGetBundleWithIdentifier(_)),
    export_c_func!(CFBundleGetAllBundles()),
    export_c_func!(CFBundleCreate(_, _)),
    export_c_func!(CFBundleCreateBundlesFromDirectory(_, _, _)),
    // Info dictionary
    export_c_func!(CFBundleGetValueForInfoDictionaryKey(_, _)),
    export_c_func!(CFBundleGetInfoDictionary(_)),
    export_c_func!(CFBundleGetLocalInfoDictionary(_)),
    export_c_func!(CFBundleCopyInfoDictionaryForURL(_)),
    export_c_func!(CFBundleCopyInfoDictionaryInDirectory(_)),
    export_c_func!(CFBundleGetIdentifier(_)),
    export_c_func!(CFBundleCopyBundleIdentifier(_)),
    export_c_func!(CFBundleGetVersionNumber(_)),
    export_c_func!(CFBundleCopyShortVersionString(_)),
    export_c_func!(CFBundleCopyBundleDevelopmentRegion(_)),
    export_c_func!(CFBundleCopyDisplayName(_)),
    export_c_func!(CFBundleCopyHelpBookName(_)),
    // URLs
    export_c_func!(CFBundleCopyBundleURL(_)),
    export_c_func!(CFBundleCopyResourcesDirectoryURL(_)),
    export_c_func!(CFBundleCopyExecutableURL(_)),
    export_c_func!(CFBundleCopyAuxiliaryExecutableURL(_, _)),
    export_c_func!(CFBundleCopyPrivateFrameworksURL(_)),
    export_c_func!(CFBundleCopySharedFrameworksURL(_)),
    export_c_func!(CFBundleCopySharedSupportURL(_)),
    export_c_func!(CFBundleCopyBuiltInPlugInsURL(_)),
    export_c_func!(CFBundleCopyResourceURL(_, _, _, _)),
    export_c_func!(CFBundleCopyResourceURLInDirectory(_, _, _, _)),
    export_c_func!(CFBundleCopyResourceURLsOfType(_, _, _)),
    export_c_func!(CFBundleCopyResourceURLForLocalization(_, _, _, _, _)),
    export_c_func!(CFBundleCopyResourceURLsOfTypeForLocalization(_, _, _, _)),
    // Localizations
    export_c_func!(CFBundleCopyBundleLocalizations(_)),
    export_c_func!(CFBundleCopyPreferredLocalizationsFromArray(_)),
    export_c_func!(CFBundleCopyLocalizedString(_, _, _, _)),
    export_c_func!(CFBundleCopyLocalizedStringForLocalization(_, _, _, _, _)),
    // Load state
    export_c_func!(CFBundleIsExecutableLoaded(_)),
    export_c_func!(CFBundlePreflightExecutable(_, _)),
    export_c_func!(CFBundleLoadExecutable(_)),
    export_c_func!(CFBundleLoadExecutableAndReturnError(_, _)),
    export_c_func!(CFBundleUnloadExecutable(_)),
    // Symbol / data lookup
    export_c_func!(CFBundleGetFunctionPointerForName(_, _)),
    export_c_func!(CFBundleGetFunctionPointersForNames(_, _, _)),
    export_c_func!(CFBundleGetDataPointerForName(_, _)),
    export_c_func!(CFBundleGetDataPointersForNames(_, _, _)),
];
