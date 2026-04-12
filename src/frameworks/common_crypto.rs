/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! CommonCrypto

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstVoidPtr, GuestUSize, MutVoidPtr};
use crate::Environment;

// CCCryptorStatus
const kCCSuccess: i32 = 0;
const kCCBufferTooSmall: i32 = -4301;

// Вспомогательные функции для чтения и записи u32 (Little Endian)
fn read_u32_le(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}
fn write_u32_le(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
}

// Трансформация блока MD5
fn md5_step(state: &mut [u32; 4], data: &[u8; 64]) {
    let mut words = [0u32; 16];
    for i in 0..16 {
        words[i] = u32::from_le_bytes([
            data[i * 4], data[i * 4 + 1], data[i * 4 + 2], data[i * 4 + 3]
        ]);
    }
    let [mut a, mut b, mut c, mut d] = *state;

    let s = [
        7, 12, 17, 22,  7, 12, 17, 22,  7, 12, 17, 22,  7, 12, 17, 22,
        5,  9, 14, 20,  5,  9, 14, 20,  5,  9, 14, 20,  5,  9, 14, 20,
        4, 11, 16, 23,  4, 11, 16, 23,  4, 11, 16, 23,  4, 11, 16, 23,
        6, 10, 15, 21,  6, 10, 15, 21,  6, 10, 15, 21,  6, 10, 15, 21,
    ];
    let k = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee,
        0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
        0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be,
        0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
        0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa,
        0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
        0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
        0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
        0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05,
        0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039,
        0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1,
        0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
    ];

    for i in 0..64 {
        let (mut f, g) = match i {
            0..=15 => ((b & c) | (!b & d), i),
            16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
            48..=63 => (c ^ (b | !d), (7 * i) % 16),
            _ => unreachable!(),
        };
        f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(words[g]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(f.rotate_left(s[i]));
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

#[allow(non_snake_case)]
fn CC_MD5_Init(env: &mut Environment, c: MutVoidPtr) -> i32 {
    if c.is_null() { return 0; }
    let c_ptr = c.cast::<u8>();
    
    // CC_MD5_CTX занимает 92 байта в памяти
    let mut ctx = [0u8; 92];
    write_u32_le(&mut ctx, 0, 0x67452301);
    write_u32_le(&mut ctx, 4, 0xefcdab89);
    write_u32_le(&mut ctx, 8, 0x98badcfe);
    write_u32_le(&mut ctx, 12, 0x10325476);
    
    env.mem.bytes_at_mut(c_ptr, 92).copy_from_slice(&ctx);
    1
}

#[allow(non_snake_case)]
fn CC_MD5_Update(env: &mut Environment, c: MutVoidPtr, data: ConstVoidPtr, len: GuestUSize) -> i32 {
    if c.is_null() || data.is_null() || len == 0 { return 1; }
    let c_ptr = c.cast::<u8>();
    let data_ptr = data.cast::<u8>();

    let mut ctx = env.mem.bytes_at(c_ptr, 92).to_vec();
    let input = env.mem.bytes_at(data_ptr, len).to_vec();

    let mut state = [
        read_u32_le(&ctx, 0), read_u32_le(&ctx, 4),
        read_u32_le(&ctx, 8), read_u32_le(&ctx, 12)
    ];
    let mut nl = read_u32_le(&ctx, 16);
    let mut nh = read_u32_le(&ctx, 20);
    let mut num = read_u32_le(&ctx, 88) as usize;

    let bits = (len as u64) * 8;
    let nl_new = nl as u64 + bits;
    nl = nl_new as u32;
    nh = nh.wrapping_add((nl_new >> 32) as u32);

    let mut input_idx = 0;
    let input_len = len as usize;

    while input_idx < input_len {
        let space = 64 - num;
        let chunk = std::cmp::min(space, input_len - input_idx);
        ctx[24 + num .. 24 + num + chunk].copy_from_slice(&input[input_idx .. input_idx + chunk]);
        num += chunk;
        input_idx += chunk;

        if num == 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&ctx[24..88]);
            md5_step(&mut state, &block);
            num = 0;
        }
    }

    write_u32_le(&mut ctx, 0, state[0]);
    write_u32_le(&mut ctx, 4, state[1]);
    write_u32_le(&mut ctx, 8, state[2]);
    write_u32_le(&mut ctx, 12, state[3]);
    write_u32_le(&mut ctx, 16, nl);
    write_u32_le(&mut ctx, 20, nh);
    write_u32_le(&mut ctx, 88, num as u32);

    env.mem.bytes_at_mut(c_ptr, 92).copy_from_slice(&ctx);
    1
}

