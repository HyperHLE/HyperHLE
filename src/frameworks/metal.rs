/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Minimal Metal.framework compatibility layer.
//!
//! Metal is a native Apple GPU API, while HyperHLE renders through its
//! portable GLES presentation path. This layer intentionally provides the
//! object and descriptor contract used during app startup and resource setup;
//! it does not pretend to execute Metal command streams on non-Apple hosts.

use crate::dyld::{ConstantExports, HostDylib};
use crate::frameworks::foundation::{ns_string, NSUInteger};
use crate::mem::{ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr};
use crate::objc::{id, msg, msg_class, nil, objc_classes, ClassExports, HostObject, NSZonePtr};
use crate::Environment;

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/Metal.framework/Metal",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[FUNCTIONS],
};

const MTL_RESOURCE_STORAGE_MODE_SHARED: NSUInteger = 0;
const MTL_RESOURCE_STORAGE_MODE_MANAGED: NSUInteger = 1;
const MTL_RESOURCE_STORAGE_MODE_PRIVATE: NSUInteger = 2;
const MTL_RESOURCE_STORAGE_MODE_MEMORYLESS: NSUInteger = 3;
const MTL_LOAD_ACTION_DONT_CARE: NSUInteger = 0;
const MTL_LOAD_ACTION_LOAD: NSUInteger = 1;
const MTL_LOAD_ACTION_CLEAR: NSUInteger = 2;
const MTL_STORE_ACTION_DONT_CARE: NSUInteger = 0;
const MTL_STORE_ACTION_STORE: NSUInteger = 1;

#[derive(Default)]
struct MetalObjectHostObject {
    label: id,
    device: id,
    length: NSUInteger,
    contents: MutVoidPtr,
    usage: NSUInteger,
    storage_mode: NSUInteger,
    pixel_format: NSUInteger,
    width: NSUInteger,
    height: NSUInteger,
    depth: NSUInteger,
    mipmap_level_count: NSUInteger,
    sample_count: NSUInteger,
    load_action: NSUInteger,
    store_action: NSUInteger,
    clear_color: [f64; 4],
    command_buffer: id,
}
impl HostObject for MetalObjectHostObject {}

fn metal_string(env: &mut Environment, value: &'static str) -> id {
    ns_string::get_static_str(env, value)
}

fn allocation_size(length: NSUInteger) -> GuestUSize {
    length.max(1)
}

fn constant_uinteger(env: &mut Environment, value: NSUInteger) -> ConstVoidPtr {
    ConstVoidPtr::from_bits(env.mem.alloc_and_write(value).to_bits())
}

const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);

@implementation MTLDevice: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::new(MetalObjectHostObject::default()), &mut env.mem)
}

- (id)init { this }
- (id)name { metal_string(env, "HyperHLE Metal compatibility device") }
- (bool)hasUnifiedMemory { true }
- (NSUInteger)recommendedMaxWorkingSetSize { 0 }
- (bool)supportsFamily:(NSUInteger)_family { false }
- (bool)supportsTextureSampleCount:(NSUInteger)count { count == 1 }
- (id)newCommandQueue { msg_class![env; MTLCommandQueue new] }
- (id)newCommandQueueWithMaxCommandBufferCount:(NSUInteger)_count { msg_class![env; MTLCommandQueue new] }
- (id)newBufferWithLength:(NSUInteger)length options:(NSUInteger)options {
    let object = msg_class![env; MTLBuffer alloc];
    let contents = env.mem.alloc(allocation_size(length));
    let host = env.objc.borrow_mut::<MetalObjectHostObject>(object);
    host.length = length;
    host.storage_mode = options & 0xf;
    host.contents = contents;
    object
}
- (id)newBufferWithBytes:(ConstVoidPtr)bytes length:(NSUInteger)length options:(NSUInteger)options {
    let object = msg![env; this newBufferWithLength:length options:options];
    if !bytes.is_null() && length != 0 {
        let source = env.mem.bytes_at(bytes.cast(), length).to_vec();
        let destination_ptr = env.objc.borrow::<MetalObjectHostObject>(object).contents;
        env.mem.bytes_at_mut(destination_ptr.cast(), length).copy_from_slice(&source);
    }
    object
}
- (id)newTextureWithDescriptor:(id)descriptor {
    let object = msg_class![env; MTLTexture alloc];
    let source = env.objc.borrow::<MetalObjectHostObject>(descriptor);
    let device = source.device;
    let pixel_format = source.pixel_format;
    let width = source.width;
    let height = source.height;
    let depth = source.depth;
    let mipmap_level_count = source.mipmap_level_count;
    let sample_count = source.sample_count;
    let host = env.objc.borrow_mut::<MetalObjectHostObject>(object);
    host.device = if device == nil { this } else { device };
    host.pixel_format = pixel_format;
    host.width = width;
    host.height = height;
    host.depth = depth;
    host.mipmap_level_count = mipmap_level_count;
    host.sample_count = sample_count;
    object
}
- (id)newSamplerStateWithDescriptor:(id)_descriptor { msg_class![env; MTLSamplerState new] }
- (id)newRenderPipelineStateWithDescriptor:(id)_descriptor error:(MutPtr<id>)_error { msg_class![env; MTLRenderPipelineState new] }
- (id)newDepthStencilStateWithDescriptor:(id)_descriptor { msg_class![env; MTLDepthStencilState new] }

