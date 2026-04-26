/*
 * Этот исходный код подчиняется условиям лицензии Mozilla Public License, v. 2.0.
 * Если копия MPL не поставлялась с этим файлом, вы можете получить её на https://mozilla.org/MPL/2.0/.
 */
//! OpenAL.
//!
//! Это тонкая оболочка поверх OpenAL Soft, см. [crate::audio::openal].
//!
//! Ресурсы:
//! - [Спецификация OpenAL 1.1](https://www.openal.org/documentation/openal-1.1-specification.pdf)
//! - Apple [Technical Note TN2199: OpenAL FAQ для iPhone OS](https://web.archive.org/web/20090826202158/http://developer.apple.com/iPhone/library/technotes/tn2008/tn2199.html) (также доступно [здесь](https://developer.apple.com/library/archive/technotes/tn2199/_index.html))

use crate::audio::openal as al;
use crate::audio::openal::al_types::*;
use crate::audio::openal::alc_types::*;
use crate::audio::openal::{
    OpenAL, OpenALContext, ALC_DEVICE_SPECIFIER, ALC_FREQUENCY, ALC_MONO_SOURCES, ALC_REFRESH,
    ALC_STEREO_SOURCES, ALC_SYNC, AL_EXTENSIONS, AL_RENDERER, AL_VENDOR, AL_VERSION,
};
use crate::dyld::{export_c_func, FunctionExports, HostDylib};
use crate::libc::string::strcmp;
use crate::mem::{ConstPtr, ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr, Ptr, SafeWrite};
use crate::Environment;
use std::collections::HashMap;
use std::ffi::{CStr, CString};

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/OpenAL.framework/OpenAL",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};

#[derive(Default)]
pub struct State {
    devices: HashMap<MutPtr<GuestALCdevice>, *mut ALCdevice>,
    contexts: HashMap<MutPtr<GuestALCcontext>, OpenALContext>,
    strings_cache: HashMap<ALenum, ConstPtr<u8>>,
    current_ctx: MutPtr<GuestALCcontext>,
}
impl State {
    fn get(env: &mut Environment) -> &mut Self {
        &mut env.framework_state.openal
    }

    fn try_make_current(env: &mut Environment) -> Option<OpenAL<'_>> {
        let state = &mut env.framework_state.openal;
        state
            .contexts
            .get_mut(&state.current_ctx)
            .map(|ctx| ctx.make_current(&mut env.openal_manager))
    }
}

/// Непрозрачный тип в гостевой памяти, представляющий [ALCdevice] из памяти хоста.
struct GuestALCdevice {
    _filler: u8,
}
impl SafeWrite for GuestALCdevice {}
/// Непрозрачный тип в гостевой памяти, представляющий [ALCcontext] из памяти хоста.
struct GuestALCcontext {
    _filler: u8,
}
impl SafeWrite for GuestALCcontext {}

macro_rules! try_get_context {
    ($env: ident, $name: ident) => {
        let state = &mut $env.framework_state.openal;
        let Some($name) = state
            .contexts
            .get_mut(&state.current_ctx)
            .map(|ctx| ctx.make_current(&mut $env.openal_manager))
        else {
            log_dbg!(
                "Попытка получить контекст, но текущий активный контекст {:?} недействителен, пропускаем!",
                State::get($env).current_ctx
            );
            // TODO: установить ошибку
            return;
        };
    };
    ($env: ident, $name: ident, $rval: expr) => {
        let state = &mut $env.framework_state.openal;
        let Some($name) = state
            .contexts
            .get_mut(&state.current_ctx)
            .map(|ctx| ctx.make_current(&mut $env.openal_manager))
        else {
            log_dbg!(
                "Попытка получить контекст, но текущий активный контекст {:?} недействителен, пропускаем!",
                State::get($env).current_ctx
            );
            // TODO: установить ошибку
            return $rval;
        };
    };
}

// === alc.h ===

fn alcOpenDevice(env: &mut Environment, devicename: ConstPtr<u8>) -> MutPtr<GuestALCdevice> {
    if !devicename.is_null() {
        // Если имя устройства не равно null, мы проверяем, соответствует ли оно тому,
        // которое было получено при вызове alcGetString(NULL, ALC_DEVICE_SPECIFIER).
        // Если нет, возвращаем NULL.

        let d_name = alcGetString(env, Ptr::null(), ALC_DEVICE_SPECIFIER);
        if strcmp(env, d_name, devicename) != 0 {
            log!(
                "Неподдерживаемое имя устройства {:?}, поддерживается {:?}. Возвращаем NULL",
                env.mem.cstr_at_utf8(devicename),
                env.mem.cstr_at_utf8(d_name)
            );
            env.mem.free(d_name.cast_mut().cast());
            return Ptr::null();
        }
        env.mem.free(d_name.cast_mut().cast());
    }

    let res = unsafe { al::alcOpenDevice(std::ptr::null()) };
    if res.is_null() {
        log_dbg!("alcOpenDevice(NULL) вернул NULL");
        return Ptr::null();
    }

    let guest_res = env.mem.alloc_and_write(GuestALCdevice { _filler: 0 });
    State::get(env).devices.insert(guest_res, res);
    log_dbg!("alcOpenDevice(NULL) => {:?} (хост: {:?})", guest_res, res,);
    guest_res
}
fn alcCloseDevice(env: &mut Environment, device: MutPtr<GuestALCdevice>) -> bool {
    if device.is_null() {
        log!("alcCloseDevice() вызван с устройством NULL, игнорируем");
        return false;
    }
    let host_device = State::get(env).devices.remove(&device).unwrap();
    env.mem.free(device.cast());
    let res = unsafe { al::alcCloseDevice(host_device) };
    log_dbg!("alcCloseDevice({:?}) => {:?}", device, res,);
    res != al::ALC_FALSE
}