#[allow(non_snake_case)]
fn CC_MD5_Final(env: &mut Environment, md: MutVoidPtr, c: MutVoidPtr) -> i32 {
    if md.is_null() || c.is_null() { return 0; }
    let md_ptr = md.cast::<u8>();
    let c_ptr = c.cast::<u8>();

    let mut ctx = env.mem.bytes_at(c_ptr, 92).to_vec();
    let mut state = [
        read_u32_le(&ctx, 0), read_u32_le(&ctx, 4),
        read_u32_le(&ctx, 8), read_u32_le(&ctx, 12)
    ];
    let nl = read_u32_le(&ctx, 16);
    let nh = read_u32_le(&ctx, 20);
    let mut num = read_u32_le(&ctx, 88) as usize;

    ctx[24 + num] = 0x80;
    num += 1;

    if num > 56 {
        for i in num..64 { ctx[24 + i] = 0; }
        let mut block = [0u8; 64];
        block.copy_from_slice(&ctx[24..88]);
        md5_step(&mut state, &block);
        num = 0;
    }

    for i in num..56 { ctx[24 + i] = 0; }

    ctx[24 + 56 .. 24 + 60].copy_from_slice(&nl.to_le_bytes());
    ctx[24 + 60 .. 24 + 64].copy_from_slice(&nh.to_le_bytes());

    let mut block = [0u8; 64];
    block.copy_from_slice(&ctx[24..88]);
    md5_step(&mut state, &block);

    let mut hash = [0u8; 16];
    hash[0..4].copy_from_slice(&state[0].to_le_bytes());
    hash[4..8].copy_from_slice(&state[1].to_le_bytes());
    hash[8..12].copy_from_slice(&state[2].to_le_bytes());
    hash[12..16].copy_from_slice(&state[3].to_le_bytes());

    env.mem.bytes_at_mut(md_ptr, 16).copy_from_slice(&hash);
    env.mem.bytes_at_mut(c_ptr, 92).copy_from_slice(&[0u8; 92]);

    1
}

// Split CCCrypt into a wrapper that reads stack args manually.
// ARM ABI: R0-R3 = first 4 args, rest on stack.
// CCCrypt has 12 args total.
// We expose first 8 params as normal, then read remaining 4 from stack.
#[allow(non_snake_case)]
fn CCCrypt(
    env: &mut Environment,
    op: u32,
    alg: u32,
    options: u32,
    key: ConstVoidPtr,
    key_length: GuestUSize,
    _iv: ConstVoidPtr,
    data_in: ConstVoidPtr,
    data_in_length: GuestUSize,
) -> i32 {
    // Read remaining 4 args from guest stack
    let sp = env.cpu.regs()[13]; // SP
    let data_out = crate::mem::Ptr::from_bits(env.mem.read(crate::mem::Ptr::<u32, false>::from_bits(sp)));
    let data_out_available: u32 = env.mem.read(crate::mem::Ptr::<u32, false>::from_bits(sp + 4));
    let data_out_moved_ptr = crate::mem::Ptr::<u32, true>::from_bits(env.mem.read(crate::mem::Ptr::<u32, false>::from_bits(sp + 8)));

    log!(
        "CCCrypt(op={}, alg={}, options={:#x}, keyLen={}, dataLen={})",
        op, alg, options, key_length, data_in_length
    );

    if data_out_available < data_in_length {
        return kCCBufferTooSmall;
    }

    // RC4 stream cipher
    if alg == 4 {
        let input = env.mem.bytes_at(data_in.cast(), data_in_length).to_vec();
        let key_bytes = env.mem.bytes_at(key.cast(), key_length).to_vec();
        let mut output = vec![0u8; data_in_length as usize];

        let mut s: Vec<u8> = (0..=255u8).collect();
        let mut j: usize = 0;
        for i in 0..256usize {
            j = (j + s[i] as usize + key_bytes[i % key_length as usize] as usize) % 256;
            s.swap(i, j);
        }
        let mut i = 0usize;
        j = 0;
        for (idx, &byte) in input.iter().enumerate() {
            i = (i + 1) % 256;
            j = (j + s[i] as usize) % 256;
            s.swap(i, j);
            let k = s[(s[i] as usize + s[j] as usize) % 256];
            output[idx] = byte ^ k;
        }
        env.mem.bytes_at_mut(data_out, data_in_length).copy_from_slice(&output);
        env.mem.write(data_out_moved_ptr, data_in_length);
        return kCCSuccess;
    }

    // Other algorithms: copy as-is (TODO: implement AES/DES properly)
    let input = env.mem.bytes_at(data_in.cast(), data_in_length).to_vec();
    env.mem.bytes_at_mut(data_out, data_in_length).copy_from_slice(&input);
    env.mem.write(data_out_moved_ptr, data_in_length);
    log!("CCCrypt: alg={} not implemented, data copied as-is", alg);
    kCCSuccess
}

#[allow(non_snake_case)]
fn CCKeyDerivationPBKDF(
    _env: &mut Environment,
    _algorithm: u32,
    _password: ConstVoidPtr,
    _password_len: GuestUSize,
    _salt: ConstVoidPtr,
    _salt_len: GuestUSize,
    _prf: u32,
    _rounds: u32,
) -> i32 {
    log!("TODO: CCKeyDerivationPBKDF");
    kCCSuccess
}

#[allow(non_snake_case)]
fn CCHmac(
    _env: &mut Environment,
    _algorithm: u32,
    _key: ConstVoidPtr,
    _key_length: GuestUSize,
    _data: ConstVoidPtr,
    _data_length: GuestUSize,
    _mac_out: MutVoidPtr,
) {
    log!("TODO: CCHmac");
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CCCrypt(_, _, _, _, _, _, _, _)),
    export_c_func!(CCKeyDerivationPBKDF(_, _, _, _, _, _, _)), 
    export_c_func!(CCHmac(_, _, _, _, _, _)),
    // Исправленное количество аргументов (исключая env):
    export_c_func!(CC_MD5_Init(_)),           // Было (_, _), нужно (_)
    export_c_func!(CC_MD5_Update(_, _, _)),    // Было (_, _, _, _), нужно (_, _, _)
    export_c_func!(CC_MD5_Final(_, _)),       // Было (_, _, _), нужно (_, _)
];

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/usr/lib/libcommonCrypto.dylib",
    aliases: &[
        "/System/Library/Frameworks/Security.framework/Security",
        "/usr/lib/libCommonCrypto.dylib",
    ],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};
