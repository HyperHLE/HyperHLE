use miniaudio::*;

pub struct OpenALContext {
    pub device: Device,
}

static mut CONTEXT: Option<OpenALContext> = None;

pub fn alcOpenDevice() -> u32 {
    1 // non-null handle
}

pub fn alcCreateContext() -> u32 {
    unsafe {
        let context = OpenALContext {
            device: Device::new(None, &DeviceConfig::default()).unwrap(),
        };

        CONTEXT = Some(context);
    }
    1
}

pub fn alcMakeContextCurrent() -> u32 {
    1
}

pub fn alcGetString(param: i32) -> *const u8 {
    match param {
        0x1005 => b"touchHLE OpenAL\0".as_ptr(), // ALC_DEVICE_SPECIFIER
        _ => b"\0".as_ptr(),
    }
}
