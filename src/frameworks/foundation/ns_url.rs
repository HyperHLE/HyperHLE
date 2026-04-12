/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSURL`.

use super::ns_string::{from_rust_string, get_static_str, to_rust_string, NSUTF8StringEncoding};
use super::NSUInteger;
use crate::fs::{GuestPath, GuestPathBuf};
use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;
use std::borrow::Cow;

enum NSURLHostObject {
    FileURL {
        ns_string: id,
        working_directory: GuestPathBuf,
    },
    OtherURL { ns_string: id },
}
impl HostObject for NSURLHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSURL: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = NSURLHostObject::FileURL {
        ns_string: nil,
        working_directory: env.fs.working_directory().into(),
    };
    env.objc.alloc_object(this, Box::new(host_object), &mut env.mem)
}

+ (id)URLWithString:(id)url { // NSString*
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithString:url];
    autorelease(env, new)
}

+ (id)URLWithString:(id)url // NSString*
      relativeToURL:(id)base_url { // NSURL*
    if url == nil {
        return nil;
    }
    let url_str = to_rust_string(env, url);
    // If the string is already absolute, ignore the base.
    if url_str.contains("://") || url_str.starts_with('/') {
        return msg_class![env; NSURL URLWithString:url];
    }
    // Resolve relative to base URL path.
    if base_url == nil {
        return msg_class![env; NSURL URLWithString:url];
    }
    let base_path: id = msg![env; base_url absoluteString];
    let base_str = to_rust_string(env, base_path).into_owned();
    let combined = if base_str.ends_with('/') {
        format!("{}{}", base_str, url_str)
    } else {
        format!("{}/{}", base_str, url_str)
    };
    let ns = from_rust_string(env, combined);
    let new: id = msg_class![env; NSURL alloc];
    let new: id = msg![env; new initWithString:ns];
    release(env, ns);
    autorelease(env, new)
}

+ (id)fileURLWithPath:(id)path { // NSString*
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initFileURLWithPath:path];
    autorelease(env, new)
}

+ (id)fileURLWithPath:(id)path // NSString*
          isDirectory:(bool)is_dir {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initFileURLWithPath:path isDirectory:is_dir];
    autorelease(env, new)
}

+ (id)fileURLWithPathComponents:(id)components { // NSArray*
    let count: NSUInteger = msg![env; components count];
    if count == 0 {
        return nil;
    }
    let first: id = msg![env; components objectAtIndex:0u32];
    let mut path_str = to_rust_string(env, first).into_owned();
    for i in 1..count {
        let comp: id = msg![env; components objectAtIndex:i];
        let comp_str = to_rust_string(env, comp);
        if !path_str.ends_with('/') {
            path_str.push('/');
        }
        path_str.push_str(&comp_str);
    }
    let ns_path = from_rust_string(env, path_str);
    let url = msg_class![env; NSURL fileURLWithPath:ns_path];
    release(env, ns_path);
    url
}

// MARK: - Dealloc / copy

- (())dealloc {
    match *env.objc.borrow(this) {
        NSURLHostObject::FileURL { ns_string, .. } => release(env, ns_string),
        NSURLHostObject::OtherURL { ns_string }    => release(env, ns_string),
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)copyWithZone:(NSZonePtr)_zone {
    retain(env, this)
}

// MARK: - Init

- (id)initFileURLWithPath:(id)path { // NSString*
    msg![env; this initFileURLWithPath:path isDirectory:false]
}

- (id)initFileURLWithPath:(id)path // NSString*
              isDirectory:(bool)_is_dir {
    let path_str = to_rust_string(env, path);
    let mut safe_path = path;
    
    // Если игра передала путь вместе с префиксом file:, очищаем его
    if path_str.starts_with("file:") {
        let stripped = path_str
            .replacen("file://localhost", "", 1)
            .replacen("file://", "", 1)
            .replacen("file:", "", 1);
            
        // Выделяем создание строки в отдельный шаг, чтобы порадовать borrow checker
        let new_ns_string = from_rust_string(env, stripped);
        safe_path = autorelease(env, new_ns_string);
    }

    let expanded_path: id = msg![env; safe_path stringByExpandingTildeInPath];
    let copied_path: id = msg![env; expanded_path copy];
    *env.objc.borrow_mut(this) = NSURLHostObject::FileURL {
        ns_string: copied_path,
        working_directory: env.fs.working_directory().into(),
    };
    this
}

- (id)initWithString:(id)url { // NSString*
    if url == nil {
        return nil;
    }
    
    let url_str = to_rust_string(env, url);
    // Если это локальный файл, перенаправляем инициализацию в правильный метод
    if url_str.starts_with("file:") {
        let stripped = url_str
            .replacen("file://localhost", "", 1)
            .replacen("file://", "", 1)
            .replacen("file:", "", 1);
            
        // То же самое: разбиваем на два шага
        let new_ns_string = from_rust_string(env, stripped);
        let safe_path = autorelease(env, new_ns_string);
        
        return msg![env; this initFileURLWithPath:safe_path isDirectory:false];
    }
    
    let url: id = msg![env; url copy];
    *env.objc.borrow_mut(this) = NSURLHostObject::OtherURL { ns_string: url };
    this
}
    
- (id)initWithString:(id)url // NSString*
       relativeToURL:(id)base_url { // NSURL*
    if url == nil {
        release(env, this);
        return nil;
    }
    // Reuse class-level logic.
    let resolved = msg_class![env; NSURL URLWithString:url relativeToURL:base_url];
    if resolved == nil {
        release(env, this);
        return nil;
    }
    // Copy the host object from the resolved URL into this one.
    let ns_string: id = msg![env; resolved absoluteString];
    let ns_string: id = msg![env; ns_string copy];
    *env.objc.borrow_mut(this) = NSURLHostObject::OtherURL { ns_string };
    this
}

// MARK: - Type checks

- (bool)isFileURL {
    matches!(env.objc.borrow(this), NSURLHostObject::FileURL { .. })
}

// MARK: - String representations

- (id)description {
    match env.objc.borrow(this) {
        NSURLHostObject::FileURL { ns_string, working_directory } => {
            let wd   = working_directory.as_str().to_string();
            let mut desc = to_rust_string(env, *ns_string).to_string();
            if !desc.starts_with('/') {
                desc = format!(
                    "{} -- file://localhost{}",
                    desc.trim_start_matches("./"),
                    wd
                );
            }
            let ns = from_rust_string(env, desc);
            autorelease(env, ns)
        }
        NSURLHostObject::OtherURL { ns_string } => *ns_string,
    }
}

- (id)absoluteString {
    match *env.objc.borrow(this) {
        NSURLHostObject::FileURL { ns_string, .. } => ns_string,
        NSURLHostObject::OtherURL { ns_string }    => ns_string,
    }
}

- (id)relativeString {
    // We don't track base URLs; return the full string.
    msg![env; this absoluteString]
}

// MARK: - Path components

- (id)scheme {
    let s: id = msg![env; this absoluteString];
    let str = to_rust_string(env, s);
    if let Some(pos) = str.find("://") {
        let scheme = from_rust_string(env, str[..pos].to_string());
        return autorelease(env, scheme);
    }
    nil
}

- (id)host {
    let s: id = msg![env; this absoluteString];
    let str = to_rust_string(env, s).into_owned();
    // Strip scheme://
    let after_scheme = if let Some(pos) = str.find("://") {
        &str[pos + 3..]
    } else {
        return nil;
    };
    // Strip userinfo@ if present
    let after_user = if let Some(at) = after_scheme.find('@') {
        &after_scheme[at + 1..]
    } else {
        after_scheme
    };
    // Strip path, query, fragment
    let host_and_port = after_user
        .split('/')
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .split('#')
        .next()
        .unwrap_or("");
    // Strip port
    let host = if let Some(colon) = host_and_port.rfind(':') {
        &host_and_port[..colon]
    } else {
        host_and_port
    };
    if host.is_empty() {
        return nil;
    }
    let ns = from_rust_string(env, host.to_string());
    autorelease(env, ns)
}

- (id)port { // NSNumber*
    let s: id = msg![env; this absoluteString];
    let str = to_rust_string(env, s).into_owned();
    let after_scheme = if let Some(pos) = str.find("://") {
        &str[pos + 3..]
    } else {
        return nil;
    };
    let after_user = if let Some(at) = after_scheme.find('@') {
        &after_scheme[at + 1..]
    } else {
        after_scheme
    };
    let host_and_port = after_user
        .split('/')
        .next()
        .unwrap_or("");
    if let Some(colon) = host_and_port.rfind(':') {
        let port_str = &host_and_port[colon + 1..];
        if let Ok(port_num) = port_str.parse::<i32>() {
            let ns_port = msg_class![env; NSNumber numberWithInt:port_num];
            return ns_port;
        }
    }
    nil
}

- (id)user {
    let s: id = msg![env; this absoluteString];
    let str = to_rust_string(env, s).into_owned();
    let after_scheme = if let Some(pos) = str.find("://") {
        &str[pos + 3..]
    } else {
        return nil;
    };
    if let Some(at) = after_scheme.find('@') {
        let userinfo = &after_scheme[..at];
        let user = userinfo.split(':').next().unwrap_or("");
        if !user.is_empty() {
            let ns = from_rust_string(env, user.to_string());
            return autorelease(env, ns);
        }
    }
    nil
}

- (id)password {
    let s: id = msg![env; this absoluteString];
    let str = to_rust_string(env, s).into_owned();
    let after_scheme = if let Some(pos) = str.find("://") {
        &str[pos + 3..]
    } else {
        return nil;
    };
    if let Some(at) = after_scheme.find('@') {
        let userinfo = &after_scheme[..at];
        let mut parts = userinfo.splitn(2, ':');
        let _ = parts.next(); // user
        if let Some(pass) = parts.next() {
            if !pass.is_empty() {
                let ns = from_rust_string(env, pass.to_string());
                return autorelease(env, ns);
            }
        }
    }
    nil
}

- (id)path {
    // Override the existing path to handle non-file URLs properly.
    match *env.objc.borrow(this) {
        NSURLHostObject::FileURL { ns_string, .. } => ns_string,
        NSURLHostObject::OtherURL { ns_string } => {
            let str = to_rust_string(env, ns_string).into_owned();
            // Strip scheme://host[:port][userinfo@]
            let after_scheme = if let Some(pos) = str.find("://") {
                &str[pos + 3..]
            } else {
                return ns_string;
            };
            let after_authority = if let Some(slash) = after_scheme.find('/') {
                &after_scheme[slash..]
            } else {
                return nil;
            };
            // Strip query and fragment
            let path = after_authority
                .split('?').next().unwrap_or("")
                .split('#').next().unwrap_or("");
            if path.is_empty() {
                return nil;
            }
            let ns = from_rust_string(env, path.to_string());
            autorelease(env, ns)
        }
    }
}

- (id)query {
    let s: id = msg![env; this absoluteString];
    let str = to_rust_string(env, s).into_owned();
    // Find ? but stop before #
    let after_q = if let Some(q) = str.find('?') {
        &str[q + 1..]
    } else {
        return nil;
    };
    let query = after_q.split('#').next().unwrap_or("");
    if query.is_empty() {
        return nil;
    }
    let ns = from_rust_string(env, query.to_string());
    autorelease(env, ns)
}

- (id)fragment {
    let s: id = msg![env; this absoluteString];
    let str = to_rust_string(env, s).into_owned();
    if let Some(hash) = str.find('#') {
        let fragment = &str[hash + 1..];
        if !fragment.is_empty() {
            let ns = from_rust_string(env, fragment.to_string());
            return autorelease(env, ns);
        }
    }
    nil
}

- (id)parameterString {
    // Semicolon-delimited parameter string (rarely used, part of old RFC 2396).
    nil
}

- (id)resourceSpecifier {
    // Everything after the scheme colon: "//host/path?query#fragment"
    let s: id = msg![env; this absoluteString];
    let str = to_rust_string(env, s).into_owned();
    if let Some(colon) = str.find(':') {
        let spec = &str[colon + 1..];
        if !spec.is_empty() {
            let ns = from_rust_string(env, spec.to_string());
            return autorelease(env, ns);
        }
    }
    nil
}

- (bool)isFileReferenceURL {
    false
}

- (id)fileReferenceURL {
    if msg![env; this isFileURL] {
        retain(env, this);
        return autorelease(env, this);
    }
    nil
}

- (id)filePathURL {
    if msg![env; this isFileURL] {
        retain(env, this);
        return autorelease(env, this);
    }
    nil
}

- (id)absoluteURL {
    this
}

- (id)baseURL {
    nil
}

- (id)relativeURL {
    this
}

- (id)standardizedURL {
    // Return self — no symlink resolution.
    this
}

- (id)lastPathComponent {
    let path: id = msg![env; this path];
    msg![env; path lastPathComponent]
}

- (id)pathExtension {
    let path: id = msg![env; this path];
    msg![env; path pathExtension]
}

- (id)pathComponents { // NSArray* of NSString*
    let path: id = msg![env; this path];
    msg![env; path pathComponents]
}

- (bool)hasDirectoryPath {
    let path: id = msg![env; this path];
    let s = to_rust_string(env, path);
    s.ends_with('/')
}

// MARK: - Path manipulation

- (id)URLByAppendingPathComponent:(id)component { // NSString*
    msg![env; this URLByAppendingPathComponent:component isDirectory:false]
}

- (id)URLByAppendingPathComponent:(id)path_component // NSString*
                      isDirectory:(bool)is_directory {
    let &NSURLHostObject::FileURL { ns_string, .. } = env.objc.borrow(this) else {
        log!("Warning: URLByAppendingPathComponent: called on non-file URL");
        return this;
    };
    let mut path: id = msg![env; ns_string stringByAppendingPathComponent:path_component];
    if is_directory {
        path = msg![env; path stringByAppendingString:(get_static_str(env, "/"))];
    }
    msg_class![env; NSURL fileURLWithPath:path]
}

- (id)URLByAppendingPathExtension:(id)ext { // NSString*
    let path: id = msg![env; this path];
    let new_path: id = msg![env; path stringByAppendingPathExtension:ext];
    msg_class![env; NSURL fileURLWithPath:new_path]
}

- (id)URLByDeletingLastPathComponent {
    let &NSURLHostObject::FileURL { ns_string, .. } = env.objc.borrow(this) else {
        log!("Warning: URLByDeletingLastPathComponent: called on non-file URL");
        return this;
    };
    let path: id = msg![env; ns_string stringByDeletingLastPathComponent];
    msg_class![env; NSURL fileURLWithPath:path]
}

- (id)URLByDeletingPathExtension {
    let path: id = msg![env; this path];
    let new_path: id = msg![env; path stringByDeletingPathExtension];
    msg_class![env; NSURL fileURLWithPath:new_path]
}

- (id)URLByResolvingSymlinksInPath {
    // No symlink support — return self.
    this
}

- (id)URLByStandardizingPath {
    this
}

// MARK: - Resource values (stub)

- (bool)getResourceValue:(MutPtr<id>)value forKey:(id)key error:(MutPtr<id>)_err {
    let key_str = to_rust_string(env, key);
    if key_str == "NSURLIsDirectoryKey" {
        // ИСПРАВЛЕНИЕ ЗДЕСЬ: Добавлена явная аннотация типа `: bool`
        let is_dir: bool = msg![env; this hasDirectoryPath]; 
        let ns_bool = msg_class![env; NSNumber numberWithBool:is_dir];
        env.mem.write(value, ns_bool);
        return true;
    }
    // Default to nil/false for others
    if !value.is_null() { env.mem.write(value, nil); }
    false
}

- (bool)setResourceValue:(id)_value      // id
                  forKey:(id)_key        // NSURLResourceKey
                   error:(MutPtr<id>)_err // NSError**
{
    false
}

// MARK: - File system representation

- (bool)getFileSystemRepresentation:(MutPtr<u8>)buffer
                          maxLength:(NSUInteger)buffer_size {
   
    let &NSURLHostObject::FileURL { ns_string, .. } = env.objc.borrow(this) else {
        return false;
    };
    msg![env; ns_string getCString:buffer
                         maxLength:buffer_size
                          encoding:NSUTF8StringEncoding]
}

// MARK: - Equality

- (bool)isEqual:(id)other {
    if other == nil { return false; }
    if this == other { return true; }
    let a: id = msg![env; this absoluteString];
    let b: id = msg![env; other absoluteString];
    msg![env; a isEqualToString:b]
}

- (NSUInteger)hash {
    let s: id = msg![env; this absoluteString];
    msg![env; s hash]
}

@end

@implementation NSNetServiceBrowser: NSObject
@end

@implementation NSHTTPURLResponse: NSObject
@end

// MARK: - NSURLCache stub

@implementation NSURLCache: NSObject

+ (id)sharedURLCache {
    nil
}

+ (())setSharedURLCache:(id)_cache {
    log!("NSURLCache setSharedURLCache: stubbed");
}

- (id)initWithMemoryCapacity:(NSUInteger)_mem
                diskCapacity:(NSUInteger)_disk
                    diskPath:(id)_path {
    this
}

- (NSUInteger)memoryCapacity   { 0 }
- (NSUInteger)diskCapacity     { 0 }
- (NSUInteger)currentMemoryUsage { 0 }
- (NSUInteger)currentDiskUsage   { 0 }

- (())setMemoryCapacity:(NSUInteger)_cap { }
- (())setDiskCapacity:(NSUInteger)_cap   { }

- (id)cachedResponseForRequest:(id)_request { nil }

- (())storeCachedResponse:(id)_response forRequest:(id)_request {
    log!("NSURLCache storeCachedResponse:forRequest: stubbed");
}

- (())removeCachedResponseForRequest:(id)_request { }
- (())removeAllCachedResponses { }

@end

};

/// Shortcut for host code, provides a view of a URL as a path.
pub fn to_rust_path(env: &mut Environment, url: id) -> Cow<'static, GuestPath> {
    let path_string: id = msg![env; url path];
    match to_rust_string(env, path_string) {
        Cow::Borrowed(path) => Cow::Borrowed(path.as_ref()),
        Cow::Owned(path_buf) => Cow::Owned(path_buf.into()),
    }
}