@end

@implementation MTLCommandQueue: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::new(MetalObjectHostObject::default()), &mut env.mem)
}
- (id)commandBuffer { let buffer = msg_class![env; MTLCommandBuffer new]; env.objc.borrow_mut::<MetalObjectHostObject>(buffer).command_buffer = this; buffer }
- (id)commandBufferWithUnretainedReferences { msg![env; this commandBuffer] }
@end

@implementation MTLCommandBuffer: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::new(MetalObjectHostObject::default()), &mut env.mem)
}
- (id)renderCommandEncoderWithDescriptor:(id)_descriptor { msg_class![env; MTLRenderCommandEncoder new] }
- (())commit {}
- (())waitUntilCompleted {}
- (())presentDrawable:(id)_drawable {}
- (NSUInteger)status { 0 }
@end

@implementation MTLRenderCommandEncoder: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::new(MetalObjectHostObject::default()), &mut env.mem)
}
- (())endEncoding {}
- (())setRenderPipelineState:(id)_state {}
- (())setVertexBuffer:(id)_buffer offset:(NSUInteger)_offset atIndex:(NSUInteger)_index {}
- (())setFragmentBuffer:(id)_buffer offset:(NSUInteger)_offset atIndex:(NSUInteger)_index {}
- (())setVertexTexture:(id)_texture atIndex:(NSUInteger)_index {}
- (())setFragmentTexture:(id)_texture atIndex:(NSUInteger)_index {}
- (())drawPrimitives:(NSUInteger)_type vertexStart:(NSUInteger)_start vertexCount:(NSUInteger)_count {}
- (())drawIndexedPrimitives:(NSUInteger)_type indexCount:(NSUInteger)_count indexType:(NSUInteger)_index_type indexBuffer:(id)_buffer indexBufferOffset:(NSUInteger)_offset {}
@end

@implementation MTLRenderPassDescriptor: NSObject
+ (id)renderPassDescriptor { msg_class![env; MTLRenderPassDescriptor new] }
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::new(MetalObjectHostObject::default()), &mut env.mem)
}
- (id)colorAttachments { msg_class![env; MTLRenderPassColorAttachmentDescriptorArray new] }
- (id)depthAttachment { msg_class![env; MTLRenderPassDepthAttachmentDescriptor new] }
- (id)stencilAttachment { msg_class![env; MTLRenderPassStencilAttachmentDescriptor new] }
@end

@implementation MTLRenderPassColorAttachmentDescriptorArray: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::new(MetalObjectHostObject::default()), &mut env.mem)
}
- (id)objectAtIndexedSubscript:(NSUInteger)_index { msg_class![env; MTLRenderPassColorAttachmentDescriptor new] }
- (id)objectAtIndex:(NSUInteger)_index { msg![env; this objectAtIndexedSubscript:_index] }
@end

@implementation MTLRenderPassColorAttachmentDescriptor: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::new(MetalObjectHostObject::default()), &mut env.mem)
}
- (())setTexture:(id)texture { env.objc.borrow_mut::<MetalObjectHostObject>(this).device = texture }
- (id)texture { env.objc.borrow::<MetalObjectHostObject>(this).device }
- (())setLoadAction:(NSUInteger)action { env.objc.borrow_mut::<MetalObjectHostObject>(this).load_action = action }
- (NSUInteger)loadAction { env.objc.borrow::<MetalObjectHostObject>(this).load_action }
- (())setStoreAction:(NSUInteger)action { env.objc.borrow_mut::<MetalObjectHostObject>(this).store_action = action }
- (NSUInteger)storeAction { env.objc.borrow::<MetalObjectHostObject>(this).store_action }
- (())setClearColor:(f64)color { env.objc.borrow_mut::<MetalObjectHostObject>(this).clear_color[0] = color }
@end