fn alcGetError(env: &mut Environment, device: MutPtr<GuestALCdevice>) -> i32 {
    let &host_device = State::get(env).devices.get(&device).unwrap();

    let res = unsafe { al::alcGetError(host_device) };
    log_dbg!("alcGetError({:?}) => {:#x}", host_device, res);
    res
}

fn alcGetString(
    env: &mut Environment,
    device: MutPtr<GuestALCdevice>,
    param: ALenum,
) -> ConstPtr<u8> {
    // Получаем реальный (хостовый) указатель на устройство, если игра его передала
    let host_device = if device.is_null() {
        std::ptr::null_mut()
    } else {
        match State::get(env).devices.get(&device) {
            Some(&dev) => dev,
            None => {
                log!("Предупреждение: alcGetString вызван с неизвестным устройством {:?}", device);
                std::ptr::null_mut()
            }
        }
    };

    // Передаем хостовое устройство (или NULL) в настоящую библиотеку OpenAL Soft
    let res = unsafe { al::alcGetString(host_device, param) };
    
    // Защита от краша, если OpenAL ничего не вернул
    if res.is_null() {
        log_dbg!("alcGetString({:?}, {}) вернул NULL", device, param);
        return Ptr::null();
    }

    let s = unsafe { CStr::from_ptr(res) };
    log_dbg!("alcGetString({:?}, {}) => {:?}", device, param, s);
    log!("TODO: alcGetString({}) приводит к утечке памяти", param);
    
    // Аллоцируем строку в памяти гостя
    env.mem.alloc_and_write_cstr(s.to_bytes()).cast_const()
}

const ALLOWED_CONTEXT_ATTRIBUTES: [ALCint; 5] = [
    ALC_FREQUENCY,
    ALC_REFRESH,
    ALC_SYNC,
    ALC_MONO_SOURCES,
    ALC_STEREO_SOURCES,
];

fn alcCreateContext(
    env: &mut Environment,
    device: MutPtr<GuestALCdevice>,
    attr_list: ConstPtr<i32>,
) -> MutPtr<GuestALCcontext> {
    let attr_list_ptr: *const ALCint = if attr_list.is_null() {
        std::ptr::null()
    } else {
        let mut ptr: MutPtr<i32> = attr_list.cast_mut();
        // список атрибутов завершается нулем (NULL)
        while env.mem.read(ptr) != 0 {
            let attr = env.mem.read(ptr);
            log_dbg!(
                "Атрибут alcCreateContext {:#x} => {}",
                attr,
                env.mem.read(ptr + 1)
            );
            assert!(ALLOWED_CONTEXT_ATTRIBUTES.contains(&attr)); // TODO
            ptr += 2;
        }

        let list_size = Ptr::to_bits(ptr) - Ptr::to_bits(attr_list);
        let attr_list_slice = env.mem.bytes_at(attr_list.cast(), list_size);
        attr_list_slice.as_ptr() as *const _
    };

    let state = State::get(env);
    let &host_device = state.devices.get(&device).unwrap();

    let res = unsafe {
        OpenALContext::new_with_device_and_attrlist(
            env.openal_manager.as_mut(),
            host_device,
            attr_list_ptr,
        )
    };
    let Ok(ctx) = res else {
        log_dbg!("alcCreateContext({:?}, (...)) вернул NULL", device);
        return Ptr::null();
    };

    let guest_res = env.mem.alloc_and_write(GuestALCcontext { _filler: 0 });

    log_dbg!(
        "alcCreateContext({:?}, NULL) => {:?} (хост: {:?})",
        device,
        guest_res,
        ctx,
    );

    State::get(env).contexts.insert(guest_res, ctx);
    guest_res
}
fn alcDestroyContext(env: &mut Environment, context: MutPtr<GuestALCcontext>) {
    if context.is_null() {
        log!("alcDestroyContext() вызван с контекстом NULL, игнорируем");
        return;
    }
    let _host_context = State::get(env).contexts.remove(&context).unwrap();
    env.mem.free(context.cast());
    log!("alcDestroyContext({:?})", context);
}

fn alcProcessContext(env: &mut Environment, context: MutPtr<GuestALCcontext>) {
    if context.is_null() {
        log!("alcProcessContext() вызван с контекстом NULL, игнорируем");
        return;
    }
    let host_context = State::get(env).contexts.get_mut(&context).unwrap();
    host_context.ProcessContext()
}
fn alcSuspendContext(env: &mut Environment, context: MutPtr<GuestALCcontext>) {
    if context.is_null() {
        log!("alcSuspendContext() вызван с контекстом NULL, игнорируем");
        return;
    }
    let host_context = State::get(env).contexts.get_mut(&context).unwrap();
    host_context.SuspendContext()
}

fn alcMakeContextCurrent(env: &mut Environment, context: MutPtr<GuestALCcontext>) -> bool {
    let res = if context.is_null() || State::get(env).contexts.contains_key(&context) {
        State::get(env).current_ctx = context;
        true
    } else {
        false
    };
    log_dbg!("alcMakeContextCurrent({:?}) => {}", context, res);
    res
}

fn alcGetCurrentContext(env: &mut Environment) -> MutPtr<GuestALCcontext> {
    State::get(env).current_ctx
}

fn alcGetContextsDevice(
    env: &mut Environment,
    context: MutPtr<GuestALCcontext>,
) -> MutPtr<GuestALCdevice> {
    if context.is_null() {
        log!("alcGetContextsDevice() вызван с контекстом NULL, игнорируем");
        return Ptr::null();
    }
    let host_context = State::get(env).contexts.get(&context).unwrap();
    let host_device = host_context.GetContextsDevice();
    *State::get(env)
        .devices
        .iter()
        .find(|(&_guest, &host)| host == host_device)
        .unwrap()
        .0
}

fn alcGetProcAddress(
    env: &mut Environment,
    _device: ConstPtr<GuestALCdevice>,
    func_name: ConstPtr<u8>,
) -> MutVoidPtr {
    let mangled_func_name = format!("_{}", env.mem.cstr_at_utf8(func_name).unwrap());
    assert!(mangled_func_name.starts_with("_al"));

    if let Ok(ptr) = env
        .dyld
        .create_proc_address(&mut env.mem, &mut env.cpu, &mangled_func_name)
    {
        Ptr::from_bits(ptr.addr_with_thumb_bit())
    } else {
        if mangled_func_name == "_alcMacOSMixerOutputRate" {
            log!("Допускаем несуществующую функцию alcMacOSMixerOutputRate() в alcGetProcAddress(), возвращаем NULL.");
            return Ptr::null();
        }
        panic!(
            "Запрос адреса процедуры для нереализованной функции OpenAL {mangled_func_name}"
        );
    }
}

// TODO: больше функций

// === al.h ===

fn alGetError(env: &mut Environment) -> i32 {
    // Super Monkey Ball и другие приложения пытаются использовать эту функцию (вместо
    // alcGetError), чтобы узнать, удалось ли открыть устройство. Это неправильно и похоже на ошибку.
    // Вероятно, iPhone OS не обращает на это внимания, но OpenAL Soft в этом случае возвращает ошибку,
    // и игра пропускает оставшуюся часть инициализации звука.
    // Некоторые другие приложения пытаются вызвать это в удаленном контексте
    // (обычно из другого потока), поэтому нам нужно просто тихо игнорировать это.
    try_get_context!(env, context, al::AL_NO_ERROR);
    let res = unsafe { context.GetError() };
    log_dbg!("alGetError() => {:#x}", res);
    res
}

fn alDistanceModel(env: &mut Environment, value: ALenum) {
    try_get_context!(env, context);
    unsafe { context.DistanceModel(value) };
}

fn alGetEnumValue(env: &mut Environment, enumName: ConstPtr<u8>) -> ALenum {
    let s = env.mem.cstr_at_utf8(enumName).unwrap();
    let ss = CString::new(s).unwrap();

    let res = unsafe { OpenALContext::GetEnumValue(ss.as_ptr()) };
    log_dbg!("alGetEnumValue({:?}) => {:?}", s, res);
    res
}

fn alIsBuffer(env: &mut Environment, buffer: ALuint) -> ALboolean {
    try_get_context!(env, context, 0);
    unsafe { context.IsBuffer(buffer) }
}

fn alGetBufferi(env: &mut Environment, buffer: ALuint, param: ALenum, value: MutPtr<ALint>) {
    let value = env.mem.ptr_at(value, 1);
    try_get_context!(env, context);
    unsafe { context.GetBufferi(buffer, param, value) }
}

fn alIsSource(env: &mut Environment, source: ALuint) -> ALboolean {
    try_get_context!(env, context, 0);
    unsafe { context.IsSource(source) }
}

fn alIsExtensionPresent(env: &mut Environment, ext_name: ConstPtr<u8>) -> ALboolean {
    try_get_context!(env, context, 0);
    let s = env.mem.cstr_at_utf8(ext_name).unwrap();
    let ss = CString::new(s).unwrap();
    unsafe { context.IsExtensionPresent(ss.as_ptr()) }
}

fn alEnable(env: &mut Environment, capability: ALenum) {
    try_get_context!(env, context);
    unsafe { context.Enable(capability) };
}