@implementation MTLTexture: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::new(MetalObjectHostObject::default()), &mut env.mem)
}
- (id)device { env.objc.borrow::<MetalObjectHostObject>(this).device }
- (NSUInteger)width { env.objc.borrow::<MetalObjectHostObject>(this).width }
- (NSUInteger)height { env.objc.borrow::<MetalObjectHostObject>(this).height }
- (NSUInteger)depth { env.objc.borrow::<MetalObjectHostObject>(this).depth }
- (NSUInteger)mipmapLevelCount { env.objc.borrow::<MetalObjectHostObject>(this).mipmap_level_count }
- (NSUInteger)sampleCount { env.objc.borrow::<MetalObjectHostObject>(this).sample_count }
- (NSUInteger)pixelFormat { env.objc.borrow::<MetalObjectHostObject>(this).pixel_format }
- (())replaceRegion:(id)_region mipmapLevel:(NSUInteger)_level withBytes:(ConstVoidPtr)_bytes bytesPerRow:(NSUInteger)_row {}
@end

@implementation MTLBuffer: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::new(MetalObjectHostObject::default()), &mut env.mem)
}
- (id)device { env.objc.borrow::<MetalObjectHostObject>(this).device }
- (NSUInteger)length { env.objc.borrow::<MetalObjectHostObject>(this).length }
- (MutVoidPtr)contents { env.objc.borrow::<MetalObjectHostObject>(this).contents }
- (NSUInteger)storageMode { env.objc.borrow::<MetalObjectHostObject>(this).storage_mode }
@end

@implementation MTLRenderPipelineState: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone { env.objc.alloc_object(this, Box::new(MetalObjectHostObject::default()), &mut env.mem) }
- (id)device { env.objc.borrow::<MetalObjectHostObject>(this).device }
@end

@implementation MTLDepthStencilState: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone { env.objc.alloc_object(this, Box::new(MetalObjectHostObject::default()), &mut env.mem) }
@end

@implementation MTLSamplerState: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone { env.objc.alloc_object(this, Box::new(MetalObjectHostObject::default()), &mut env.mem) }
@end

};

fn MTLCreateSystemDefaultDevice(env: &mut Environment) -> id {
    msg_class![env; MTLDevice new]
}

pub const FUNCTIONS: crate::dyld::FunctionExports =
    &[crate::dyld::export_c_func!(MTLCreateSystemDefaultDevice())];

pub const CONSTANTS: ConstantExports = &[
    (
        "_MTLResourceStorageModeShared",
        crate::dyld::HostConstant::Custom(|env| {
            constant_uinteger(env, MTL_RESOURCE_STORAGE_MODE_SHARED)
        }),
    ),
    (
        "_MTLResourceStorageModeManaged",
        crate::dyld::HostConstant::Custom(|env| {
            constant_uinteger(env, MTL_RESOURCE_STORAGE_MODE_MANAGED)
        }),
    ),
    (
        "_MTLResourceStorageModePrivate",
        crate::dyld::HostConstant::Custom(|env| {
            constant_uinteger(env, MTL_RESOURCE_STORAGE_MODE_PRIVATE)
        }),
    ),
    (
        "_MTLResourceStorageModeMemoryless",
        crate::dyld::HostConstant::Custom(|env| {
            constant_uinteger(env, MTL_RESOURCE_STORAGE_MODE_MEMORYLESS)
        }),
    ),
    (
        "_MTLLoadActionDontCare",
        crate::dyld::HostConstant::Custom(|env| constant_uinteger(env, MTL_LOAD_ACTION_DONT_CARE)),
    ),
    (
        "_MTLLoadActionLoad",
        crate::dyld::HostConstant::Custom(|env| constant_uinteger(env, MTL_LOAD_ACTION_LOAD)),
    ),
    (
        "_MTLLoadActionClear",
        crate::dyld::HostConstant::Custom(|env| constant_uinteger(env, MTL_LOAD_ACTION_CLEAR)),
    ),
    (
        "_MTLStoreActionDontCare",
        crate::dyld::HostConstant::Custom(|env| constant_uinteger(env, MTL_STORE_ACTION_DONT_CARE)),
    ),
    (
        "_MTLStoreActionStore",
        crate::dyld::HostConstant::Custom(|env| constant_uinteger(env, MTL_STORE_ACTION_STORE)),
    ),
];