fn alGetString(env: &mut Environment, param: ALenum) -> ConstPtr<u8> {
    let res = if let Some(&str) = env.framework_state.openal.strings_cache.get(&param) {
        str
    } else {
        // Эти значения извлечены из iPhone 3GS, iOS 4.0.1
        // (такие же значения были замечены в симуляторе iPhone)
        let s: &[u8] = match param {
            AL_VENDOR => b"Apple Inc.",
            AL_VERSION => b"1.1",
            AL_RENDERER => b"Software",
            AL_EXTENSIONS => b"AL_EXT_OFFSET AL_EXT_LINEAR_DISTANCE AL_EXT_EXPONENT_DISTANCE AL_EXT_STATIC_BUFFER",
            _ => unreachable!()
        };
        let new_str = env.mem.alloc_and_write_cstr(s).cast_const();
        env.framework_state
            .openal
            .strings_cache
            .insert(param, new_str);
        new_str
    };
    log_dbg!(
        "alGetString({}) => '{:?}'",
        param,
        env.mem.cstr_at_utf8(res)
    );
    res
}

fn alListenerf(env: &mut Environment, param: ALenum, value: ALfloat) {
    try_get_context!(env, context);
    unsafe { context.Listenerf(param, value) };
}
fn alListenerfv(env: &mut Environment, param: ALenum, values: ConstPtr<ALfloat>) {
    // мы предполагаем, что должен быть передан хотя бы 1 параметр
    let values = env.mem.ptr_at(values, 1);
    try_get_context!(env, context);
    unsafe { context.Listenerfv(param, values) };
}
fn alListener3f(
    env: &mut Environment,

    param: ALenum,
    value1: ALfloat,
    value2: ALfloat,
    value3: ALfloat,
) {
    try_get_context!(env, context);
    unsafe { context.Listener3f(param, value1, value2, value3) };
}
fn alListeneri(env: &mut Environment, param: ALenum, value: ALint) {
    try_get_context!(env, context);
    unsafe { context.Listeneri(param, value) };
}
fn alListener3i(env: &mut Environment, param: ALenum, value1: ALint, value2: ALint, value3: ALint) {
    try_get_context!(env, context);
    unsafe { context.Listener3i(param, value1, value2, value3) };
}
fn alListeneriv(env: &mut Environment, param: ALenum, values: ConstPtr<ALint>) {
    let values = env.mem.ptr_at(values, 3); // верхняя граница
    try_get_context!(env, context);
    unsafe { context.Listeneriv(param, values) };
}

fn alGetListenerf(env: &mut Environment, param: ALenum, value: MutPtr<ALfloat>) {
    let value = env.mem.ptr_at_mut(value, 1);
    try_get_context!(env, context);
    unsafe { context.GetListenerf(param, value) };
}
fn alGetListener3f(
    env: &mut Environment,

    param: ALenum,
    value1: MutPtr<ALfloat>,
    value2: MutPtr<ALfloat>,
    value3: MutPtr<ALfloat>,
) {
    try_get_context!(env, context);
    let mut values = [0.0; 3];
    unsafe { context.GetListener3f(param, &mut values[0], &mut values[1], &mut values[2]) };
    env.mem.write(value1, values[0]);
    env.mem.write(value2, values[1]);
    env.mem.write(value3, values[2]);
}
fn alGetListenerfv(env: &mut Environment, param: ALenum, values: MutPtr<ALfloat>) {
    let values = env.mem.ptr_at_mut(values, 3); // верхняя граница
    try_get_context!(env, context);
    unsafe { context.GetListenerfv(param, values) };
}
fn alGetListeneri(env: &mut Environment, param: ALenum, value: MutPtr<ALint>) {
    let value = env.mem.ptr_at_mut(value, 1);
    try_get_context!(env, context);
    unsafe { context.GetListeneri(param, value) };
}
fn alGetListener3i(
    env: &mut Environment,

    param: ALenum,
    value1: MutPtr<ALint>,
    value2: MutPtr<ALint>,
    value3: MutPtr<ALint>,
) {
    let mut values = [0; 3];
    try_get_context!(env, context);
    unsafe { context.GetListener3i(param, &mut values[0], &mut values[1], &mut values[2]) };
    env.mem.write(value1, values[0]);
    env.mem.write(value2, values[1]);
    env.mem.write(value3, values[2]);
}
fn alGetListeneriv(env: &mut Environment, param: ALenum, values: MutPtr<ALint>) {
    let values = env.mem.ptr_at_mut(values, 3); // верхняя граница
    try_get_context!(env, context);
    unsafe { context.GetListeneriv(param, values) };
}

fn alGenSources(env: &mut Environment, n: ALsizei, sources: MutPtr<ALuint>) {
    let n_usize: GuestUSize = match n.try_into() {
        Ok(val) => val,
        Err(_) => {
            log!("Предупреждение: alGenSources вызван с отрицательным количеством {}", n);
            return;
        }
    };
    let sources = env.mem.ptr_at_mut(sources, n_usize);
    try_get_context!(env, context);
    unsafe { context.GenSources(n, sources) };
}
fn alDeleteSources(env: &mut Environment, n: ALsizei, sources: ConstPtr<ALuint>) {
    let n_usize: GuestUSize = match n.try_into() {
        Ok(val) => val,
        Err(_) => {
            log!("Предупреждение: alDeleteSources вызван с отрицательным количеством {}", n);
            return;
        }
    };
    let sources = env.mem.ptr_at(sources, n_usize);
    try_get_context!(env, context);
    unsafe { context.DeleteSources(n, sources) };
}

fn alSourcef(env: &mut Environment, source: ALuint, param: ALenum, value: ALfloat) {
    try_get_context!(env, context);
    unsafe { context.Sourcef(source, param, value) };
}
fn alSourcefv(env: &mut Environment, source: ALuint, param: ALenum, values: ConstPtr<ALfloat>) {
    // мы предполагаем, что должен быть передан хотя бы 1 параметр
    let values = env.mem.ptr_at(values, 1);
    try_get_context!(env, context);
    unsafe { context.Sourcefv(source, param, values) };
}
fn alSource3f(
    env: &mut Environment,
    source: ALuint,
    param: ALenum,
    value1: ALfloat,
    value2: ALfloat,
    value3: ALfloat,
) {
    try_get_context!(env, context);
    unsafe { context.Source3f(source, param, value1, value2, value3) };
}
fn alSourcei(env: &mut Environment, source: ALuint, param: ALenum, value: ALint) {
    try_get_context!(env, context);
    unsafe { context.Sourcei(source, param, value) };
}
fn alSource3i(
    env: &mut Environment,
    source: ALuint,
    param: ALenum,
    value1: ALint,
    value2: ALint,
    value3: ALint,
) {
    try_get_context!(env, context);
    unsafe { context.Source3i(source, param, value1, value2, value3) };
}
fn alSourceiv(env: &mut Environment, source: ALuint, param: ALenum, values: ConstPtr<ALint>) {
    let values = env.mem.ptr_at(values, 3); // верхняя граница
    try_get_context!(env, context);
    unsafe { context.Sourceiv(source, param, values) };
}

fn alGetSourcef(env: &mut Environment, source: ALuint, param: ALenum, value: MutPtr<ALfloat>) {
    let value = env.mem.ptr_at_mut(value, 1);
    try_get_context!(env, context);
    unsafe { context.GetSourcef(source, param, value) };
}
fn alGetSource3f(
    env: &mut Environment,
    source: ALuint,
    param: ALenum,
    value1: MutPtr<ALfloat>,
    value2: MutPtr<ALfloat>,
    value3: MutPtr<ALfloat>,
) {
    let mut values = [0.0; 3];
    try_get_context!(env, context);
    unsafe {
        context.GetSource3f(
            source,
            param,
            &mut values[0],
            &mut values[1],
            &mut values[2],
        )
    };
    env.mem.write(value1, values[0]);
    env.mem.write(value2, values[1]);
    env.mem.write(value3, values[2]);
}
fn alGetSourcefv(env: &mut Environment, source: ALuint, param: ALenum, values: MutPtr<ALfloat>) {
    let values = env.mem.ptr_at_mut(values, 3); // верхняя граница
    try_get_context!(env, context);
    unsafe { context.GetSourcefv(source, param, values) };
}
fn alGetSourcei(env: &mut Environment, source: ALuint, param: ALenum, value: MutPtr<ALint>) {
    let value = env.mem.ptr_at_mut(value, 1);
    try_get_context!(env, context);
    unsafe { context.GetSourcei(source, param, value) };
}
fn alGetSource3i(
    env: &mut Environment,
    source: ALuint,
    param: ALenum,
    value1: MutPtr<ALint>,
    value2: MutPtr<ALint>,
    value3: MutPtr<ALint>,
) {
    let mut values = [0; 3];
    try_get_context!(env, context);
    unsafe {
        context.GetSource3i(
            source,
            param,
            &mut values[0],
            &mut values[1],
            &mut values[2],
        )
    };
    env.mem.write(value1, values[0]);
    env.mem.write(value2, values[1]);
    env.mem.write(value3, values[2]);
}
fn alGetSourceiv(env: &mut Environment, source: ALuint, param: ALenum, values: MutPtr<ALint>) {
    let values = env.mem.ptr_at_mut(values, 3); // верхняя граница
    try_get_context!(env, context);
    unsafe { context.GetSourceiv(source, param, values) };
}

fn alSourcePlay(env: &mut Environment, source: ALuint) {
    try_get_context!(env, context);
    unsafe { context.SourcePlay(source) };
    let mut state: ALint = 0;
    let mut max_gain: ALfloat = 0.0;
    let mut buffer: ALint = 0;
    let err: ALenum;
    unsafe {
        context.GetSourcei(source, al::AL_SOURCE_STATE, &mut state as *mut _);
        context.GetSourcef(source, al::AL_MAX_GAIN, &mut max_gain as *mut _);
        context.GetSourcei(source, al::AL_BUFFER, &mut buffer as *mut _);
        err = context.GetError();
    }
    log!(
        "alSourcePlay(source={}) -> state=0x{:x}, max_gain={}, buffer={}, err=0x{:x}",
        source, state, max_gain, buffer, err
    );
}
fn alSourcePause(env: &mut Environment, source: ALuint) {
    try_get_context!(env, context);
    unsafe { context.SourcePause(source) };
}
fn alSourceStop(env: &mut Environment, source: ALuint) {
    try_get_context!(env, context);
    unsafe { context.SourceStop(source) };
}
fn alSourceRewind(env: &mut Environment, source: ALuint) {
    try_get_context!(env, context);
    unsafe { context.SourceRewind(source) };
}

fn alSourceQueueBuffers(
    env: &mut Environment,
    source: ALuint,
    nb: ALsizei,
    buffers: ConstPtr<ALuint>,
) {
    let nb_usize: GuestUSize = match nb.try_into() {
        Ok(val) => val,
        Err(_) => {
            log!("Предупреждение: alSourceQueueBuffers вызван с отрицательным количеством {}", nb);
            return;
        }
    };
    let buffers = env.mem.ptr_at(buffers, nb_usize);
    try_get_context!(env, context);
    unsafe { context.SourceQueueBuffers(source, nb, buffers) }
}
fn alSourceUnqueueBuffers(
    env: &mut Environment,
    source: ALuint,
    nb: ALsizei,
    buffers: MutPtr<ALuint>,
) {
    // Пример кода Apple для зацикленного звукового эффекта содержит функцию с названием
    // SoundEngineEffect::ClearSourceBuffers(), которая имеет следующий шаблон:
    //
    //    alGetSourcei(source, AL_BUFFERS_QUEUED, &n);
    //    alSourceUnqueueBuffers(source, n, &buffers);
    //
    // К сожалению, при некоторых обстоятельствах этот код некорректен: извлечение буферов из 
    // очереди во время их воспроизведения не разрешено спецификацией OpenAL! Возможно, это по 
    // какой-то причине работало с реализацией OpenAL от Apple, но OpenAL Soft этого не допускает, 
    // поэтому многие приложения, использовавшие этот пример (например, Super Monkey Ball), 
    // сталкиваются с неожиданной ошибкой OpenAL.
    //
    // Ограничение количества извлекаемых буферов кажется эффективным обходным путем для
    // протестированных приложений. Этот пример кода на самом деле не использует возвращаемые 
    // идентификаторы буферов, поэтому нет проблемы в том, что мы запишем их меньше.
    try_get_context!(env, context);
    let buffers_processed = {
        let mut val = 0;
        unsafe { context.GetSourcei(source, al::AL_BUFFERS_PROCESSED, &mut val) };
        val
    };
    let nb = if buffers_processed < nb {
        log_dbg!("Применяем обходной путь для бага в примере кода Apple: игнорируем удаление {}/{} обработанных буферов из очереди для источника {}", nb, buffers_processed, source);
        buffers_processed
    } else {
        nb
    };

    let nb_usize: GuestUSize = match nb.try_into() {
        Ok(val) => val,
        Err(_) => {
            log!("Предупреждение: alSourceUnqueueBuffers в итоге получил отрицательное количество {}", nb);
            return;
        }
    };
    let buffers = env.mem.ptr_at_mut(buffers, nb_usize);
    unsafe { context.SourceUnqueueBuffers(source, nb, buffers) }
}

fn alGenBuffers(env: &mut Environment, n: ALsizei, buffers: MutPtr<ALuint>) {
    let n_usize: GuestUSize = match n.try_into() {
        Ok(val) => val,
        Err(_) => {
            log!("Предупреждение: alGenBuffers вызван с отрицательным количеством {}", n);
            return;
        }
    };
    let buffers = env.mem.ptr_at_mut(buffers, n_usize);
    try_get_context!(env, context);
    unsafe { context.GenBuffers(n, buffers) };
}
fn alDeleteBuffers(env: &mut Environment, n: ALsizei, buffers: ConstPtr<ALuint>) {
    let n_usize: GuestUSize = match n.try_into() {
        Ok(val) => val,
        Err(_) => {
            log!("Предупреждение: alDeleteBuffers вызван с отрицательным количеством {}", n);
            return;
        }
    };
    let buffers = env.mem.ptr_at(buffers, n_usize);
    let Some(context) = State::try_make_current(env) else {
        log!(
            "Попытка вызова alDeleteBuffers({}, {:?}) с неактивным контекстом {:?}, пропускаем!",
            n,
            buffers,
            State::get(env).current_ctx
        );
        return;
    };
    unsafe { context.DeleteBuffers(n, buffers) };
}

fn alBufferData(
    env: &mut Environment,
    buffer: ALuint,
    format: ALenum,
    data: ConstVoidPtr,
    size: ALsizei,
    samplerate: ALsizei,
) {
    let size_usize: GuestUSize = match size.try_into() {
        Ok(val) => val,
        Err(_) => {
            log!("Предупреждение: alBufferData вызван с отрицательным размером {}", size);
            return;
        }
    };
    let data_ptr: *const ALvoid = if data.is_null() {
        std::ptr::null()
    } else {
        let data_slice = env.mem.bytes_at(data.cast(), size_usize);
        data_slice.as_ptr() as *const _
    };
    log!(
        "alBufferData(buffer={}, format=0x{:x}, size={}, samplerate={})",
        buffer, format, size, samplerate
    );
    try_get_context!(env, context);
    unsafe { context.BufferData(buffer, format, data_ptr, size, samplerate) };
}

/// Это расширение Apple, которое рассматривает переданные данные как статический буфер,
/// а не временный, что означает, что его никогда не нужно копировать.
/// OpenAL Soft это не поддерживает, поэтому мы передаем управление в `alBufferData`
/// и надеемся, что гостевое приложение не полагается на статичность (не должно).
fn alBufferDataStatic(
    env: &mut Environment,
    buffer: ALuint,
    format: ALenum,
    data: ConstVoidPtr,
    size: ALsizei,
    samplerate: ALsizei,
) {
    alBufferData(env, buffer, format, data, size, samplerate);
}

// Специфичное расширение Apple для OpenAL
fn alcMacOSXMixerOutputRate(_env: &mut Environment, value: ALdouble) {
    log!("Приложение хочет установить частоту дискретизации микшера на {} Гц", value);
}
fn alcMacOSXGetMixerOutputRate(_env: &mut Environment) -> ALdouble {
    // Значение по умолчанию было проверено на iPhone 3GS, iOS 4.0.1
    log!("Приложение хочет получить частоту дискретизации микшера, возвращаем 0 по умолчанию");
    0.0
}

fn alDopplerFactor(env: &mut Environment, value: ALfloat) {
    try_get_context!(env, context);
    unsafe { context.DopplerFactor(value) };
}

fn alDopplerVelocity(env: &mut Environment, value: ALfloat) {
    // По всей видимости, wolf3d устанавливает скорость Доплера в ноль, но это приводит
    // к приглушению всего звука в программной реализации Open AL 1.1!
    // Дополнительную информацию см. в разделе "A note for OpenAL library implementors regarding OpenAL 1.0" 
    // спецификации OpenAL 1.1.
    let bundle_id = env.bundle.bundle_identifier();
    if bundle_id.starts_with("com.zodttd.wolf3d")
        || bundle_id.starts_with("com.idsoftware.wolf3d")
        || bundle_id.starts_with("nu.r3.wolf3d")
    {
        // ИСПРАВЛЕНИЕ: Блокируем вызов только если игра реально передает 0.0,
        // чтобы не сломать звук. Жесткий assert убран.
        if value == 0.0 {
            log_dbg!("Применяем хак для Wolf3D-iOS: игнорируем нулевую скорость Доплера (0.0).");
            return;
        }
    }
    
    // Если передано нормальное значение (например, 1.0) или это другая игра — честно отдаем в OpenAL
    try_get_context!(env, context);
    unsafe { context.DopplerVelocity(value) };
}

fn alSpeedOfSound(env: &mut Environment, value: ALfloat) {
    try_get_context!(env, context);
    unsafe { context.SpeedOfSound(value) };
}

// TODO: больше функций

// Примечание: По некоторым причинам Wolf3d регистрирует много функций OpenAL,
// но фактически использует лишь несколько. Чтобы обойти это, мы просто предоставляем заглушки.

fn alcGetEnumValue(
    _env: &mut Environment,
    _device: MutPtr<GuestALCdevice>,
    _enumName: ConstPtr<u8>,
) -> ALenum {
    todo!();
}
fn alcGetIntegerv(
    _env: &mut Environment,
    _device: MutPtr<GuestALCdevice>,
    _param: ALenum,
    _size: ALCsizei,
    _values: MutPtr<ALCint>,
) {
    todo!();
}
fn alcIsExtensionPresent(
    _env: &mut Environment,
    _device: MutPtr<GuestALCdevice>,
    _extName: ConstPtr<u8>,
) -> ALCboolean {
    0
}
fn alGetBufferf(_env: &mut Environment, _buffer: ALuint, _param: ALenum, _value: MutPtr<ALfloat>) {
    todo!();
}
fn alDisable(_env: &mut Environment, _capability: ALenum) {
    todo!();
}
fn alGetBoolean(_env: &mut Environment, _param: ALenum) -> ALboolean {
    todo!();
}
fn alGetBooleanv(_env: &mut Environment, _param: ALenum, _values: MutPtr<ALboolean>) {
    todo!();
}
fn alGetDouble(_env: &mut Environment, _param: ALenum) -> ALdouble {
    todo!();
}
fn alGetDoublev(_env: &mut Environment, _param: ALenum, _values: MutPtr<ALdouble>) {
    todo!();
}
fn alGetFloat(_env: &mut Environment, _param: ALenum) -> ALfloat {
    todo!();
}
fn alGetFloatv(_env: &mut Environment, _param: ALenum, _values: MutPtr<ALfloat>) {
    todo!();
}
fn alGetInteger(_env: &mut Environment, _param: ALenum) -> ALint {
    todo!();
}
fn alGetIntegerv(_env: &mut Environment, _param: ALenum, _values: MutPtr<ALint>) {
    todo!();
}
fn alGetProcAddress(env: &mut Environment, funcName: ConstPtr<u8>) -> MutVoidPtr {
    alcGetProcAddress(env, Ptr::null(), funcName)
}
fn alIsEnabled(_env: &mut Environment, _capability: ALenum) -> ALboolean {
    todo!();
}
fn alSourcePlayv(_env: &mut Environment, _nsources: ALsizei, _sources: ConstPtr<ALuint>) {
    todo!();
}
fn alSourcePausev(_env: &mut Environment, _nsources: ALsizei, _sources: ConstPtr<ALuint>) {
    todo!();
}
fn alSourceStopv(_env: &mut Environment, _nsources: ALsizei, _sources: ConstPtr<ALuint>) {
    todo!();
}
fn alSourceRewindv(_env: &mut Environment, _nsources: ALsizei, _sources: ConstPtr<ALuint>) {
    todo!();
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(alcOpenDevice(_)),
    export_c_func!(alcCloseDevice(_)),
    export_c_func!(alcGetError(_)),
    export_c_func!(alcCreateContext(_, _)),
    export_c_func!(alcDestroyContext(_)),
    export_c_func!(alcProcessContext(_)),
    export_c_func!(alcSuspendContext(_)),
    export_c_func!(alcMakeContextCurrent(_)),
    export_c_func!(alcGetProcAddress(_, _)),
    export_c_func!(alGetError()),
    export_c_func!(alDistanceModel(_)),
    export_c_func!(alListenerf(_, _)),
    export_c_func!(alListener3f(_, _, _, _)),
    export_c_func!(alListenerfv(_, _)),
    export_c_func!(alListeneri(_, _)),
    export_c_func!(alListener3i(_, _, _, _)),
    export_c_func!(alListeneriv(_, _)),
    export_c_func!(alGetListenerf(_, _)),
    export_c_func!(alGetListener3f(_, _, _, _)),
    export_c_func!(alGetListenerfv(_, _)),
    export_c_func!(alGetListeneri(_, _)),
    export_c_func!(alGetListener3i(_, _, _, _)),
    export_c_func!(alGetListeneriv(_, _)),
    export_c_func!(alGenSources(_, _)),
    export_c_func!(alDeleteSources(_, _)),
    export_c_func!(alSourcef(_, _, _)),
    export_c_func!(alSource3f(_, _, _, _, _)),
    export_c_func!(alSourcefv(_, _, _)),
    export_c_func!(alSourcei(_, _, _)),
    export_c_func!(alSource3i(_, _, _, _, _)),
    export_c_func!(alSourceiv(_, _, _)),
    export_c_func!(alGetSourcef(_, _, _)),
    export_c_func!(alGetSource3f(_, _, _, _, _)),
    export_c_func!(alGetSourcefv(_, _, _)),
    export_c_func!(alGetSourcei(_, _, _)),
    export_c_func!(alGetSource3i(_, _, _, _, _)),
    export_c_func!(alGetSourceiv(_, _, _)),
    export_c_func!(alSourcePlay(_)),
    export_c_func!(alSourcePause(_)),
    export_c_func!(alSourceStop(_)),
    export_c_func!(alSourceRewind(_)),
    export_c_func!(alSourceQueueBuffers(_, _, _)),
    export_c_func!(alSourceUnqueueBuffers(_, _, _)),
    export_c_func!(alGenBuffers(_, _)),
    export_c_func!(alDeleteBuffers(_, _)),
    export_c_func!(alBufferData(_, _, _, _, _)),
    export_c_func!(alBufferDataStatic(_, _, _, _, _)),
    export_c_func!(alcMacOSXMixerOutputRate(_)),
    export_c_func!(alcMacOSXGetMixerOutputRate()),
    export_c_func!(alcGetContextsDevice(_)),
    export_c_func!(alcGetCurrentContext()),
    export_c_func!(alcGetEnumValue(_, _)),
    export_c_func!(alcGetIntegerv(_, _, _, _)),
    export_c_func!(alcGetString(_, _)),
    export_c_func!(alcIsExtensionPresent(_, _)),
    export_c_func!(alIsBuffer(_)),
    export_c_func!(alGetBufferf(_, _, _)),
    export_c_func!(alGetBufferi(_, _, _)),
    export_c_func!(alEnable(_)),
    export_c_func!(alDisable(_)),
    export_c_func!(alDopplerFactor(_)),
    export_c_func!(alDopplerVelocity(_)),
    export_c_func!(alGetBoolean(_)),
    export_c_func!(alGetBooleanv(_, _)),
    export_c_func!(alGetDouble(_)),
    export_c_func!(alGetDoublev(_, _)),
    export_c_func!(alGetFloat(_)),
    export_c_func!(alGetFloatv(_, _)),
    export_c_func!(alGetInteger(_)),
    export_c_func!(alGetIntegerv(_, _)),
    export_c_func!(alGetEnumValue(_)),
    export_c_func!(alGetProcAddress(_)),
    export_c_func!(alGetString(_)),
    export_c_func!(alIsExtensionPresent(_)),
    export_c_func!(alIsEnabled(_)),
    export_c_func!(alIsSource(_)),
    export_c_func!(alSourcePlayv(_, _)),
    export_c_func!(alSourcePausev(_, _)),
    export_c_func!(alSourceStopv(_, _)),
    export_c_func!(alSourceRewindv(_, _)),
    export_c_func!(alSpeedOfSound(_)),
];
