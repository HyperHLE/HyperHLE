/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The core of the emulator: management of state, execution, threading.
//!
//! Unlike its siblings, this module should be considered private and only used
//! via the re-exports one level up.

pub mod app_picker;
mod mutex;
mod nullable_box;
mod undecodable;

use crate::abi::{CallFromHost, GuestFunction};
use crate::audio::openal::OpenALManager;
use crate::cpu::Cpu;
use crate::libc::semaphore::sem_t;
use crate::mem::{self, GuestUSize, MutPtr, MutVoidPtr, Ptr};
use crate::{
    abi, bundle, cpu, dyld, frameworks, fs, gdb, image, libc, mach_o, objc, options, stack, window,
};
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::TcpListener;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use crate::libc::pthread::cond::pthread_cond_t;
use crate::window::DeviceFamily;
use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, Yielder};
pub use mutex::{MutexId, MutexType, PTHREAD_MUTEX_DEFAULT};
use nullable_box::NullableBox;

/// Index into the [Vec] of threads. Thread 0 is always the main thread.
pub type ThreadId = usize;

pub type HostContext = Coroutine<Environment, Environment, Environment>;

/// Bookkeeping for a thread.
pub struct Thread {
    /// Once a thread finishes, this is set to false.
    pub active: bool,
    /// If this is not [ThreadBlock::NotBlocked], the thread is not executing
    /// until a certain condition is fufilled.
    pub blocked_by: ThreadBlock,
    /// Container for thread local state of various child modules
    pub thread_local_framework_state: frameworks::ThreadLocalState,
    /// The thread's return value, including an explicit main-thread pthread_exit.
    return_value: Option<MutVoidPtr>,
    /// Context object containing the CPU state for this thread.
    ///
    /// There should always be `(threads.len() - 1)` contexts in existence.
    /// When a thread is currently executing, its state is stored directly in
    /// the CPU, rather than in a context object. In that case, this field is
    /// None. See also: [std::mem::take] and [cpu::Cpu::swap_context].
    pub guest_context: Option<Box<cpu::CpuContext>>,
    /// The coroutine associated with this thread.
    ///
    /// In more typical rust, this is equivalent to to a [std::future::Future].
    /// Like a [std::future::Future], it holds the call stack so the inner
    /// function can (cooperatively) suspend execution and be resumed at a
    /// later time. Unlike a [std::future::Future], the call stack is actually
    /// stored as a stack, and not as an anonymous, compiler generated,
    /// (typically heap allocated) object.
    host_context: Option<HostContext>,
    /// Address range of this thread's stack, used to check if addresses are in
    /// range while producing a stack trace.
    pub stack: Option<std::ops::RangeInclusive<u32>>,
}

impl Thread {
    fn is_blocked(&self) -> bool {
        !matches!(self.blocked_by, ThreadBlock::NotBlocked)
    }
}

impl std::fmt::Debug for Thread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Thread {{ active: {:?}, blocked_by: {:?}, return_value: {:?} }}",
            self.active, self.blocked_by, self.return_value
        )
    }
}

/// Last guest PC seen by the CPU loop, for crash diagnostics.
pub static LAST_GUEST_PC: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Last guest LR seen by the CPU loop, for crash diagnostics.
pub static LAST_GUEST_LR: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Ring buffer of the last N guest PCs, for crash diagnostics.
pub static GUEST_PC_RING: [std::sync::atomic::AtomicU32; 32] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    [ZERO; 32]
};
pub static GUEST_PC_RING_IDX: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Guest r0-r7 as of the most recent host-function dispatch, for crash
/// diagnostics. r0-r3 are the AAPCS argument registers; r4-r7 are included
/// because a guest loop that repeatedly calls one host function usually keeps
/// the (nil) destination base in a callee-saved register rather than
/// reloading it into r0-r3 every iteration.
pub static LAST_HOST_CALL_REGS: [std::sync::atomic::AtomicU32; 8] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    [ZERO; 8]
};

/// Thread id as of the most recent host-function dispatch, for crash
/// diagnostics (the guest PC alone does not say which thread hit it).
pub static LAST_HOST_CALL_THREAD: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(u32::MAX);

/// Symbol name of the most recent host-function dispatch, stored as the
/// (pointer, length) of a `&'static str`, for crash diagnostics. Diagnostics
/// only: both halves always come from dyld's static symbol table (or a name
/// that dyld deliberately leaks), so the bytes stay mapped for the whole
/// process lifetime. Readers must still clamp the length and validate UTF-8,
/// since the two halves are not stored atomically together.
pub static LAST_HOST_CALL_SYMBOL_PTR: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
pub static LAST_HOST_CALL_SYMBOL_LEN: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// How a guest process ended, independently of the host process.
#[derive(Clone, Copy, Debug)]
pub enum GuestTermination {
    Exit(i32),
    Signal(i32),
    /// The desktop game window was closed; reopen the app picker.
    ReturnToPicker,
}

impl GuestTermination {
    /// Map the guest result to a CLI or SDL process status.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Exit(code) => code & 0xff,
            // Shell convention; do not send a host signal to the emulator.
            Self::Signal(signal) => 128 + signal,
            Self::ReturnToPicker => 0,
        }
    }
}

/// The struct containing the entire emulator state. Methods are provided for
/// execution and management of threads.
pub struct Environment {
    /// Reference point for various timing functions.
    pub startup_time: Instant,
    pub(crate) guest_clock: crate::guest_clock::GuestClock,
    pub bundle: NullableBox<bundle::Bundle>,
    pub fs: NullableBox<fs::Fs>,
    /// The window is only absent when running in headless mode.
    pub window: Option<Box<window::Window>>,
    pub openal_manager: NullableBox<OpenALManager>,
    pub mem: NullableBox<mem::Mem>,
    /// Loaded binaries. Index `0` is always the app binary, other entries are
    /// dynamic libraries.
    pub bins: Vec<mach_o::MachO>,
    pub objc: NullableBox<objc::ObjC>,
    pub dyld: NullableBox<dyld::Dyld>,
    pub cpu: NullableBox<cpu::Cpu>,
    pub current_thread: ThreadId,
    pub threads: Vec<Thread>,
    pub libc_state: NullableBox<libc::State>,
    pub framework_state: NullableBox<frameworks::State>,
    pub mutex_state: NullableBox<mutex::MutexState>,
    pub options: NullableBox<options::Options>,
    gdb_server: Option<Box<gdb::GdbServer>>,
    pub env_vars: HashMap<Vec<u8>, MutPtr<u8>>,
    /// Set to [true] when created using [Environment::new_without_app].
    pub dump_file: Option<std::fs::File>,
    pub is_app_picker: bool,
    yielder: *const Yielder<Environment, Environment>,
    // The amount of ticks to run for Some(value), or single-stepping for None.
    // Sadly, setting ticks to 1 does not step properly, so Option is required.
    remaining_ticks: Option<u64>,
    panic_cell: Rc<Cell<Option<Environment>>>,
    /// Tracks repeated guest CPU-trap bypasses. See `debug_cpu_error`.
    cpu_error_bypass_last: Option<(u32, u32)>,
    cpu_error_bypass_count: u32,
    /// Total guest CPU-trap bypasses, independent of the call site. Logging is
    /// keyed on this so cycling through many `(pc, lr)` pairs cannot flood
    /// the log with one "occurrence 1" line per pair. See `debug_cpu_error`.
    cpu_error_bypass_total: u32,
    /// Tracks consecutive guest CPU-trap bypasses that fake-return to the same
    /// LR, regardless of the faulting PC. This catches runaway loops where the
    /// faulting PC alternates between bogus addresses but the guest keeps
    /// bouncing back to one return site, e.g. through a nil/garbage function
    /// pointer. See `debug_cpu_error`.
    cpu_error_bypass_last_lr: Option<u32>,
    cpu_error_bypass_lr_count: u32,
    /// Thread and stack pointer of the most recent deliberate guest trap
    /// (`udf`/`trap`/`bkpt`) that was bypassed.
    last_deliberate_trap_bypass: Option<(ThreadId, u32)>,
    /// Per-site counters for instructions dynarmic could not decode inside a
    /// code section and that were skipped as no-ops (keyed on the fault PC).
    /// Only used to rate-limit logging. See `debug_cpu_error`.
    cpu_skipped_instruction_sites: HashMap<u32, u32>,
    /// Guest process result, consumed at the return-to-host boundary without
    /// terminating the host process inside a linked libc function.
    guest_termination_requested: Option<GuestTermination>,
    /// A required Unity player archive could not be resolved or read from the
    /// mounted bundle. Retained to report the first missing archive once.
    missing_unity_player_archive: Option<String>,
    /// A linked host function deliberately redirected the guest PC. This skips
    /// the normal post-SVC return, which would overwrite the new continuation
    /// for compact four-byte stubs.
    guest_control_flow_redirected: bool,
    /// Synthetic guest frames installed by `GuestFunction::call_from_host`.
    /// They are host-call boundaries, not safe recovery targets.
    host_to_guest_stack_frames: Vec<(usize, u32)>,
    trainer: crate::trainer::Trainer,
}

/// What to do next when executing this thread.
enum ThreadNextAction {
    /// Continue CPU emulation.
    Continue,
    /// Return to host.
    ReturnToHost,
    /// Debug the current CPU error.
    DebugCpuError(cpu::CpuError),
}

/// What kind of instruction raised a guest UndefinedInstruction/Breakpoint.
/// See [Environment::classify_guest_trap].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuestTrapKind {
    /// `udf`/`trap`/`bkpt`: the compiler or the app placed this on purpose
    /// (`__builtin_trap()`, assertion failure, unreachable `switch` arm, …).
    DeliberateTrap { encoding: u32 },
    /// Some other encoding inside a code section that dynarmic refused to
    /// decode (unsupported or unimplemented instruction). Executing the
    /// surrounding code is still meaningful, so the right thing to do is to
    /// step over it, not to abandon the function.
    UndecodableInstruction { encoding: u32 },
    /// The PC is inside a code section, but the bytes there are not an
    /// instruction stream: a literal pool, switch table or other data blob
    /// inside `__text`, reached through a bad function pointer or a computed
    /// branch that went wrong. Nothing sensible can be executed from here, and
    /// stepping over the bytes would only walk the guest deeper into the blob,
    /// so this is recovered like [GuestTrapKind::OutsideCode]. See
    /// `undecodable::undecodable_site_is_likely_code`.
    ExecutionInData { encoding: u32 },
    /// The PC is in a code section but points at the *second* halfword of a
    /// 32-bit `bl`/`blx <label>` (the halfword before it is a valid first
    /// half and the "instruction" at PC is a valid second half). Execution
    /// is misaligned with the real instruction stream, e.g. after a jump
    /// table was indexed with a value the game never expected. Nothing
    /// sensible can be executed from here, so this is treated like a trap.
    MisalignedInstruction { encoding: u32 },
    /// The PC is not inside any code section (wild jump through a bad
    /// function pointer, execution ran off the end of a function into data,
    /// …) or the instruction bytes could not be read.
    OutsideCode,
}

/// A validated ARM frame record. See
/// [Environment::validated_guest_frame_record].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GuestFrameRecord {
    /// Address of the record (the frame pointer, r7).
    fp: u32,
    /// `[fp]`: the caller's frame pointer.
    saved_fp: u32,
    /// `[fp + 4]`: the return address (with Thumb bit) into the caller.
    saved_lr: u32,
    /// `fp + 8`: the caller's stack pointer after the return.
    caller_sp: u32,
}

/// How to get the guest past a trap instruction. See
/// [Environment::plan_guest_trap_recovery].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuestTrapRecovery {
    /// Branch to LR, leaving SP and the frame pointer alone. This is the
    /// historical behaviour; it is only right when the trapping function has
    /// not pushed a frame of its own.
    ReturnToLr,
    /// Pop the frame record at r7: restore r7 and SP, and return to the saved
    /// LR. Equivalent to the trapping function (or, if LR is stale, the
    /// function that owns the record) returning 0.
    UnwindFrame {
        frame: GuestFrameRecord,
        reason: &'static str,
    },
    /// Pop the return address of the frame that trapped off the guest stack
    /// (the word at SP after a normal `push {..., lr}` prologue) and continue
    /// in its caller. Used when the frame-pointer chain is unusable but the
    /// stack still holds a plausible call return address.
    UnwindStack {
        return_addr: u32,
        caller_sp: u32,
        reason: &'static str,
    },
    /// Step over the faulting instruction and continue in the same function.
    SkipInstruction { reason: &'static str },
}

/// If/what a thread is blocked by.
#[derive(Debug, Clone, PartialEq)]
pub enum ThreadBlock {
    // Default state. (thread is not blocked)
    NotBlocked,
    // Thread is sleeping. (until Instant)
    Sleeping(Instant),
    // Guest deadline, rescaled dynamically by the game clock.
    GuestSleeping(Instant),
    // Thread is waiting for a mutex to unlock.
    Mutex(MutexId),
    // Thread is waiting on a semaphore.
    Semaphore(MutPtr<sem_t>),
    // Thread is waiting on a condition variable
    Condition(MutPtr<pthread_cond_t>, Option<Duration>),
    // Thread is waiting for another thread to finish (joining).
    Joining(ThreadId, MutPtr<MutVoidPtr>),
    // Thread has hit a cpu error, and is waiting to be debugged.
    WaitingForDebugger(Option<cpu::CpuError>),
    // Thread is suspended. We keep a suspend count and a previous thread state
    // (boxed to avoid cyclic dependency), which would be restored upon
    // resuming.
    #[allow(dead_code)]
    Suspended(usize, Box<ThreadBlock>),
    // Thread is waiting for a KeyMgr process-wide pointer lock.
    KeymgrLock(i32),
}

struct BinaryDependencyNode {
    name: String,
    dependencies: Vec<String>,
}

fn canonicalize_dylib_path(path: &str) -> String {
    let (directory, name) = path.rsplit_once('/').unwrap_or(("", path));
    let canonical_name = match name {
        "libstdc++.6.dylib" => "libstdc++.6.0.9.dylib",
        "libz.1.dylib" | "libz.dylib" | "libz.1.1.3.dylib" => "libz.1.2.3.dylib",
        "libsqlite3.0.dylib" => "libsqlite3.dylib",
        _ => name,
    };
    if directory.is_empty() {
        canonical_name.to_owned()
    } else {
        format!("{directory}/{canonical_name}")
    }
}

fn load_transitive_dependencies<T>(
    roots: &[String],
    mut load: impl FnMut(&str) -> Result<Option<(T, Vec<String>)>, String>,
) -> Result<Vec<T>, String> {
    let mut pending: VecDeque<String> = roots.iter().cloned().collect();
    let mut visited: HashSet<String> = HashSet::new();
    let mut loaded = Vec::new();
    while let Some(path) = pending.pop_front() {
        if !visited.insert(canonicalize_dylib_path(&path)) {
            continue;
        }
        if let Some((binary, dependencies)) = load(&path)? {
            pending.extend(dependencies);
            loaded.push(binary);
        }
    }
    Ok(loaded)
}

/// Topologically sorts the binary dylibs using Kahn's algorithm
/// and returns the sorted list of indices
fn generate_binary_load_order(graph: &[BinaryDependencyNode]) -> Result<Vec<usize>, String> {
    let node_to_index: HashMap<_, _> = graph
        .iter()
        .enumerate()
        .map(|(idx, node)| (node.name.as_str(), idx))
        .collect();
    let mut node_dependents = HashMap::new();
    let mut node_in_degrees: HashMap<_, _> = node_to_index.values().map(|&idx| (idx, 0)).collect();

    for node in graph {
        let &bin_index = node_to_index
            .get(node.name.as_str())
            .ok_or_else(|| format!("Failed to find {:?} name mapping", &node.name))?;

        // Bin names dont include prefix while dynamic lib paths do
        for dependency in node
            .dependencies
            .iter()
            .map(|path| path.strip_prefix("/usr/lib/").unwrap_or(path.as_str()))
        {
            // Ignore dependencies that are not included in packaged dylibs
            let Some(&dylib_index) = node_to_index.get(dependency) else {
                continue;
            };

            node_dependents
                .entry(dylib_index)
                .or_insert_with(Vec::new)
                .push(bin_index);

            node_in_degrees
                .entry(bin_index)
                .and_modify(|in_degree| *in_degree += 1);
        }
    }

    let mut leaf_nodes: VecDeque<_> = node_in_degrees
        .iter()
        .filter(|(_, &in_degree)| in_degree == 0)
        .map(|(&node, _)| node)
        .collect();

    let mut sorted_indices = Vec::new();

    while let Some(node) = leaf_nodes.pop_front() {
        sorted_indices.push(node);

        let Some(dependents) = node_dependents.get(&node) else {
            continue;
        };

        for &dependant in dependents {
            let Some(in_degree) = node_in_degrees.get_mut(&dependant) else {
                continue;
            };
            *in_degree -= 1;

            if *in_degree == 0 {
                leaf_nodes.push_back(dependant);
            }
        }
    }

    if let Some((&index, _)) = node_in_degrees.iter().find(|(_, &in_degree)| in_degree > 0) {
        return Err(format!(
            "Failed to sort nodes, cycle with {:?}",
            graph.get(index).unwrap().name
        ));
    }

    log!(
        "Found sorted order {:?}",
        sorted_indices
            .iter()
            .map(|&index| graph.get(index).unwrap().name.as_str())
            .collect::<Vec<_>>()
    );

    Ok(sorted_indices)
}

/// Enforces the one (real) Environment limit. See
/// [Environment::with_yielder] for why this is needed.
static ENVIRONMENT_INSTANCE_EXISTS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

impl Environment {
    /// Loads the binary and sets up the emulator.
    pub fn new(
        bundle: bundle::Bundle,
        fs: fs::Fs,
        mut options: options::Options,
        app_args: Vec<String>,
    ) -> Result<Environment, String> {
        let startup_time = Instant::now();
        let launched_bundle_id = bundle.bundle_identifier().to_owned();

        if launched_bundle_id == "at.source.potato.full" {
            log!(
        "Applying PotatoGold compatibility profile: disable present rotation, remap touch location to landscape, fake network success, and use silent OpenAL fallback."
    );

            // SAFETY: Environment::new runs during startup before guest worker threads
            // are created. These env vars are read by compatibility shims inside this
            // same process.
            unsafe {
                std::env::set_var("TOUCHHLE_DISABLE_PRESENT_ROTATION", "1");
                std::env::set_var("TOUCHHLE_TOUCH_LOCATION_PORTRAIT_TO_LANDSCAPE", "1");
                std::env::set_var("TOUCHHLE_FAKE_NETWORK_SUCCESS", "1");

                // PotatoGold's audio path was crashing on some Linux setups unless
                // OpenAL Soft used the null backend. This keeps the app playable even
                // if sound is silent.
            }
        }
        // A compatibility profile above may have changed the environment after
        // the caches were last refreshed (the app picker also runs the same
        // code), so make the cached `TOUCHHLE_*` flags re-read it.
        crate::env_flags::invalidate_caches();
        // Enforces the one (real) Environment limit. See `with_yielder` for
        // why this is needed.
        if ENVIRONMENT_INSTANCE_EXISTS.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Err("Only one (real) Environment can exist at a time!".to_string());
        }

        // Certain apps need to launch in a non-portrait orientation, and this
        // should be handled before creating the window because handling of
        // window rotation after-the-fact is somewhat glitchy.
        // This also ensures the splash screen is correctly oriented.
        //
        // Only force a non-portrait orientation when the app explicitly
        // does NOT advertise portrait support. Storyboard apps (and any
        // other modern UIKit binary) routinely declare every orientation
        // they can run in via `UISupportedInterfaceOrientations`, and
        // picking the first non-portrait entry would force them into
        // landscape even when portrait is perfectly fine. Apple's own
        // launch logic uses portrait by default whenever it's listed, so
        // mirror that.
        let portrait_supported = bundle
            .supported_interface_orientations()
            .contains(&"UIInterfaceOrientationPortrait");
        if options.initial_orientation == window::DeviceOrientation::Portrait && !portrait_supported
        {
            if let Some(&non_portrait_orientation) = bundle
                .supported_interface_orientations()
                .iter()
                .find(|&&o| o != "UIInterfaceOrientationPortrait")
            {
                // TODO: Overwriting the options might not be ideal; do we need
                //       to distinguish this kind of orientation change from
                //       others?
                options.initial_orientation = match non_portrait_orientation {
                    // UIInterfaceOrientation values are flipped relative to
                    // (UI)DeviceOrientation values (content has to rotate in
                    // the opposite direction to how the device rotates).
                    "UIInterfaceOrientationLandscapeLeft" => {
                        window::DeviceOrientation::LandscapeRight
                    }
                    "UIInterfaceOrientationLandscapeRight" => {
                        window::DeviceOrientation::LandscapeLeft
                    }
                    // This appears to be an older way set the orientation.
                    // From testing, it seems to correspond to left.
                    "UIInterfaceOrientationLandscape" => window::DeviceOrientation::LandscapeLeft,

                    // ДОБАВЛЯЕМ СЮДА ПРИВЯЗКУ К ОБЫЧНОМУ ПОРТРЕТУ:
                    "UIInterfaceOrientationPortraitUpsideDown" => {
                        window::DeviceOrientation::Portrait
                    }

                    other => {
                        log!(
                            "Warning: Unsupported startup orientation: {:?}; defaulting to Portrait.",
                            other
                        );
                        window::DeviceOrientation::Portrait
                    }
                };
                log!("App needs non-portrait user interface orientation {:?}, applying device orientation {:?}.", non_portrait_orientation, options.initial_orientation);
            }
        }

        let device_family_override = options.device_family;
        // `--device-family=auto`: when the user hasn't pinned a specific family,
        // probe the host display and pick the closest-matching emulated device.
        // This is treated exactly like an explicit override below, so it still
        // respects what the app bundle actually supports.
        let device_family_override = if device_family_override.is_none()
            && options.auto_device_family
            && !options.headless
        {
            match window::host_screen_size() {
                Some((w, h)) => {
                    let picked = DeviceFamily::pick_for_screen(w, h);
                    if options.host_screen_size.is_none() {
                        options.host_screen_size = Some((w, h));
                    }
                    log!(
                        "Auto device family: host screen is {}x{} px, exposing the same resolution to the app and picking closest match {:?}.",
                        w,
                        h,
                        picked
                    );
                    Some(picked)
                }
                None => {
                    log!("Auto device family: couldn't determine host screen size; leaving choice to the app bundle.");
                    None
                }
            }
        } else {
            device_family_override
        };
        let device_family_array = bundle.device_family_array();
        // The bundle only declares generic device *classes* (iPhone == phone
        // family, iPad == tablet family). A user override may now name a
        // specific model (e.g. iPhone 4s, iPad mini 2). We accept the override
        // when its class matches one the bundle supports, and otherwise fall
        // back to a sensible default model for a supported class.
        let bundle_supports_ipad = device_family_array.iter().any(|f| f.is_ipad());
        let bundle_supports_phone = device_family_array.iter().any(|f| !f.is_ipad());
        // Default model picked for each class when the user hasn't chosen one.
        // iPhone 3GS (iPhone2,1) is the historical touchHLE phone default
        // (320x480, GLES2-capable); iPad 2 (iPad2,1) is the tablet default.
        let default_phone = DeviceFamily::iPhone3GS;
        let default_ipad = DeviceFamily::iPad2;

        let device_family = if let Some(dfo) = device_family_override {
            let override_is_ipad = dfo.is_ipad();
            if override_is_ipad && bundle_supports_ipad {
                dfo
            } else if !override_is_ipad && bundle_supports_phone {
                dfo
            } else {
                log!(
                    "Warning: User-defined {:?} device family override is not supported by the app (supported: {:?}); ignoring.",
                    dfo,
                    device_family_array
                );
                if bundle_supports_phone {
                    default_phone
                } else if bundle_supports_ipad {
                    default_ipad
                } else {
                    default_phone
                }
            }
        } else if bundle_supports_phone {
            // Prefer the phone family when the bundle supports it, matching the
            // previous behaviour for universal (iPhone + iPad) bundles.
            default_phone
        } else if bundle_supports_ipad {
            default_ipad
        } else {
            log!(
                "Warning: bundle declares no recognised supported device families ({:?}); falling back to iPhone.",
                device_family_array
            );
            default_phone
        };
        log!("{:?} device family is chosen.", device_family);
        options.device_family = Some(device_family);

        // Read the executable before constructing the Mach-O image below;
        // this lets us reuse the bytes instead of reading the file twice.
        let executable_path = bundle.executable_path();
        let executable_name = executable_path.file_name().unwrap().to_string();
        let executable_bytes = fs
            .read(executable_path)
            .map_err(|_| "Could not load executable: Could not read executable file".to_string())?;

        let window = if options.headless {
            None
        } else {
            let icon = bundle.load_icon(&fs);
            if let Err(ref e) = icon {
                log!("Warning: {}", e);
            }

            // `launch_image_path` already resolved the spelling of the file
            // that exists in the bundle, so no extra is_file() check here:
            // doing an exact-match check on the plist's spelling instead was
            // what made the picker log "Using launch image" for a file that
            // then silently wasn't shown.
            let launch_image = bundle
                .launch_image_path(&fs, device_family)
                .and_then(|path| {
                    let res = fs
                        .read(path)
                        .map_err(|_| "Could not read launch image file".to_string())
                        .and_then(|bytes| {
                            image::Image::from_bytes(&bytes)
                                .map_err(|e| format!("Could not parse launch image: {e}"))
                        });
                    if let Err(ref e) = res {
                        log!("Warning: {}", e);
                    };
                    res.ok()
                });
            Some(Box::new(window::Window::new(
                &format!(
                    "{} ({} {}{}{})",
                    bundle.display_name(),
                    super::DISPLAY_NAME,
                    super::branding(),
                    if super::branding().is_empty() {
                        ""
                    } else {
                        " "
                    },
                    super::VERSION
                ),
                icon.ok(),
                launch_image.map(|image| (image, false)),
                &options,
            )?))
        };

        let mut mem = mem::Mem::new();

        let is_spore = bundle.bundle_identifier().starts_with("com.ea.spore");
        let is_critter_crunch = bundle
            .bundle_identifier()
            .starts_with("com.capybaragames.CritterCrunch")
            || bundle
                .bundle_identifier()
                .starts_with("com.go.starwave.CritterCrunch");
        let is_geometry_dash = bundle
            .bundle_identifier()
            .starts_with("com.robtop.geometryjump");
        let is_wheres_my_water = bundle.bundle_identifier() == "com.disney.SwampyGame";
        let is_gravity_guy = bundle.bundle_identifier() == "com.miniclip.gravityguy";
        // We always reset this flag depending on which game is launched.
        mem.zero_memory_on_free = !is_spore
            && !is_critter_crunch
            && !is_geometry_dash
            && !is_wheres_my_water
            && !is_gravity_guy;
        if is_spore {
            log!("Applying game-specific hack for Spore Origins: zeroing memory on alloc instead of free.");
        }
        if is_critter_crunch {
            // Without this hack, every time a critter 'explodes',
            // the game crashes with a null page access error.
            log!("Applying game-specific hack for Critter Crunch: zeroing memory on alloc instead of free.");
        }
        if is_geometry_dash {
            // Geometry Dash plays its music as FMOD streams fed from a memory
            // buffer that the game frees as soon as createStream() has
            // returned. Scrubbing freed memory (the default) therefore wipes
            // the song out from under FMOD's streamer, which then decodes
            // silence and the game retries loading the track, while one-shot
            // samples — decoded before the buffer is freed — keep working.
            // That is exactly the reported symptom: sound effects are audible,
            // music never is. Real malloc leaves freed bytes in place until the
            // chunk is reused, so zero on alloc instead, as for Spore Origins
            // and Critter Crunch above.
            log!("Applying game-specific hack for Geometry Dash: zeroing memory on alloc instead of free.");
        }
        if is_wheres_my_water {
            // When a finger is lifted, Where's My Water's
            // Widget_FingerCatcher::lostFingerHandled() walks a std::list
            // through a node that has already been freed. With freed memory
            // scrubbed, the node's `next` pointer reads as NULL and the walk
            // loops forever on the null page, freezing the game on the first
            // dig. Real malloc leaves the stale link in place.
            log!("Applying game-specific hack for Where's My Water: zeroing memory on alloc instead of free.");
        }
        if is_gravity_guy {
            // Gravity Guy reads memory it has already freed while the first
            // level starts. With freed memory scrubbed, the camera usually
            // races tens of thousands of points ahead within a few frames, so
            // the player is never on screen and the tutorial never appears.
            // Real malloc leaves the freed bytes in place. (This makes a good
            // start much more likely but not certain yet.)
            log!("Applying game-specific hack for Gravity Guy: zeroing memory on alloc instead of free.");
        }
        let executable = mach_o::MachO::load_from_bytes(
            &executable_bytes,
            &mut mem,
            executable_name,
            /* slide: */ 0,
        )
        .map_err(|e| format!("Could not load executable: {e}"))?;
        drop(executable_bytes);

        let dylibs = load_transitive_dependencies(&executable.dynamic_libraries, |dylib| {
            let dylib_path = fs::GuestPath::new(dylib);
            if fs.is_file(dylib_path) {
                assert!(dylib_path.as_str().starts_with("/usr/lib/"));
                let name = dylib_path.file_name().unwrap();
                let dylib_slide = match name {
                    "libstdc++.6.dylib" | "libstdc++.6.0.9.dylib" => 0x3748a000,
                    "libc++.1.dylib" => 0x38000000,
                    "libc++abi.dylib" => 0x38100000,
                    "libiconv.2.dylib" => 0x32000000,
                    "libgcc_s.1.dylib" => 0x30000000,
                    "libz.1.dylib" | "libz.1.2.3.dylib" | "libz.dylib" | "libz.1.1.3.dylib" => 0,
                    "libsqlite3.dylib" | "libsqlite3.0.dylib" => 0,
                    _ => {
                        log!(
                            "Warning: unknown binary slide for {:?}; loading at slide 0. App may fail to bind some symbols.",
                            name
                        );
                        0
                    }
                };
                let binary = mach_o::MachO::load_from_file(
                    fs::GuestPath::new(dylib),
                    &fs,
                    &mut mem,
                    dylib_slide,
                )
                .map_err(|e| format!("Could not load bundled dylib: {e}"))?;
                let dependencies = binary.dynamic_libraries.clone();
                Ok(Some((binary, dependencies)))
            } else {
                let implemented_in_host = crate::dyld::DYLIB_LIST
                    .iter()
                    .any(|d| d.path == dylib || d.aliases.contains(&dylib));
                let is_swift = dylib.rsplit('/').next().unwrap_or(dylib).starts_with("libswift");
                if !implemented_in_host && !is_swift {
                    log!(
                        "Warning: app binary depends on unimplemented or missing dylib \"{}\"",
                        dylib
                    );
                }
                Ok(None)
            }
        })?;

        let entry_point_addr = executable
            .entry_point_pc
            .ok_or_else(|| {
                "Mach-O file does not specify an entry point PC, perhaps it is not an executable?"
                    .to_string()
            })
            .unwrap();

        let entry_point_is_lc_main = executable.entry_point_is_lc_main;

        let entry_point_addr = abi::GuestFunction::from_addr_with_thumb_bit(entry_point_addr);

        log_dbg!("Address of start function: {:?}", entry_point_addr);

        let mut bins = dylibs;
        bins.insert(0, executable);

        let mut objc = objc::ObjC::new();

        let mut dyld = dyld::Dyld::new();
        dyld.do_initial_linking(&bundle, &bins, &mut mem, &mut objc);

        let cpu = cpu::Cpu::new(match options.direct_memory_access {
            true => Some(&mut mem),
            false => None,
        });

        // XaView BypassStackOverflow: guest code runs on this coroutine stack.
        // The 1MB corosensei default is too small for deeply-nested guest -> host
        // -> JNI calls on Android (ART's CheckJNI aborts with a pending
        // StackOverflowError -> SIGABRT). Give it the same 16MB as SDLThread.
        let main_thread_init_stack = DefaultStack::new(16 * 1024 * 1024)
            .expect("failed to allocate main guest coroutine stack");
        let main_thread_init_routine = Coroutine::with_stack(
            main_thread_init_stack,
            move |yielder, mut env: Environment| {
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    env.with_yielder(yielder, move |env| {
                        echo!("CPU emulation begins now.");
                        // Some apps use the stack inside the static initializer.
                        // While properly behaving apps should be fine, some app
                        // will try to poke the top of the stack, so we'll give
                        // it some room.
                        env.cpu.regs_mut()[Cpu::SP] = 0xFFFFF000;

                        // Call `+load` method on classes where it's defined.
                        // TODO: `+load` methods from our image should take priority
                        // over frameworks ones.
                        // TODO: a category `+load` method should be called after
                        // the class's own +load method.
                        // Note: `+load` is sent without triggering `+initialize`,
                        // matching the runtime's guarantee that `+load` runs first.
                        let mut to_be_loaded = Vec::new();
                        let mut processed = HashSet::new();
                        let load_sel: objc::SEL = env
                            .objc
                            .register_host_selector("load".to_string(), &mut env.mem);
                        for (class_name, &class) in env.objc.all_classes() {
                            if processed.contains(&class) {
                                continue;
                            }
                            if env.objc.is_unimplemented_class(class)
                                || env.objc.is_fake_class(class)
                            {
                                continue;
                            }
                            if env
                                .objc
                                .object_has_uninherited_method(&env.mem, class, load_sel)
                            {
                                log_dbg!(
                                    "Calling +load on inheritance chain of {} class",
                                    class_name
                                );
                                let mut inherited = Vec::new();
                                let mut curr_class = class;
                                while curr_class != objc::nil
                                    && !env.objc.is_unimplemented_class(curr_class)
                                    && !env.objc.is_fake_class(curr_class)
                                {
                                    if !processed.contains(&curr_class)
                                        && env.objc.object_has_uninherited_method(
                                            &env.mem, curr_class, load_sel,
                                        )
                                    {
                                        inherited.push(curr_class);
                                        processed.insert(curr_class);
                                    }
                                    curr_class = env.objc.get_superclass(curr_class);
                                }
                                to_be_loaded.extend(inherited.into_iter().rev());
                            }
                        }
                        for &class in &to_be_loaded {
                            () = objc::msg_send_no_initialize(env, (class, load_sel));
                        }

                        // Static initializers for libraries must be run before
                        // the initializer in the app binary.
                        for bin_idx in env.get_sorted_bin_indices().unwrap() {
                            let Some(bin) = env.bins.get(bin_idx) else {
                                continue;
                            };
                            let Some(section) =
                                bin.get_section(mach_o::SectionType::ModInitFuncPointers)
                            else {
                                continue;
                            };

                            log_dbg!("Calling static initializers for {:?}", bin.name);
                            assert!(section.size % 4 == 0);

                            let base: mem::ConstPtr<abi::GuestFunction> =
                                mem::Ptr::from_bits(section.addr);

                            let count = section.size / 4;
                            for i in 0..count {
                                let func = env.mem.read(base + i);

                                log_dbg!(
                                    "Calling static initializer at {:?} from {:?}",
                                    func,
                                    (base + i)
                                );

                                () = func.call_from_host(env, ());
                            }
                            log_dbg!("Static initialization done");
                        }

                        {
                            let bin_path = env.bundle.executable_path();

                            let envp_list: Vec<String> = env
                                .env_vars
                                .clone()
                                .iter_mut()
                                .map(|tuple| {
                                    [
                                        std::str::from_utf8(tuple.0).unwrap(),
                                        "=",
                                        env.mem.cstr_at_utf8(*tuple.1).unwrap(),
                                    ]
                                    .concat()
                                })
                                .collect();

                            let envp_ref_list: Vec<&str> =
                                envp_list.iter().map(|keyvalue| keyvalue.as_str()).collect();

                            let bin_path_apple_key =
                                format!("executable_path={}", bin_path.as_str());

                            let argv = Vec::from_iter(
                                std::iter::once(bin_path.as_str())
                                    .chain(app_args.iter().map(|s| s.as_str())),
                            );

                            let envp = envp_ref_list.as_slice();
                            let apple = &[bin_path_apple_key.as_str()];
                            stack::prep_stack_for_start(
                                &mut env.mem,
                                &mut env.cpu,
                                &argv,
                                envp,
                                apple,
                                entry_point_is_lc_main,
                            );
                        }

                        // Manually call here, since running call_from_host pushes
                        // a stack frame and disrupts abi for _start.
                        env.cpu
                            .branch_with_link(entry_point_addr, env.dyld.thread_exit_routine());

                        env.run_call();

                        let exited_via_pthread = env.threads[0].return_value.is_some();
                        if exited_via_pthread {
                            env.exit_if_last_thread();
                        } else if entry_point_is_lc_main && !env.is_guest_termination_requested() {
                            // LC_MAIN enters main directly: dyld normally
                            // passes its return value to exit(), including
                            // the registered atexit handlers.
                            let code = env.cpu.regs()[0] as i32;
                            crate::libc::stdlib::exit_session(env, code);
                        }

                        if env.guest_termination_requested.is_some() {
                            echo!(
                                "Guest requested controlled termination; returning to the host."
                            );
                        } else if !exited_via_pthread {
                            panic!("Main function exited unexpectedly!");
                        }
                    })
                }));

                if let Err(e) = res {
                    let panic_cell = env.panic_cell.clone();
                    panic_cell.set(Some(env));
                    std::panic::resume_unwind(e);
                }
                env
            },
        );

        let main_thread = Thread {
            active: true,
            blocked_by: ThreadBlock::NotBlocked,
            return_value: None,
            guest_context: None,
            host_context: Some(main_thread_init_routine),
            stack: Some(mem::Mem::MAIN_THREAD_STACK_LOW_END..=mem::Mem::MAIN_THREAD_STACK_HIGH_END),
            thread_local_framework_state: Default::default(),
        };

        let mut env = Environment {
            startup_time,
            guest_clock: crate::guest_clock::GuestClock::new(),
            bundle: NullableBox::new(bundle),
            fs: NullableBox::new(fs),
            window,
            openal_manager: NullableBox::new(OpenALManager::new()?),
            mem: NullableBox::new(mem),
            bins,
            objc: NullableBox::new(objc),
            dyld: NullableBox::new(dyld),
            cpu: NullableBox::new(cpu),
            current_thread: 0,
            threads: vec![main_thread],
            libc_state: Default::default(),
            mutex_state: Default::default(),
            framework_state: Default::default(),
            options: NullableBox::new(options),
            gdb_server: None,
            env_vars: Default::default(),
            dump_file: None,
            is_app_picker: false,
            yielder: std::ptr::null(),
            remaining_ticks: None,
            panic_cell: Rc::new(Cell::new(None)),
            cpu_error_bypass_last: None,
            cpu_error_bypass_count: 0,
            cpu_error_bypass_total: 0,
            cpu_error_bypass_last_lr: None,
            cpu_error_bypass_lr_count: 0,
            last_deliberate_trap_bypass: None,
            cpu_skipped_instruction_sites: HashMap::new(),
            guest_termination_requested: None,
            missing_unity_player_archive: None,
            guest_control_flow_redirected: false,
            host_to_guest_stack_frames: Vec::new(),
            trainer: crate::trainer::Trainer::new(false),
        };

        env.trainer = crate::trainer::Trainer::new(!env.options.trainer_disabled);
        env.set_tpidruro_for_current_thread();

        if env.options.dumping_options.any() {
            env.dump_file =
                Some(std::fs::File::create(&env.options.dumping_file).map_err(|e| e.to_string())?);
        }

        env.set_up_initial_env_vars();
        dyld::Dyld::do_late_linking(&mut env);

        if env.bundle.bundle_identifier() == "com.coffeestainstudios.goatsimulator" {
            const REPLAYKIT_SINGLETON: &str =
                "__ZN22UPlatformInterfaceBase32GetReplayKitIntegrationSingletonEv";
            if let Some(address) = env
                .bins
                .iter()
                .find(|bin| bin.name == "GoatGame")
                .and_then(|bin| bin.exported_symbols.get(REPLAYKIT_SINGLETON))
                .copied()
            {
                let address = address & !1;
                env.mem
                    .bytes_at_mut(mem::MutPtr::<u8>::from_bits(address), 4)
                    .copy_from_slice(&[0x00, 0x20, 0x70, 0x47]);
                env.cpu.invalidate_cache_range(address, 4);
                log!("Goat Simulator: disabled unavailable ReplayKit integration.");
            } else {
                log!("Goat Simulator: ReplayKit singleton symbol was not found.");
            }
        }

        env.cpu.set_cpsr(cpu::Cpu::CPSR_USER_MODE);

        if let Some(addrs) = env.options.gdb_listen_addrs.take() {
            let listener = TcpListener::bind(addrs.as_slice())
                .map_err(|e| format!("Could not bind to {addrs:?}: {e}"))?;

            echo!(
                "Waiting for debugger connection on {}...",
                addrs
                    .into_iter()
                    .map(|a| format!("{a}"))
                    .collect::<Vec<String>>()
                    .join(", ")
            );

            let (client, client_addr) = listener
                .accept()
                .map_err(|e| format!("Could not accept connection: {e}"))?;

            echo!("Debugger client connected on {}.", client_addr);
            let mut gdb_server = gdb::GdbServer::new(client);
            let step = gdb_server.wait_for_debugger(None, &mut env.cpu, &mut env.mem);

            assert!(!step, "Can't step right now!"); // TODO?
            env.gdb_server = Some(Box::new(gdb_server));
        }

        if env.options.dumping_options.linking_info {
            let file = env.dump_file.as_mut().unwrap();

            env.objc.dump_classes(file).unwrap();
            env.dyld.dump_lazy_symbols(&env.bins, file).unwrap();
            env.objc
                .dump_selectors(&env.bins[0], &env.mem, file)
                .unwrap();
        }

        env.cpu.branch(entry_point_addr);
        Ok(env)
    }

    /// Set up the emulator environment without loading an app binary.
    ///
    /// This is a special mode that only exists to support the app picker, which
    /// uses the emulated environment to draw its UI and process input. Filling
    /// some of the fields with fake data is a hack, but it means the frameworks
    /// do not need to be aware of the app picker's peculiarities, so it is
    /// cleaner than the alternative!
    pub fn new_without_app(
        options: options::Options,
        icon: image::Image,
    ) -> Result<Environment, String> {
        // Enforces a one (real) Environment limit. See `with_yielder` for
        // why this is needed.
        if ENVIRONMENT_INSTANCE_EXISTS.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Only one (real) Environment can exist at a time!".to_string());
        }
        ENVIRONMENT_INSTANCE_EXISTS.store(true, std::sync::atomic::Ordering::Relaxed);
        let bundle = bundle::Bundle::new_fake_bundle();
        let fs = fs::Fs::new_fake_fs();

        let startup_time = Instant::now();

        let launch_image = None;

        assert!(!options.headless);
        // On macOS, passing no icon restores the bundle's Dock icon after a
        // game has replaced it through SDL_SetWindowIcon.
        #[cfg(target_os = "macos")]
        let window_icon = {
            let _ = icon;
            None
        };
        #[cfg(not(target_os = "macos"))]
        let window_icon = Some(icon);
        let window = Some(Box::new(window::Window::new(
            &format!(
                "{} {}{}{}",
                super::DISPLAY_NAME,
                super::branding(),
                if super::branding().is_empty() {
                    ""
                } else {
                    " "
                },
                super::VERSION
            ),
            window_icon,
            launch_image,
            &options,
        )?));

        let mut mem = mem::Mem::new();

        let bins = Vec::new();

        let mut objc = objc::ObjC::new();

        let mut dyld = dyld::Dyld::new();

        dyld.do_initial_linking_with_no_bins(&mut mem, &mut objc);

        let cpu = cpu::Cpu::new(match options.direct_memory_access {
            true => Some(&mut mem),
            false => None,
        });

        let main_thread = Thread {
            active: true,
            blocked_by: ThreadBlock::NotBlocked,
            return_value: None,
            guest_context: None,
            host_context: None,
            stack: Some(mem::Mem::MAIN_THREAD_STACK_LOW_END..=mem::Mem::MAIN_THREAD_STACK_HIGH_END),
            thread_local_framework_state: Default::default(),
        };

        let mut env = Environment {
            startup_time,
            guest_clock: crate::guest_clock::GuestClock::new(),
            bundle: NullableBox::new(bundle),
            fs: NullableBox::new(fs),
            window,
            openal_manager: NullableBox::new(OpenALManager::new()?),
            mem: NullableBox::new(mem),
            bins,
            objc: NullableBox::new(objc),
            dyld: NullableBox::new(dyld),
            cpu: NullableBox::new(cpu),
            current_thread: 0,
            threads: vec![main_thread],
            libc_state: Default::default(),
            mutex_state: Default::default(),
            framework_state: Default::default(),
            options: NullableBox::new(options),
            gdb_server: None,
            env_vars: Default::default(),
            dump_file: None,
            is_app_picker: true,
            yielder: std::ptr::null(),
            remaining_ticks: None,
            panic_cell: Rc::new(Cell::new(None)),
            cpu_error_bypass_last: None,
            cpu_error_bypass_count: 0,
            cpu_error_bypass_total: 0,
            cpu_error_bypass_last_lr: None,
            cpu_error_bypass_lr_count: 0,
            last_deliberate_trap_bypass: None,
            cpu_skipped_instruction_sites: HashMap::new(),
            guest_termination_requested: None,
            missing_unity_player_archive: None,
            guest_control_flow_redirected: false,
            host_to_guest_stack_frames: Vec::new(),
            trainer: crate::trainer::Trainer::new(false),
        };

        env.set_up_initial_env_vars();

        // Dyld::do_late_linking() would be called here, but it doesn't do
        // anything relevant here, so it's skipped.

        {
            let argv = &[];
            let envp = &[];
            let apple = &[];
            stack::prep_stack_for_start(&mut env.mem, &mut env.cpu, argv, envp, apple, false);
        }

        env.cpu.set_cpsr(cpu::Cpu::CPSR_USER_MODE);

        // GDB server setup would be done here, but there's no need for it.

        // "CPU emulation begins now" would happen here, but there's nothing
        // to emulate. :)

        Ok(env)
    }

    /// Create a new Environment to swap with.
    ///
    /// SAFETY: You must *NEVER, IN ANY CIRCUMSTANCE* dereference any fields or
    /// call any methods on the environment. This means that you must *NEVER,
    /// IN ANY CIRCUMSTANCE* leak this to safe code. You *MUST* make sure this
    /// includes panic safety - do not allow a panic to accidentally smuggle
    /// out this environment to safe code!
    ///
    /// Admittedly, even if this is leaked, it's very unlikely it would lead to
    /// any real problems, just a null pointer deref.
    unsafe fn new_fake() -> Self {
        Self {
            startup_time: Instant::now(),
            guest_clock: crate::guest_clock::GuestClock::new(),
            bundle: NullableBox::null(),
            fs: NullableBox::null(),
            window: None,
            openal_manager: NullableBox::null(),
            mem: NullableBox::null(),
            bins: Vec::new(),
            objc: NullableBox::null(),
            dyld: NullableBox::null(),
            cpu: NullableBox::null(),
            current_thread: 0,
            threads: Vec::new(),
            libc_state: NullableBox::null(),
            framework_state: NullableBox::null(),
            mutex_state: NullableBox::null(),
            options: NullableBox::null(),
            gdb_server: None,
            env_vars: HashMap::new(),
            dump_file: None,
            is_app_picker: true,
            yielder: std::ptr::null(),
            remaining_ticks: None,
            panic_cell: Rc::new(Cell::new(None)),
            cpu_error_bypass_last: None,
            cpu_error_bypass_count: 0,
            cpu_error_bypass_total: 0,
            cpu_error_bypass_last_lr: None,
            cpu_error_bypass_lr_count: 0,
            last_deliberate_trap_bypass: None,
            cpu_skipped_instruction_sites: HashMap::new(),
            guest_termination_requested: None,
            missing_unity_player_archive: None,
            guest_control_flow_redirected: false,
            host_to_guest_stack_frames: Vec::new(),
            trainer: crate::trainer::Trainer::new(false),
        }
    }

    /// Add a [corosensei::Yielder] so that it can be used by the passed
    /// Environment in the passed function. This exists to avoid
    /// reannotating all code with an additional lifetime on every use of
    /// Environment.
    ///
    /// The design _would_ be unsound if it wasn't for the one real
    /// Environment limit.
    ///
    /// Theoretically (even if this is extremely unlikely), this could
    /// happen:
    ///      - New thread(coroutine) is created and calls [Self::with_yielder]
    ///      - Coroutine swaps the provided env with another env.
    ///      - Coroutine moves the env to the executor.
    ///      - Coroutine ends (and ends the yielder).
    ///      - Executor uses yielder - oh no, UAF!
    ///  (While this is pretty theoretical, prudent readers will in fact
    ///  notice this is the exact same way we share Environments across
    ///  threads anyways! - just with [Self::new_fake] instead of a "real"
    ///  Environment.)
    ///
    ///  There is however, (seemingly) no way to (safely) move out behind a
    ///  &mut T without another T to replace it - so it is safe as long there
    ///  is only ever one Environment exposed to safe code (this is part of
    ///  the [Self::new_fake] safety requirements).
    pub fn with_yielder<F, T>(&mut self, yielder: &Yielder<Environment, Environment>, block: F) -> T
    where
        F: FnOnce(&mut Environment) -> T + 'static,
        T: 'static,
    {
        assert!(self.yielder.is_null());

        self.yielder = yielder;
        // We need to ensure panic safety here, so make sure to reset the
        // yielder if the inner function panics.
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| block(self)));
        self.yielder = std::ptr::null();
        match res {
            Ok(ret) => ret,
            Err(e) => {
                std::panic::resume_unwind(e);
            }
        }
    }

    /// Get a shared reference to the window. Panics if touchHLE is running in
    /// headless mode.
    pub fn window(&self) -> &window::Window {
        self.window.as_ref().expect(
            "Tried to do something that needs a window, but touchHLE is running in headless mode!",
        )
    }

    /// Get a mutable reference to the window. Panics if touchHLE is running
    /// in headless mode.
    pub fn window_mut(&mut self) -> &mut window::Window {
        self.window.as_mut().expect(
            "Tried to do something that needs a window, but touchHLE is running in headless mode!",
        )
    }

    pub fn stack_for_longjmp(&self, mut lr: u32, fp: u32) -> Vec<u32> {
        let stack_range = self.threads[self.current_thread].stack.clone().unwrap();

        let mut frames = Vec::new();
        let mut fp: mem::ConstPtr<u8> = mem::Ptr::from_bits(fp);
        let return_to_host_routine_addr = self.dyld.return_to_host_routine().addr_with_thumb_bit();

        while stack_range.contains(&fp.to_bits()) && lr != return_to_host_routine_addr {
            frames.push(lr);

            lr = self.mem.read((fp + 4).cast());
            fp = self.mem.read(fp.cast());
        }
        frames
    }

    /// DIAG (TOUCHHLE_STALL_DIAG): detect a guest hang — frames stopped being
    /// presented while the guest keeps executing — and periodically dump every
    /// active thread's stack so the spinning function can be located.
    fn stall_diag(&self) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static LAST_DUMP_MS: AtomicU64 = AtomicU64::new(0);
        static FIRST_SEEN_MS: AtomicU64 = AtomicU64::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // Record when the run loop first started polling us (proxy for guest
        // start), so we can also catch a guest that hangs BEFORE presenting its
        // very first frame.
        let _ = FIRST_SEEN_MS.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
        let since_present = match crate::gles::present::ms_since_last_present() {
            // A frame has been presented: stall = >3s since the last one.
            Some(ms) if ms >= 3000 => ms,
            Some(_) => return,
            // No frame ever presented: stall = >6s since guest start.
            None => {
                let since_start = now.saturating_sub(FIRST_SEEN_MS.load(Ordering::Relaxed));
                if since_start < 6000 {
                    return;
                }
                since_start
            }
        };
        // Throttle dumps to once every ~2s.
        if now.saturating_sub(LAST_DUMP_MS.load(Ordering::Relaxed)) < 2000 {
            return;
        }
        LAST_DUMP_MS.store(now, Ordering::Relaxed);
        let regs = *self.cpu.regs();
        echo_no_panic!(
            "STALL-DIAG no frame presented for {} ms; current thread #{} PC={:#x} LR={:#x} FP={:#x} SP={:#x}",
            since_present,
            self.current_thread,
            regs[cpu::Cpu::PC],
            regs[cpu::Cpu::LR],
            regs[abi::FRAME_POINTER],
            regs[cpu::Cpu::SP],
        );
        self.stack_trace_all();
    }

    fn dump_all_regs(&self) {
        echo_no_panic!(
            "Dumping registers for current thread (#{})",
            self.current_thread
        );
        self.cpu.dump_regs();
        for (tid, thread) in self.threads.iter().enumerate() {
            if thread.active && tid != self.current_thread {
                echo_no_panic!(
                    "Dumping registers for thread #{} (blocked by {:?})",
                    tid,
                    thread.blocked_by
                );
                let Some(ctx) = thread.guest_context.as_ref() else {
                    echo_no_panic!("Could not get registers for thread {}!", tid);
                    return;
                };
                cpu::Cpu::echo_regs(&ctx.regs);
            }
        }
    }

    pub(crate) fn stack_trace_current(&self) {
        if self.current_thread == 0 {
            echo_no_panic!("Attempting to produce stack trace for main thread:");
        } else {
            echo_no_panic!(
                "Attempting to produce stack trace for thread {}:",
                self.current_thread
            );
        }
        self.stack_trace_for_thread(self.current_thread);
    }

    fn stack_trace_all(&self) {
        echo_no_panic!(
            "Attempting to produce stack trace for current thread (#{}):",
            self.current_thread
        );
        self.stack_trace_for_thread(self.current_thread);
        for tid in 0..self.threads.len() {
            if self.threads[tid].active && tid != self.current_thread {
                echo_no_panic!("Attempting to produce stack trace for thread #{}:", tid);
                self.stack_trace_for_thread(tid);
            }
        }
    }

    fn stack_trace_for_thread(&self, tid: usize) {
        if tid >= self.threads.len() {
            echo_no_panic!(
                "Thread {} is too large ({} threads exist)!",
                tid,
                self.threads.len()
            );
        }
        let Some(stack_range) = self.threads[tid].stack.clone() else {
            echo_no_panic!("Failed to get stack trace!");
            return;
        };
        let (regs, cpsr) = if self.current_thread == tid {
            // Current thread is not stored in context since it is used by cpu,
            // get it from cpu.
            (*self.cpu.regs(), self.cpu.cpsr())
        } else {
            let Some(ctx) = self.threads[tid].guest_context.as_ref() else {
                echo_no_panic!("Failed to get registers for thread {}!", tid);
                return;
            };
            (ctx.regs, ctx.cpsr)
        };
        let pc_nothumb = regs[cpu::Cpu::PC];
        let thumb = (cpsr & cpu::Cpu::CPSR_THUMB) == cpu::Cpu::CPSR_THUMB;
        let pc = GuestFunction::from_addr_and_thumb_flag(pc_nothumb, thumb);
        echo_no_panic!(" 0. {:#x} (PC)", pc.addr_with_thumb_bit());

        let mut lr = regs[cpu::Cpu::LR];
        let return_to_host_routine_addr = self.dyld.return_to_host_routine().addr_with_thumb_bit();
        let thread_exit_routine_addr = self.dyld.thread_exit_routine().addr_with_thumb_bit();

        if lr == return_to_host_routine_addr {
            echo_no_panic!(" 1. [host function] (LR)");
        } else if lr == thread_exit_routine_addr {
            echo_no_panic!(" 1. [thread exit] (LR)");
            return;
        } else {
            echo_no_panic!(" 1. {:#x} (LR)", lr);
        }
        // A corrupted guest frame chain used to make diagnostics loop forever
        // before a recovery path could reject it. Keep stack traces
        // best-effort: only read complete, aligned records inside the current
        // thread's stack; require older frames to be higher on ARM's
        // descending stack; and bound the walk even if guest memory cycles.
        const MAX_STACK_TRACE_FRAMES: usize = 64;
        let mut i = 2;
        let mut fp = regs[abi::FRAME_POINTER];
        for _ in 0..MAX_STACK_TRACE_FRAMES {
            if fp == 0 || !fp.is_multiple_of(4) {
                echo_no_panic!("Next FP ({:#x}) is null or unaligned.", fp);
                break;
            }
            let Some(saved_lr_addr) = fp.checked_add(4) else {
                echo_no_panic!("Next FP ({:#x}) overflows its frame record.", fp);
                break;
            };
            if !stack_range.contains(&fp) || !stack_range.contains(&saved_lr_addr) {
                echo_no_panic!("Next FP ({:#x}) is outside the stack.", fp);
                break;
            }

            lr = self.mem.read(mem::ConstPtr::<u32>::from_bits(saved_lr_addr));
            let previous_fp: u32 = self.mem.read(mem::ConstPtr::<u32>::from_bits(fp));
            if lr == return_to_host_routine_addr {
                echo_no_panic!("{:2}. [host function]", i);
            } else if lr == thread_exit_routine_addr {
                echo_no_panic!("{:2}. [thread exit]", i);
                return;
            } else {
                echo_no_panic!("{:2}. {:#x}", i, lr);
            }

            if previous_fp == 0 {
                echo_no_panic!("Next FP is null.");
                break;
            }
            if previous_fp <= fp || !previous_fp.is_multiple_of(4) {
                echo_no_panic!(
                    "Next FP ({:#x}) does not advance toward an older frame.",
                    previous_fp
                );
                break;
            }
            fp = previous_fp;
            i += 1;
        }
    }

    /// Create a new thread and return its ID. The `start_routine` and
    /// `user_data` arguments have the same meaning as the last two arguments to
    /// `pthread_create`.
    pub fn new_thread(
        &mut self,
        start_routine: abi::GuestFunction,
        user_data: mem::MutVoidPtr,
        stack_size: GuestUSize,
    ) -> ThreadId {
        let stack_alloc = self.mem.alloc(stack_size);
        let stack_high_addr = stack_alloc.to_bits() + stack_size;
        assert!(stack_high_addr.is_multiple_of(4));

        let thread_stack =
            DefaultStack::new(16 * 1024 * 1024).expect("failed to allocate guest coroutine stack");
        let thread_routine =
            Coroutine::with_stack(thread_stack, move |yielder, mut env: Environment| {
                log_dbg!(
                    "touchHLE: guest worker thread now running (start_routine={:#x})",
                    start_routine.addr_with_thumb_bit()
                );
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    env.with_yielder(yielder, move |env| {
                        let regs = env.cpu.regs_mut();
                        regs[cpu::Cpu::LR] = env.dyld.thread_exit_routine().addr_with_thumb_bit();
                        regs[cpu::Cpu::SP] = stack_high_addr;
                        regs[0] = user_data.to_bits();

                        env.cpu.set_cpsr(
                            cpu::Cpu::CPSR_USER_MODE
                                | ((start_routine.is_thumb() as u32) * cpu::Cpu::CPSR_THUMB),
                        );
                        let return_value: mem::MutVoidPtr =
                            start_routine.call_from_host(env, (user_data,));
                        let curr_thread = &mut env.threads[env.current_thread];
                        curr_thread.return_value = Some(return_value);
                        env.exit_if_last_thread();
                        env.threads[env.current_thread].active = false;
                    });
                }));
                if let Err(e) = res {
                    let panic_cell = env.panic_cell.clone();
                    panic_cell.set(Some(env));
                    std::panic::resume_unwind(e);
                }
                env
            });

        self.threads.push(Thread {
            active: true,
            blocked_by: ThreadBlock::NotBlocked,
            return_value: None,
            guest_context: Some(Box::new(cpu::CpuContext::new())),
            host_context: Some(thread_routine),
            stack: Some(stack_alloc.to_bits()..=(stack_high_addr - 1)),
            thread_local_framework_state: Default::default(),
        });

        let new_thread_id = self.threads.len() - 1;

        log_dbg!("Created new thread {} with stack {:#x}–{:#x}, will execute function {:?} with data {:?}", new_thread_id, stack_alloc.to_bits(), (stack_high_addr - 1), start_routine, user_data);

        new_thread_id
    }

    #[allow(unused)]
    pub fn get_tl_framework_state(&mut self) -> &mut frameworks::ThreadLocalState {
        &mut self.threads[self.current_thread].thread_local_framework_state
    }

    /// Put the current thread to sleep for some duration, running other threads
    /// in the meantime as appropriate. Functions that call sleep right before
    /// they return back to the main run loop ([Environment::run]) should set
    /// `tail_call`.
    pub fn sleep(&mut self, duration: Duration) {
        log_dbg!(
            "Thread {} is going to sleep for {:?}.",
            self.current_thread,
            duration
        );
        let until = Instant::now().checked_add(duration).unwrap();
        self.yield_thread(ThreadBlock::Sleeping(until));
    }

    /// Guest-requested sleep. Keep its deadline in game time so changing
    /// speed also affects waits already in progress. Host pacing uses sleep().
    pub fn sleep_guest(&mut self, duration: Duration) {
        let until = self.guest_clock.now().checked_add(duration).unwrap();
        self.yield_thread(ThreadBlock::GuestSleeping(until));
    }

    #[allow(dead_code)]
    pub fn suspend_thread(&mut self, thread: ThreadId) {
        match &mut self.threads[thread].blocked_by {
            ThreadBlock::Suspended(count, _) => {
                *count += 1;
            }
            _ => {
                let previous_thread_state = std::mem::replace(
                    &mut self.threads[thread].blocked_by,
                    ThreadBlock::NotBlocked,
                );
                log_dbg!("Suspend thread {} from {:?}", thread, previous_thread_state);
                let new_state = ThreadBlock::Suspended(1, Box::new(previous_thread_state));
                if thread == self.current_thread {
                    self.yield_thread(new_state);
                } else {
                    self.threads[thread].blocked_by = new_state;
                }
            }
        }
    }

    #[allow(dead_code)]
    pub fn resume_thread(&mut self, thread: ThreadId) {
        let old = std::mem::replace(
            &mut self.threads[thread].blocked_by,
            ThreadBlock::NotBlocked,
        );
        match old {
            ThreadBlock::Suspended(count, previous_thread_state) => {
                assert!(count > 0);
                if count > 1 {
                    self.threads[thread].blocked_by =
                        ThreadBlock::Suspended(count - 1, previous_thread_state);
                } else {
                    log_dbg!("Resume thread {} to {:?}", thread, previous_thread_state);
                    self.threads[thread].blocked_by = *previous_thread_state;
                }
            }
            other => {
                // The caller asked to resume a thread that is not currently
                // suspended. Restore whatever state it was in (already
                // overwritten with NotBlocked above) and log a warning
                // instead of crashing the host.
                log!(
                    "Warning: resume_thread({}) called on a thread that was not Suspended (was {:?}); leaving thread NotBlocked.",
                    thread,
                    other
                );
                self.threads[thread].blocked_by = other;
            }
        }
    }

    /// Block the current thread until the given mutex unlocks.
    ///
    /// Other threads also blocking on this mutex may get access first.
    /// Like all other thread blocking functions, this will suspend
    /// execution of the current host thread.
    pub fn block_on_mutex(&mut self, mutex_id: MutexId) {
        log_dbg!(
            "Thread {} blocking on mutex #{}.",
            self.current_thread,
            mutex_id
        );
        self.yield_thread(ThreadBlock::Mutex(mutex_id));
    }

    /// Locks a semaphore (decrements value of a semaphore and blocks
    /// if necessary).
    ///
    /// Like all other thread blocking functions, this will suspend
    /// execution of the current host thread (if the semaphore is
    /// currently at 0).
    pub fn sem_decrement(&mut self, sem: MutPtr<sem_t>, wait_on_lock: bool) -> bool {
        let Some(host_sem_rc) = self.libc_state.semaphore.open_semaphores.get_mut(&sem) else {
            // The guest called sem_wait/sem_trywait on an uninitialised or
            // already-destroyed semaphore. POSIX/Apple document this as an
            // `EINVAL` failure of the wait, so we report failure to the caller
            // (which sets errno) instead of aborting the whole process.
            log!(
                "Warning: sem_decrement called on unknown semaphore {:?}; \
                 failing without blocking (guest likely waited on an \
                 uninitialised or destroyed semaphore).",
                sem
            );
            return false;
        };
        let mut host_sem = (*host_sem_rc).borrow_mut();

        if host_sem.value > 0 {
            host_sem.value -= 1;
            return true;
        }

        if !wait_on_lock {
            log_dbg!(
                "sem_decrement: semaphore {:?} attempted decrement without waiting, failed",
                sem,
            );
            return false;
        }
        host_sem.waiting.insert(self.current_thread);
        std::mem::drop(host_sem);
        // The scheduler will decrement the semaphore value when it unblocks.
        self.yield_thread(ThreadBlock::Semaphore(sem));
        true
    }

    /// Unlock a semaphore (increments value of a semaphore).
    ///
    /// Returns `true` if the semaphore was known and incremented, `false` if
    /// the pointer does not refer to a semaphore this environment is tracking.
    /// A guest may legitimately hit the latter case by calling `sem_post` on an
    /// uninitialised or already-destroyed semaphore; POSIX/Apple document this
    /// as an `EINVAL` failure of `sem_post`, not a reason to abort the whole
    /// process, so callers translate the `false` return into that errno instead
    /// of panicking.
    pub fn sem_increment(&mut self, sem: MutPtr<sem_t>) -> bool {
        let Some(host_sem_rc) = self.libc_state.semaphore.open_semaphores.get_mut(&sem) else {
            log!(
                "Warning: sem_increment called on unknown semaphore {:?}; \
                 ignoring (guest likely posted an uninitialised or destroyed semaphore).",
                sem
            );
            return false;
        };
        let mut host_sem = (*host_sem_rc).borrow_mut();

        host_sem.value += 1;
        log_dbg!(
            "sem_increment: semaphore {:?} is now {}",
            sem,
            host_sem.value
        );
        true
    }

    /// Blocks the current thread until the thread given finishes, writing its
    /// return value to ptr (if non-null).
    ///
    /// Note that there are no protections against joining with a detached
    /// thread, joining a thread with itself, or deadlocking joins. Callers
    /// should ensure these do not occur!
    ///
    /// Like all other thread blocking functions, this will suspend
    /// execution of the current host thread.
    pub fn join_with_thread(&mut self, joinee_thread: ThreadId, ptr: MutPtr<MutVoidPtr>) {
        log_dbg!(
            "Thread {} waiting for thread {} to finish.",
            self.current_thread,
            joinee_thread
        );
        self.yield_thread(ThreadBlock::Joining(joinee_thread, ptr));
    }

    pub fn run_app_picker<F, R>(mut self, f: F) -> R
    where
        F: FnOnce(&mut Environment) -> R + 'static,
        R: 'static,
    {
        let panic_cell = Rc::new(Cell::new(None));
        let mut app_picker_coroutine = Coroutine::new(move |yielder, mut env: Environment| {
            env.panic_cell = panic_cell.clone();
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                env.with_yielder(yielder, f)
            }));
            match res {
                // We want the environment to be dropped outside of the
                // coroutine, so send it back when we return.
                Ok(r) => (r, env),
                Err(e) => {
                    let panic_cell = env.panic_cell.clone();
                    panic_cell.set(Some(env));
                    std::panic::resume_unwind(e);
                }
            }
        });

        loop {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                app_picker_coroutine.resume(self)
            }));
            self = match res {
                Ok(ret) => match ret {
                    corosensei::CoroutineResult::Yield(env) => env,
                    corosensei::CoroutineResult::Return((ret_val, _env)) => {
                        return ret_val;
                    }
                },
                Err(e) => {
                    log_no_panic!("Crash in app picker!");
                    // No need to get the environment back - It's local to this
                    // function anyways.
                    std::panic::resume_unwind(e);
                }
            };
            // As with the main app run loop, poll for events with SDL treating
            // the current stack as the main stack; without this, event polling
            // would be skipped for the rest of the picker session as soon as
            // anything calls on_parent_stack_in_coroutine().
            {
                let window = self.window.as_mut().unwrap();
                window.on_main_stack = true;
                window.poll_for_events(self.options.as_ref());
            }
            assert!(self.threads.len() == 1);
            match self.threads[0].blocked_by {
                ThreadBlock::NotBlocked => {}
                ThreadBlock::Sleeping(until) => {
                    let duration = until.duration_since(Instant::now());
                    std::thread::sleep(duration);
                }
                ThreadBlock::GuestSleeping(until) => {
                    let due = self.guest_clock.host_deadline(until);
                    std::thread::sleep(due.saturating_duration_since(Instant::now()));
                }
                ref other => {
                    log!(
                        "Warning: Unexpected ThreadBlock in app picker: {:?}; clearing block.",
                        other
                    );
                }
            }
            self.threads[0].blocked_by = ThreadBlock::NotBlocked;
        }
    }

    /// Run the emulator. This is the main loop and won't return until app exit.
    /// Only `main.rs` should call this.
    pub(crate) fn run(mut self) -> Option<GuestTermination> {
        let mut curr_host_context = self.threads[0].host_context.take().unwrap();
        let panic_cell = self.panic_cell.clone();
        let mut stepping = false;
        loop {
            #[cfg(target_os = "ios")]
            if crate::take_host_exit_request() {
                echo!("Returning to the iOS host library.");
                let thread = &mut self.threads[self.current_thread];
                assert!(thread.host_context.is_none());
                thread.host_context = Some(curr_host_context);
                return None;
            }

            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            if self.window.as_ref().is_some_and(|window| window.close_requested()) {
                self.threads[self.current_thread].host_context = Some(curr_host_context);
                return Some(GuestTermination::ReturnToPicker);
            }

            if stepping {
                self.remaining_ticks = None;
            } else {
                // 1,000,000 ticks is an arbitrary number. It needs to be
                // reasonably large so we aren't jumping in and out of dynarmic
                // or trying to poll for events too often. At the same time,
                // very large values are bad for responsiveness.
                //
                // PERF: raised from 100,000 to 1,000,000: the per-batch costs
                // (leaving/re-entering the JIT, coroutine switch, scheduler
                // pass and one event-poll attempt) are amortised 10x better.
                // This is responsiveness-safe because:
                // - OS event polling has its own throttle in
                //   Window::poll_for_events (roughly 120 Hz), independent of
                //   batch size;
                // - sleeping guest threads use absolute host deadlines, so the
                //   scheduler still wakes them on time between batches;
                // - in practice most batches end early anyway, when the guest
                //   calls a host framework function (which happens many times
                //   per frame: present, timers, audio, input, etc.), so the
                //   larger cap mostly helps busy guest spin-loops, which is
                //   exactly where the per-batch overhead used to matter.
                //
                // FRAME PACING: a thread still has to finish its current batch
                // before the scheduler gets to wake any *sleeping* thread
                // whose deadline arrived in the meantime, so with the large
                // batch a pacing/timer/audio wake-up could land up to one
                // batch (~a few ms at ~1M ticks) late. To keep
                // millisecond-accurate deadlines (frame pacing, run-loop
                // timers, audio callbacks) precise while retaining the
                // overhead win in long busy stretches, fall back to the
                // smaller batch whenever any thread has an imminent wake-up.
                let imminent_wakeup = self.threads.iter().any(|thread| {
                    let deadline = match thread.blocked_by {
                        ThreadBlock::Sleeping(due) => Some(due),
                        ThreadBlock::GuestSleeping(due) => Some(self.guest_clock.host_deadline(due)),
                        _ => None,
                    };
                    deadline.is_some_and(|due| due < Instant::now() + Duration::from_millis(10))
                });
                self.remaining_ticks = Some(if imminent_wakeup {
                    100_000
                } else {
                    1_000_000
                });
            }
            // Game trainer (Cheat Engine-style memory search/patch + on-screen
            // UI). Disabled by default; skip it unless `--trainer` is enabled.
            if self.trainer.enabled {
                self.trainer.tick(
                    &mut self.mem,
                    Some(self.bundle.bundle_identifier()),
                    &self.objc,
                );
            }
            if let Some(speed) = crate::trainer_ui::take_speed_request() {
                self.guest_clock.set_speed(speed);
                crate::trainer_ui::publish_status(format!("GAME SPEED {}", speed.label()));
            }
            let mut kill_current_thread = false;
            if let Some(w) = self.window.as_mut() {
                w.on_main_stack = false;
            }
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                curr_host_context.resume(self)
            }));
            self = match res {
                Ok(ret) => match ret {
                    corosensei::CoroutineResult::Yield(env) => env,
                    corosensei::CoroutineResult::Return(env) => {
                        kill_current_thread = true;
                        env
                    }
                },
                Err(e) => {
                    let Some(mut env) = panic_cell.take() else {
                        log_no_panic!("Did not recieve env from coroutine unwind, must abort!");
                        std::process::exit(-1)
                    };
                    if let Some(window) = env.window.as_mut() {
                        window.on_main_stack = true;
                    };

                    echo!("Register state immediately after panic:");
                    env.dump_all_regs();
                    env.stack_trace_all();

                    if env.options.popup_errors {
                        let error_string = if let Some(s) = e.downcast_ref::<&str>() {
                            s
                        } else if let Some(s) = e.downcast_ref::<String>() {
                            s
                        } else {
                            "(non-string payload)"
                        };
                        window::show_error_messagebox(env.window.as_deref(), error_string);
                    }
                    // Put the host context back before resuming, the env will
                    // clean it up on drop.
                    let Some(thread) = env.threads.get_mut(env.current_thread) else {
                        log_no_panic!("Bad current_thread, must abort!");
                        std::process::exit(-1)
                    };
                    thread.host_context = Some(curr_host_context);
                    std::panic::resume_unwind(e);
                }
            };
            let mut old_context = if kill_current_thread {
                log_dbg!("Killing thread {}", self.current_thread);
                panic_cell.set(Some(self));
                std::mem::drop(curr_host_context);
                let Some(env) = panic_cell.take() else {
                    log_no_panic!("Did not get env back from coroutine after drop, must abort!");
                    std::process::exit(-1)
                };
                self = env;
                self.threads[self.current_thread].active = false;
                let stack = self.threads[self.current_thread].stack.take().unwrap();
                let stack: mem::MutVoidPtr = mem::Ptr::from_bits(*stack.start());
                log_dbg!("Freeing thread {} stack {:?}", self.current_thread, stack);
                // The main stack is a fixed mapping, not a heap allocation.
                if self.current_thread != 0 {
                    self.mem.free(stack);
                }
                None
            } else {
                Some(curr_host_context)
            };
            if let Some(w) = self.window.as_mut() {
                w.on_main_stack = true;
            }

            // DIAG (TOUCHHLE_STALL_DIAG=1): if the guest keeps running but stops
            // presenting frames, it is hung/spinning. Periodically dump the
            // stack of every active thread so the spinning function can be
            // identified. Env-gated so it costs nothing in normal runs.
            if crate::env_flag_cached!("TOUCHHLE_STALL_DIAG") {
                self.stall_diag();
            }

            if kill_current_thread && self.guest_termination_requested.is_some() {
                echo!("Guest session ended through the controlled return-to-host path.");
                return self.guest_termination_requested;
            }
            if self.guest_termination_requested.is_some() {
                // A host callback may have yielded before its linked-function
                // dispatch reached the return-to-host check. Put this live
                // context back so `Environment::drop` can unwind it safely,
                // then stop before the scheduler resumes another guest thread.
                self.threads[self.current_thread].host_context = old_context.take();
                echo!("Guest session ended while returning from a host callback.");
                return self.guest_termination_requested;
            }

            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // To maintain responsiveness when moving the window and so on,
                // we need to poll for events occasionally, even if the app
                // isn't actively processing them.
                // Polling for events can be quite expensive, so we shouldn't do
                // this until after we've done some amount of work on the guest
                // thread, lest every single callback call pay this cost.
                if let Some(ref mut window) = self.window {
                    window.poll_for_events(&self.options);
                }
                let curr_thread_block = self.threads[self.current_thread].blocked_by.clone();
                if stepping || matches!(curr_thread_block, ThreadBlock::WaitingForDebugger(_)) {
                    if old_context.is_none() {
                        let old_thread = self.current_thread;
                        let next_thread = self.schedule_next_thread();
                        self.switch_thread(&mut old_context, next_thread);
                        echo!(
                            "\nGDB WARNING ------- Thread {} has exited - switched thread to {}",
                            old_thread,
                            next_thread
                        );
                    }
                    match self.threads[self.current_thread].blocked_by {
                        ThreadBlock::NotBlocked | ThreadBlock::WaitingForDebugger(_) => {}
                        _ => {
                            let old_thread = self.current_thread;
                            let next_thread = self.schedule_next_thread();
                            self.switch_thread(&mut old_context, next_thread);
                            let block = &self.threads[old_thread].blocked_by;
                            echo!(
                                "\nGDB WARNING ------- Thread {} is blocked by {:?} - switched thread to {}",
                                old_thread,
                                block,
                                next_thread
                            );
                        }
                    }
                    let reason = if let ThreadBlock::WaitingForDebugger(reason) = curr_thread_block
                    {
                        self.threads[self.current_thread].blocked_by = ThreadBlock::NotBlocked;
                        reason.clone()
                    } else {
                        None
                    };
                    let will_step = self.gdb_server.as_deref_mut().unwrap().wait_for_debugger(
                        reason.clone(),
                        self.cpu.as_mut(),
                        self.mem.as_mut(),
                    );
                    if will_step {
                        stepping = true;
                    }
                }

                // Don't switch threads if stepping.
                if stepping {
                    assert!(old_context.is_some());
                    return;
                }

                stepping = false;
                let next_thread = self.schedule_next_thread();
                if next_thread != self.current_thread {
                    self.switch_thread(&mut old_context, next_thread);
                }
                assert!(old_context.is_some());
            }));

            match res {
                Ok(_) => {}
                Err(e) => {
                    if let Some(window) = self.window.as_mut() {
                        window.on_main_stack = true;
                    };
                    if self.options.popup_errors {
                        let error_string = if let Some(s) = e.downcast_ref::<&str>() {
                            s
                        } else if let Some(s) = e.downcast_ref::<String>() {
                            s
                        } else {
                            "(non-string payload)"
                        };
                        window::show_error_messagebox(self.window.as_deref(), error_string);
                    }
                    echo!("Register state immediately after panic:");
                    self.dump_all_regs();
                    self.stack_trace_all();

                    // Clean up the used host context. The ones inside the env
                    // are cleaned up by the drop handler.
                    let panic_cell = self.panic_cell.clone();
                    if let Some(ctx) = old_context {
                        panic_cell.set(Some(self));
                        std::mem::drop(ctx);
                        self = panic_cell.take().unwrap_or_else(|| {
                            log_no_panic!(
                            "Did not recieve env from coroutine unwind during drop, must abort!"
                            );
                            std::process::exit(-1)
                        });
                        std::mem::drop(self);
                    };
                    std::panic::resume_unwind(e);
                }
            }
            curr_host_context = old_context.unwrap();
        }
    }

    /// Record explicit thread exit separately from returning from main.
    pub(crate) fn note_pthread_exit(&mut self, retval: mem::MutVoidPtr) {
        self.threads[self.current_thread].return_value = Some(retval);
    }

    /// The last thread ends the process as exit(0), while its coroutine can
    /// still execute guest atexit handlers. Other threads must remain alive
    /// when only the main thread calls pthread_exit.
    fn exit_if_last_thread(&mut self) {
        if !self.is_guest_termination_requested()
            && !self
                .threads
                .iter()
                .enumerate()
                .any(|(id, thread)| id != self.current_thread && thread.active)
        {
            crate::libc::stdlib::exit_session(self, 0);
        }
    }

    /// Request that active guest execution returns through its host boundary.
    ///
    /// Linked libc functions use this for process termination. The request is
    /// observed by the CPU loop after the host function
    /// returns, so no guest instruction after a `noreturn` call is executed.
    pub(crate) fn request_guest_termination(&mut self, termination: GuestTermination) {
        self.guest_termination_requested = Some(termination);
    }

    /// Whether a linked guest termination function has requested session end.
    pub(crate) fn is_guest_termination_requested(&self) -> bool {
        self.guest_termination_requested.is_some()
    }

    /// Whether the current thread is still inside the call chain in which a
    /// deliberate guest trap (`udf`/`trap`/`bkpt`) was bypassed.
    pub(crate) fn guest_trap_bypass_still_on_stack(&mut self) -> bool {
        let Some((thread, trap_sp)) = self.last_deliberate_trap_bypass else {
            return false;
        };
        if thread != self.current_thread {
            return false;
        }
        if self.cpu.regs()[cpu::Cpu::SP] > trap_sp {
            self.last_deliberate_trap_bypass = None;
            return false;
        }
        true
    }

    /// Remember that Unity's mandatory serialized player archive is unavailable.
    ///
    /// This records both a missing VFS node and an unreadable/empty archive
    /// entry. When the build has no other player data, the archive is not
    /// optional: once Unity has reported this failure it calls `exit(1)` after
    /// partially initializing global state. Treating that exit as a
    /// recoverable DRM-style exit leads to use-after-null and bogus
    /// multi-gigabyte allocations, as the engine's cleanup path was not
    /// designed to return to the app.
    ///
    /// Builds that ship their player data as loose files (`globalgamemanagers`
    /// or, before Unity 5.4, `mainData`) have no archive at all; Unity logs
    /// that it is "using local filesystem" and carries on, so nothing is
    /// recorded for those.
    pub(crate) fn note_missing_unity_player_archive(&mut self, path: &str) {
        let Some((data_dir, file)) = path.rsplit_once('/') else {
            return;
        };
        let is_player_archive = file.eq_ignore_ascii_case("data.unity3d")
            && data_dir
                .rsplit('/')
                .next()
                .is_some_and(|component| component.eq_ignore_ascii_case("Data"));
        if !is_player_archive || self.missing_unity_player_archive.is_some() {
            return;
        }
        let has_loose_player_data = ["globalgamemanagers", "mainData"].iter().any(|name| {
            crate::libc::posix_io::resolve_existing_guest_path(self, &format!("{data_dir}/{name}"))
                .is_some()
        });
        if has_loose_player_data {
            return;
        }

        self.missing_unity_player_archive = Some(path.to_owned());
        log!(
            "Required Unity player archive {:?} is unavailable from the mounted \
             bundle.",
            path
        );
    }

    /// The unavailable Unity player archive, if a resource probe found one.
    pub(crate) fn missing_unity_player_archive(&self) -> Option<&str> {
        self.missing_unity_player_archive.as_deref()
    }

    /// Preserve a deliberate guest-PC redirect across linked-stub dispatch.
    pub(crate) fn note_guest_control_flow_redirect(&mut self) {
        self.guest_control_flow_redirected = true;
    }

    /// Whether the active linked host function redirected guest control flow.
    pub(crate) fn is_guest_control_flow_redirected(&self) -> bool {
        self.guest_control_flow_redirected
    }

    /// Mark a synthetic frame installed for a host-to-guest call.
    pub(crate) fn push_host_to_guest_stack_frame(
        &mut self,
        frame_pointer: u32,
    ) -> (ThreadId, u32) {
        let frame = (self.current_thread, frame_pointer);
        self.host_to_guest_stack_frames.push(frame);
        frame
    }

    /// Remove a synthetic frame after its host-to-guest call has returned.
    pub(crate) fn pop_host_to_guest_stack_frame(&mut self, frame: (ThreadId, u32)) {
        if let Some(index) = self
            .host_to_guest_stack_frames
            .iter()
            .rposition(|&candidate| candidate == frame)
        {
            self.host_to_guest_stack_frames.remove(index);
        } else {
            log_no_panic!(
                "Warning: synthetic host-to-guest frame {:?} was not tracked.",
                frame
            );
        }
    }

    /// Whether the current frame is a synthetic host-to-guest call boundary.
    pub(crate) fn is_host_to_guest_stack_frame(&self, frame_pointer: u32) -> bool {
        self.host_to_guest_stack_frames
            .iter()
            .any(|&(thread, fp)| thread == self.current_thread && fp == frame_pointer)
    }

    /// Run the emulator until the app returns control to the host. This is for
    /// host-to-guest function calls (see [abi::CallFromHost::call_from_host]).
    ///
    /// Note that this might execute code from other threads while waiting for
    /// the app to return control on the original thread!
    pub fn run_call(&mut self) {
        let old_thread = self.current_thread;
        self.run_inner();
        assert!(self.current_thread == old_thread);
    }

    /// Switch the current thread, putting the old host context (if it exists)
    /// back into its thread, and the new host context where the old one was.
    ///
    /// This also internally switches the currently used guest context.
    fn switch_thread(&mut self, old_context: &mut Option<HostContext>, new_thread: ThreadId) {
        assert!(new_thread != self.current_thread);
        assert!(self.threads[new_thread].active);
        log_dbg!(
            "Switching thread: {} => {}",
            self.current_thread,
            new_thread
        );
        let mut guest_ctx = self.threads[new_thread].guest_context.take().unwrap();
        self.cpu.swap_context(&mut guest_ctx);
        assert!(self.threads[self.current_thread].guest_context.is_none());
        assert!(old_context.is_some() || !self.threads[self.current_thread].active);
        self.threads[self.current_thread].guest_context = Some(guest_ctx);

        let new_host_ctx = self.threads[new_thread].host_context.take().unwrap();
        self.threads[self.current_thread].host_context = old_context.take();
        *old_context = Some(new_host_ctx);
        self.current_thread = new_thread;
        self.set_tpidruro_for_current_thread();
    }

    /// Point the CPU's TPIDRURO register at the current thread's guest
    /// thread-specific data array. See [libc::pthread::key].
    fn set_tpidruro_for_current_thread(&mut self) {
        let array = libc::pthread::key::tsd_array_for_thread(self, self.current_thread);
        self.cpu.set_tpidruro(array.to_bits());
    }

    fn is_recoverable_guest_cpu_error(error: &cpu::CpuError) -> bool {
        matches!(
            error,
            cpu::CpuError::UndefinedInstruction | cpu::CpuError::Breakpoint
        )
    }

    /// Dump 8 words of guest code at `addr` as little-endian hex, for crash
    /// diagnostics. Unmapped words read as `????????`.
    fn guest_code_words(&self, addr: u32) -> String {
        let mut out = String::new();
        for word in 0..8u32 {
            let at = addr.wrapping_add(word * 4);
            match self.read_guest_u16_fallible(at) {
                Some(lo) => match self.read_guest_u16_fallible(at.wrapping_add(2)) {
                    Some(hi) => out.push_str(&format!("{:04x}{:04x} ", lo, hi)),
                    None => out.push_str("???????? "),
                },
                None => out.push_str("???????? "),
            }
        }
        out
    }

    /// Read a guest halfword without panicking if the address is unmapped.
    fn read_guest_u16_fallible(&self, addr: u32) -> Option<u16> {
        let bytes = self
            .mem
            .get_bytes_fallible(mem::ConstVoidPtr::from_bits(addr), 2)?;
        Some(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// Read a guest word without panicking if the address is unmapped.
    fn read_guest_u32_fallible(&self, addr: u32) -> Option<u32> {
        let bytes = self
            .mem
            .get_bytes_fallible(mem::ConstVoidPtr::from_bits(addr), 4)?;
        Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Whether `addr` (without Thumb bit) lies inside a machine-code section
    /// of one of the loaded Mach-O images, or is one of dyld's synthetic
    /// guest routines. This is used to tell "the app deliberately executed a
    /// trap" apart from "the PC wandered off into data".
    fn is_guest_code_address(&self, addr: u32) -> bool {
        let addr = addr & !1;
        if addr == 0 {
            return false;
        }
        if addr == self.dyld.return_to_host_routine().addr_without_thumb_bit()
            || addr == self.dyld.thread_exit_routine().addr_without_thumb_bit()
        {
            return true;
        }
        self.bins.iter().any(|bin| {
            bin.sections.iter().any(|section| {
                let is_code_section = section.type_ == mach_o::SectionType::SymbolStubs
                    || matches!(
                        &*section.name,
                        "__text" | "__textcoal_nt" | "__stub_helper" | "__StaticInit"
                    );
                is_code_section
                    && addr >= section.addr
                    && addr.wrapping_sub(section.addr) < section.size
            })
        })
    }

    /// Best-effort recovery for a guest that jumped into garbage: scan the
    /// words at and above SP for one that looks like a return address (it
    /// points into a code section and is preceded by a direct `bl`/`blx`).
    /// Returns `(return_addr, caller_sp)` where `caller_sp` is the SP the
    /// caller will have after the abandoned function's frame is gone.
    ///
    /// This is deliberately conservative and only used when branching to LR
    /// has already been shown not to make progress, because a bogus unwind
    /// skips whatever the abandoned call was supposed to do.
    fn validated_stack_return_address(&self, sp: u32) -> Option<(u32, u32)> {
        const MAX_SCAN_WORDS: u32 = 64;
        let mut addr = sp;
        for _ in 0..MAX_SCAN_WORDS {
            let word = self.read_guest_u32_fallible(addr)?;
            let candidate = word & !1;
            if self.is_guest_code_address(candidate)
                && decode_direct_call_before(
                    word,
                    |a| self.read_guest_u16_fallible(a),
                    |a| self.read_guest_u32_fallible(a),
                )
                .is_some()
            {
                return Some((word, addr.wrapping_add(4)));
            }
            addr = addr.checked_add(4)?;
        }
        None
    }

    /// Decode what kind of instruction raised an UndefinedInstruction or
    /// Breakpoint at `pc`.
    fn classify_guest_trap(&self, pc: u32, thumb: bool, instruction_len: u32) -> GuestTrapKind {
        if !self.is_guest_code_address(pc) {
            return GuestTrapKind::OutsideCode;
        }

        if thumb {
            let Some(hw1) = self.read_guest_u16_fallible(pc) else {
                return GuestTrapKind::OutsideCode;
            };
            if instruction_len == 2 {
                // UDF #imm8 (clang's `trap`/`__builtin_trap()` is `udf #0xfe`,
                // GDB's software breakpoint is `udf #1`) and BKPT #imm8.
                if (hw1 & 0xff00) == 0xde00 || (hw1 & 0xff00) == 0xbe00 {
                    return GuestTrapKind::DeliberateTrap {
                        encoding: hw1 as u32,
                    };
                }
                return GuestTrapKind::UndecodableInstruction {
                    encoding: hw1 as u32,
                };
            }
            let Some(hw2) = self.read_guest_u16_fallible(pc.wrapping_add(2)) else {
                return GuestTrapKind::OutsideCode;
            };
            let encoding = ((hw1 as u32) << 16) | (hw2 as u32);
            // UDF.W #imm16: 1111 0111 1111 iiii 1010 iiii iiii iiii
            if (hw1 & 0xfff0) == 0xf7f0 && (hw2 & 0xf000) == 0xa000 {
                return GuestTrapKind::DeliberateTrap { encoding };
            }
            // Is `pc` actually the second halfword of a `bl`/`blx <label>`
            // starting at `pc - 2`? (Seen in Asphalt 8: 0xfbea at 0x196cc,
            // which is not a valid long-multiply encoding but is a perfectly
            // good BL suffix.)
            if let Some(previous_hw) = self.read_guest_u16_fallible(pc.wrapping_sub(2)) {
                let is_bl_prefix = (previous_hw & 0xf800) == 0xf000;
                let is_bl_suffix = (hw1 & 0xd000) == 0xd000;
                let is_blx_suffix = (hw1 & 0xd001) == 0xc000;
                if is_bl_prefix && (is_bl_suffix || is_blx_suffix) {
                    return GuestTrapKind::MisalignedInstruction { encoding };
                }
            }
            // Dynarmic implements essentially all of ARMv7-A (including VFP
            // and NEON), so an undecodable encoding inside a code section is
            // much more likely to be data the PC has wandered into than an
            // instruction it genuinely lacks. Only the latter should be
            // stepped over; the former has to be escaped from instead.
            if undecodable::undecodable_site_is_likely_code(pc, true, instruction_len, |addr| {
                self.read_guest_u16_fallible(addr)
            }) {
                GuestTrapKind::UndecodableInstruction { encoding }
            } else {
                GuestTrapKind::ExecutionInData { encoding }
            }
        } else {
            let Some(encoding) = self.read_guest_u32_fallible(pc) else {
                return GuestTrapKind::OutsideCode;
            };
            // UDF (any condition): cccc 0111 1111 iiii iiii iiii 1111 iiii.
            // Clang's ARM `trap` is 0xe7ffdefe, GDB uses 0xe7f001f0.
            // BKPT: cccc 0001 0010 iiii iiii iiii 0111 iiii.
            if (encoding & 0x0ff000f0) == 0x07f000f0 || (encoding & 0x0ff000f0) == 0x01200070 {
                return GuestTrapKind::DeliberateTrap { encoding };
            }
            GuestTrapKind::UndecodableInstruction { encoding }
        }
    }

    /// Validate the ARM frame record the frame pointer (r7) currently points
    /// at, without touching CPU state.
    ///
    /// Returns `(fp, saved_fp, saved_lr, caller_sp)` if the record is inside
    /// the current thread's stack, the chain moves toward older frames, and
    /// the saved return address is plausible guest code (or one of dyld's
    /// return-to-host/thread-exit sentinels).
    fn validated_guest_frame_record(&self) -> Option<GuestFrameRecord> {
        let stack_range = self.threads.get(self.current_thread)?.stack.clone()?;
        let fp = self.cpu.regs()[abi::FRAME_POINTER];
        let sp = self.cpu.regs()[cpu::Cpu::SP];

        // The record must be at or above SP: a frame pointer below the stack
        // pointer is not the live frame (e.g. a stale r7 left behind after a
        // function that doesn't maintain frame pointers reused the register).
        if fp < sp {
            return None;
        }
        // The diagnostic frame `call_from_host` builds must not be popped by
        // us: its saved LR is the interrupted guest LR, not a return address
        // through the host boundary.
        if self.is_host_to_guest_stack_frame(fp) {
            return None;
        }
        let caller_sp = libc::cxxabi::frame_record_caller_sp(&stack_range, fp)?;
        let saved_fp = self.read_guest_u32_fallible(fp)?;
        let saved_lr = self.read_guest_u32_fallible(fp.wrapping_add(4))?;

        let saved_fp_is_valid = saved_fp == 0
            || (saved_fp > fp
                && libc::cxxabi::frame_record_caller_sp(&stack_range, saved_fp).is_some());
        if !saved_fp_is_valid {
            return None;
        }
        if !self.is_guest_code_address(saved_lr) {
            return None;
        }
        Some(GuestFrameRecord {
            fp,
            saved_fp,
            saved_lr,
            caller_sp,
        })
    }

    /// Decode the call instruction that produced the return address `lr`
    /// and report whether its branch target is consistent with `lr` being
    /// the return address *of the function that contains `fault_pc`*.
    ///
    /// Returns `Some(false)` when the call site definitely went somewhere
    /// else — i.e. LR is a stale value left behind by an earlier call that
    /// already returned, and "returning" to it would land in the middle of
    /// the function that trapped. Returns `None` when the call was indirect
    /// (`blx Rm`) or no call instruction precedes `lr`.
    fn lr_is_consistent_with_fault(&self, lr: u32, fault_pc: u32) -> Option<bool> {
        let (call_site, target) = decode_direct_call_before(
            lr,
            |addr| self.read_guest_u16_fallible(addr),
            |addr| self.read_guest_u32_fallible(addr),
        )?;
        Some(direct_call_is_consistent_with_fault(
            call_site, target, fault_pc,
        ))
    }

    /// If `addr` is inside a symbol stub section, the name of the symbol the
    /// stub jumps to (i.e. the dynamically linked function being called).
    fn symbol_stub_name_at(&self, addr: u32) -> Option<&str> {
        let addr = addr & !1;
        self.bins.iter().find_map(|bin| {
            bin.sections.iter().find_map(|section| {
                if section.type_ != mach_o::SectionType::SymbolStubs
                    || addr < section.addr
                    || addr.wrapping_sub(section.addr) >= section.size
                {
                    return None;
                }
                let info = section.dyld_indirect_symbol_info.as_ref()?;
                let index = (addr - section.addr) / info.entry_size;
                info.indirect_undef_symbols
                    .get(index as usize)?
                    .as_deref()
            })
        })
    }

    /// Human-readable description of the call that produced the return
    /// address `lr`, for trap diagnostics: where the call is, where it went,
    /// and (if the target is a symbol stub) which linked function that is.
    fn describe_call_site_before(&self, lr: u32) -> String {
        match decode_direct_call_before(
            lr,
            |addr| self.read_guest_u16_fallible(addr),
            |addr| self.read_guest_u32_fallible(addr),
        ) {
            Some((call_site, target)) => match self.symbol_stub_name_at(target) {
                Some(name) => format!(
                    "LR comes from a call at {call_site:#x} to {target:#x}, \
                     which is the symbol stub for {name}"
                ),
                None => format!("LR comes from a call at {call_site:#x} to {target:#x}"),
            },
            None => "LR does not follow a direct bl/blx <label> (indirect call or not a \
                     return address)"
                .to_string(),
        }
    }

    /// Hex dump of the guest halfwords around `pc` (Thumb) or words (ARM),
    /// for trap diagnostics. The faulting location is marked with brackets.
    fn dump_guest_code_around(&self, pc: u32, thumb: bool) -> String {
        let mut out = String::new();
        let start = if thumb {
            pc.wrapping_sub(16) & !1
        } else {
            pc.wrapping_sub(16) & !3
        };
        if thumb {
            for i in 0..16u32 {
                let addr = start.wrapping_add(i * 2);
                let text = match self.read_guest_u16_fallible(addr) {
                    Some(hw) => format!("{hw:04x}"),
                    None => "????".to_string(),
                };
                if addr == pc {
                    out.push_str(&format!("[{text}] "));
                } else {
                    out.push_str(&text);
                    out.push(' ');
                }
            }
        } else {
            for i in 0..8u32 {
                let addr = start.wrapping_add(i * 4);
                let text = match self.read_guest_u32_fallible(addr) {
                    Some(word) => format!("{word:08x}"),
                    None => "????????".to_string(),
                };
                if addr == pc {
                    out.push_str(&format!("[{text}] "));
                } else {
                    out.push_str(&text);
                    out.push(' ');
                }
            }
        }
        format!("bytes from {start:#x}: {}", out.trim_end())
    }

    /// Choose how to get the guest past a deliberate trap at `pc` (an
    /// instruction inside a code section). `lr_bypass_is_not_progressing`
    /// is set once branching to LR has already been tried several times for
    /// this exact (PC, LR) pair without getting anywhere. This only inspects
    /// state; [Self::apply_guest_trap_recovery] performs the change.
    fn plan_guest_trap_recovery(
        &self,
        pc: u32,
        lr: u32,
        lr_bypass_is_not_progressing: bool,
    ) -> GuestTrapRecovery {
        let frame = self.validated_guest_frame_record();

        // If the trapping function pushed its own `{r7, lr}` record and has
        // not called anything since, the saved LR equals the live LR. Popping
        // the record is then exactly a return from the trapping function —
        // including restoring SP and r7, which a bare branch to LR does not.
        let current_function_owns_frame = frame.is_some_and(|f| f.saved_lr == lr);

        // LR is stale if it isn't a return address at all, or if the call that
        // produced it targeted some other function (which has since returned).
        // Branching to a stale LR re-executes the trapping function from the
        // middle, which typically leads straight back to the same trap.
        let lr_is_stale = !self.is_guest_code_address(lr)
            || self.lr_is_consistent_with_fault(lr, pc) == Some(false);

        if let Some(frame) = frame {
            if current_function_owns_frame {
                return GuestTrapRecovery::UnwindFrame {
                    frame,
                    reason: "the trapping function owns the frame record",
                };
            }
            if lr_is_stale {
                return GuestTrapRecovery::UnwindFrame {
                    frame,
                    reason: "LR is stale (not this function's return address)",
                };
            }
            if lr_bypass_is_not_progressing {
                return GuestTrapRecovery::UnwindFrame {
                    frame,
                    reason: "branching to LR keeps re-trapping at the same site",
                };
            }
        }

        if (lr & !1) == pc {
            // Pathological self-loop: LR points right back at the trap we
            // just hit (e.g. `bl noreturn_function` whose host stub returned,
            // followed by a trap). Branching to LR would re-enter it.
            return GuestTrapRecovery::SkipInstruction {
                reason: "LR re-enters the same instruction",
            };
        }

        GuestTrapRecovery::ReturnToLr
    }

    /// Perform the CPU-state change chosen by [Self::plan_guest_trap_recovery]
    /// for a trap at `pc` of length `instruction_len`.
    fn apply_guest_trap_recovery(
        &mut self,
        recovery: GuestTrapRecovery,
        pc: u32,
        instruction_len: u32,
    ) {
        match recovery {
            GuestTrapRecovery::ReturnToLr => {
                // Instead of skipping forward through garbage data, pretend
                // the faulting function returned to its caller.
                let lr = self.cpu.regs()[cpu::Cpu::LR];
                self.cpu.branch(GuestFunction::from_addr_with_thumb_bit(lr));
            }
            GuestTrapRecovery::UnwindFrame { frame, .. } => {
                // The resumed function's own return address is in the next
                // record up the chain. Restore it into LR too (like
                // `unwind_to_app_frame` does) so a `bx lr`-style epilogue
                // can't jump back into the frame we just abandoned; if it
                // can't be validated, thread exit is the safe terminal
                // continuation.
                let return_to_host = self.dyld.return_to_host_routine().addr_with_thumb_bit();
                let thread_exit = self.dyld.thread_exit_routine().addr_with_thumb_bit();
                let caller_lr = if frame.saved_fp == 0 {
                    thread_exit
                } else {
                    match self.read_guest_u32_fallible(frame.saved_fp.wrapping_add(4)) {
                        Some(candidate)
                            if candidate == return_to_host
                                || candidate == thread_exit
                                || self.is_guest_code_address(candidate) =>
                        {
                            candidate
                        }
                        _ => thread_exit,
                    }
                };
                let regs = self.cpu.regs_mut();
                regs[abi::FRAME_POINTER] = frame.saved_fp;
                regs[cpu::Cpu::SP] = frame.caller_sp;
                regs[cpu::Cpu::LR] = caller_lr;
                regs[0] = 0;
                self.cpu
                    .branch(GuestFunction::from_addr_with_thumb_bit(frame.saved_lr));
            }
            GuestTrapRecovery::UnwindStack {
                return_addr,
                caller_sp,
                ..
            } => {
                // LR and the frame record are both unusable (the jump came
                // from garbage), so take the return address that the call
                // left on the stack and resume the caller after its call.
                // LR is pointed at the thread-exit routine so a `bx lr`
                // epilogue in that caller cannot jump back into the mess.
                let thread_exit = self.dyld.thread_exit_routine().addr_with_thumb_bit();
                let regs = self.cpu.regs_mut();
                regs[cpu::Cpu::SP] = caller_sp;
                regs[cpu::Cpu::LR] = thread_exit;
                regs[abi::FRAME_POINTER] = 0;
                regs[0] = 0;
                self.cpu
                    .branch(GuestFunction::from_addr_with_thumb_bit(return_addr));
            }
            GuestTrapRecovery::SkipInstruction { .. } => {
                // Advance past the faulting instruction so the guest makes
                // forward progress, and clear the bypass counters since we're
                // no longer bypassing the same site.
                self.cpu.regs_mut()[cpu::Cpu::PC] = pc.wrapping_add(instruction_len);
                self.cpu_error_bypass_last = None;
                self.cpu_error_bypass_count = 0;
                self.cpu_error_bypass_last_lr = None;
                self.cpu_error_bypass_lr_count = 0;
            }
        }
    }

    #[cold]
    /// Let the debugger handle a CPU error. Without one, use bounded
    /// best-effort recovery for guest traps; unhandled cases panic.
    fn debug_cpu_error(&mut self, error: cpu::CpuError) {
        let is_thumb = (self.cpu.cpsr() & cpu::Cpu::CPSR_THUMB) != 0;
        // The instruction length this code historically *assumed*: 2 bytes in
        // Thumb state, 4 in ARM state. Thumb-2 instructions can be 4 bytes
        // long though, so this is only a fallback (see below).
        let assumed_instruction_len: u32 = if is_thumb { 2 } else { 4 };

        let is_undefined_instruction = matches!(error, cpu::CpuError::UndefinedInstruction);
        let is_recoverable = Self::is_recoverable_guest_cpu_error(&error);

        // After an UndefinedInstruction/Breakpoint, dynarmic leaves PC on the
        // *next* instruction. The legacy rewind (`next_pc - 2` in Thumb state)
        // lands in the middle of a 32-bit Thumb-2 encoding; the Android
        // workarounds below were tuned against that value, so keep it for
        // them, but use dynarmic's exact fault address for everything else.
        let next_pc = self.cpu.regs()[cpu::Cpu::PC];
        let legacy_pc = next_pc.wrapping_sub(assumed_instruction_len);
        let (fault_pc, instruction_len) = if is_recoverable {
            let exact_pc = self.cpu.last_exception_pc();
            match next_pc.wrapping_sub(exact_pc) {
                len @ (2 | 4) if is_thumb || len == 4 => (exact_pc, len),
                _ => (legacy_pc, assumed_instruction_len),
            }
        } else {
            (next_pc, assumed_instruction_len)
        };

        if is_recoverable {
            // Rewind the PC so that it's at the instruction where the error
            // occurred, rather than the next instruction. This is necessary for
            // GDB to detect its software breakpoints. For some reason this
            // isn't correct for memory errors however.
            self.cpu.regs_mut()[cpu::Cpu::PC] = fault_pc;
        }

        // Some NEON instructions are missing from dynarmic and raise an
        // UndefinedInstruction. Perform them in software and carry on.
        if is_undefined_instruction && self.is_guest_code_address(fault_pc) {
            let guest_mem = &self.mem;
            let read_u16 = |addr: u32| Some(guest_mem.read(mem::ConstPtr::<u16>::from_bits(addr)));
            if let Some(len) =
                cpu::asimd_fallback::emulate(fault_pc, is_thumb, read_u16, self.cpu.extregs_mut())
            {
                self.cpu.regs_mut()[cpu::Cpu::PC] = fault_pc.wrapping_add(len);
                return;
            }
        }

        if self.gdb_server.is_none() {
            // Bypass guest traps without framework stubs. Games can hit an
            // UndefinedInstruction (abort/__builtin_trap) or a Breakpoint
            // (`bkpt`) when an API returns nil or unexpected data. Fake a
            // function return to LR to keep execution going.
            //
            // However: if we hit the SAME (PC,LR) pair too many times in a row
            // this indicates we are looping forever (LR itself points back
            // through an infinite chain of guest traps). In that case, the
            // recovery logic below skips the faulting instruction.
            if is_recoverable {
                // NOTE: `pc` here is the legacy rewind value the Android
                // workarounds below expect (`next_pc - 2` in Thumb state,
                // which may be the *second* halfword of a 32-bit Thumb-2
                // instruction). The generic recovery further down uses the
                // exact `fault_pc` instead.
                let pc = legacy_pc;
                let lr = self.cpu.regs()[cpu::Cpu::LR];
                if is_undefined_instruction
                    && crate::env_flag_cached!("TOUCHHLE_TRACE_UDF_REGS")
                {
                    static TRACE_COUNT: std::sync::atomic::AtomicUsize =
                        std::sync::atomic::AtomicUsize::new(0);
                    if pc >= 0x700000
                        && TRACE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 3
                    {
                        let regs = self.cpu.regs();
                        log_no_panic!(
                            "UDF-REGS pc={:#x} lr={:#x} cpsr={:#x} r0={:#x} r1={:#x} r2={:#x} r3={:#x} r4={:#x} r5={:#x} r6={:#x} r7={:#x} r8={:#x} r9={:#x} r10={:#x} r11={:#x} r12={:#x} sp={:#x}",
                            pc,
                            lr,
                            self.cpu.cpsr(),
                            regs[0],
                            regs[1],
                            regs[2],
                            regs[3],
                            regs[4],
                            regs[5],
                            regs[6],
                            regs[7],
                            regs[8],
                            regs[9],
                            regs[10],
                            regs[11],
                            regs[12],
                            regs[cpu::Cpu::SP],
                        );
                        self.stack_trace_current();
                    }
                }
                // Potato Story Android hard fallback applies only to UDFs:
                //
                // The generic decoder did not match on-device, but Android
                // repeatedly reports UDF at these exact Thumb-2 sites while
                // desktop runs through them. Force the constant-load results
                // and advance PC like the desktop path effectively does.
                if is_undefined_instruction
                    && cfg!(target_os = "android")
                    && (self.cpu.cpsr() & cpu::Cpu::CPSR_THUMB) != 0
                {
                    match pc {
                        // 0x9ec2: MOVW r0, #0xa136
                        // 0x9ec6: MOVT r0, #0x0030
                        0x9ec6 => {
                            let old = self.cpu.regs()[0];
                            let new_value = (old & 0x0000_ffff) | 0x0030_0000;
                            log_no_panic!(
                                "Potato Story Android hard fallback: MOVT r0 at 0x9ec6: {:#x} -> {:#x}; PC=0x9eca",
                                old,
                                new_value
                            );
                            self.cpu.regs_mut()[0] = new_value;
                            self.cpu.regs_mut()[cpu::Cpu::PC] = 0x9eca;
                            self.cpu_error_bypass_last = None;
                            self.cpu_error_bypass_count = 0;
                            return;
                        }

                        // 0xabce: MOVW r1, #0xa0ea
                        0xabce => {
                            log_no_panic!(
                                "Potato Story Android hard fallback: MOVW r1 at 0xabce -> 0xa0ea; PC=0xabd2"
                            );
                            self.cpu.regs_mut()[1] = 0x0000_a0ea;
                            self.cpu.regs_mut()[cpu::Cpu::PC] = 0xabd2;
                            self.cpu_error_bypass_last = None;
                            self.cpu_error_bypass_count = 0;
                            return;
                        }

                        // 0xacd4: MOVT r12, #0x0030
                        // Dynarmic reports/logs the second halfword at 0xacd6.
                        0xacd6 => {
                            let old = self.cpu.regs()[12];
                            let new_value = (old & 0x0000_ffff) | 0x0030_0000;
                            log_no_panic!(
                                "Potato Story Android hard fallback: MOVT r12 at 0xacd4/0xacd6: {:#x} -> {:#x}; PC=0xacd8",
                                old,
                                new_value
                            );
                            self.cpu.regs_mut()[12] = new_value;
                            self.cpu.regs_mut()[cpu::Cpu::PC] = 0xacd8;
                            self.cpu_error_bypass_last = None;
                            self.cpu_error_bypass_count = 0;
                            return;
                        }

                        // 0xadae is another one-off Android trap in the same
                        // startup cluster. Advance past the 32-bit Thumb-2
                        // instruction instead of fake-returning to LR.
                        0xadae => {
                            log_no_panic!(
                                "Potato Story Android hard fallback: skipping trapped Thumb-2 instruction at 0xadae; PC=0xadb2"
                            );
                            self.cpu.regs_mut()[cpu::Cpu::PC] = 0xadb2;
                            self.cpu_error_bypass_last = None;
                            self.cpu_error_bypass_count = 0;
                            return;
                        }

                        _ => {}
                    }
                }

                // Android/Dynarmic workaround:
                //
                // Potato Story hits UndefinedInstruction on valid Thumb-2
                // MOVW/MOVT constant-load instructions on Android, while the
                // same code runs on desktop. Do what the desktop path does:
                // materialize the immediate into the destination register and
                // advance past the 32-bit Thumb-2 instruction instead of
                // fake-returning to LR and looping forever.
                //
                // The PC we log here has already been rewound by the generic
                // Thumb path above, which assumes 2-byte Thumb instructions.
                // Thumb-2 instructions are 4 bytes, so try both PC and PC-2.
                if is_undefined_instruction
                    && cfg!(target_os = "android")
                    && (self.cpu.cpsr() & cpu::Cpu::CPSR_THUMB) != 0
                {
                    // Android/Dynarmic sometimes reports the fault PC a few
                    // bytes before/after the real 32-bit Thumb-2 instruction.
                    // Scan nearby even halfword starts instead of only pc/pc-2.
                    for delta in [-8i32, -6, -4, -2, 0, 2, 4, 6, 8] {
                        let start = if delta < 0 {
                            pc.wrapping_sub((-delta) as u32)
                        } else {
                            pc.wrapping_add(delta as u32)
                        };

                        if start & 1 != 0 {
                            continue;
                        }

                        let hw1: u16 = self.mem.read(mem::ConstPtr::<u16>::from_bits(start));
                        let hw2: u16 = self
                            .mem
                            .read(mem::ConstPtr::<u16>::from_bits(start.wrapping_add(2)));

                        // Potato Story on Android also trips on Thumb-2
                        // VFP/coprocessor-looking instructions immediately
                        // after the constant-load clusters. Do not fake-return
                        // from the whole function; advance past the trapped
                        // 32-bit instruction and let scene setup continue.
                        if std::env::var_os("TOUCHHLE_POTATO_ANDROID_THUMB2_COMPAT").is_some()
                            && matches!(hw1 & 0xfe00, 0xec00 | 0xee00)
                        {
                            log_no_panic!(
                                "Potato Story Android Thumb-2 compat: skipping coprocessor/VFP-looking instruction at {:#x} (reported PC {:#x}, hw1={:#06x}, hw2={:#06x}); advancing to {:#x}",
                                start,
                                pc,
                                hw1,
                                hw2,
                                start.wrapping_add(4)
                            );

                            self.cpu.regs_mut()[cpu::Cpu::PC] = start.wrapping_add(4);
                            self.cpu_error_bypass_last = None;
                            self.cpu_error_bypass_count = 0;
                            return;
                        }

                        // Thumb-2 MOVW/MOVT immediate encodings. The mask
                        // keeps the opcode bits and ignores immediate bits.
                        // Examples from Potato Story:
                        //   bytes 4a f2 36 10 => hw1=f24a, hw2=1036, MOVW
                        //   bytes c0 f2 30 00 => hw1=f2c0, hw2=0030, MOVT
                        //   bytes 4a f6 ea 01 => hw1=f64a, hw2=01ea, MOVW
                        let is_movw = (hw1 & 0xfbf0) == 0xf240 && (hw2 & 0x8000) == 0;
                        let is_movt = (hw1 & 0xfbf0) == 0xf2c0 && (hw2 & 0x8000) == 0;

                        if is_movw || is_movt {
                            let imm4 = (hw1 & 0x000f) as u32;
                            let i = ((hw1 >> 10) & 1) as u32;
                            let imm3 = ((hw2 >> 12) & 0x7) as u32;
                            let rd = ((hw2 >> 8) & 0xf) as usize;
                            let imm8 = (hw2 & 0x00ff) as u32;
                            let imm16 = (imm4 << 12) | (i << 11) | (imm3 << 8) | imm8;

                            // MOVW/MOVT to PC is not a normal case here; if it
                            // ever appears, let the existing error path handle
                            // it instead of inventing branch semantics.
                            if rd == cpu::Cpu::PC {
                                continue;
                            }

                            let old = self.cpu.regs()[rd];
                            let new_value = if is_movt {
                                (old & 0x0000_ffff) | (imm16 << 16)
                            } else {
                                imm16
                            };

                            log_no_panic!(
                                "Android Thumb-2 compat: emulated {} at {:#x}: r{} {:#x} -> {:#x}; advancing to {:#x}",
                                if is_movt { "MOVT" } else { "MOVW" },
                                start,
                                rd,
                                old,
                                new_value,
                                start.wrapping_add(4)
                            );

                            self.cpu.regs_mut()[rd] = new_value;
                            self.cpu.regs_mut()[cpu::Cpu::PC] = start.wrapping_add(4);
                            self.cpu_error_bypass_last = None;
                            self.cpu_error_bypass_count = 0;
                            return;
                        }
                    }
                }

                // ---- Generic guest-trap recovery ----
                //
                // From here on use dynarmic's exact fault address; the legacy
                // `pc` above may point into the middle of a Thumb-2 encoding.
                let pc = fault_pc;
                let trap_kind = self.classify_guest_trap(pc, is_thumb, instruction_len);

                // A real instruction in a code section that dynarmic simply
                // could not decode (unsupported/unimplemented encoding) is
                // not an abort: the function it sits in is meant to keep
                // running. Step over it as a no-op. This is deliberately kept
                // out of the bypass counters below: an unsupported
                // instruction in a hot loop would otherwise trip the
                // same-LR runaway panic even though the guest is making
                // perfectly good progress.
                //
                // Stepping over *one* such instruction is right; stepping
                // over hundreds of different ones is not. A real function
                // does not contain that many instructions dynarmic cannot
                // decode, so past `MAX_UNDECODABLE_SITES` distinct sites the
                // PC is evidently running through a data blob — the case
                // `classify_guest_trap` reports statically as
                // `GuestTrapKind::ExecutionInData` — and the recovery below
                // has to get the guest out of it instead. This is the safety
                // net for a misclassification: it bounds the damage if that
                // static check misses one.
                const MAX_UNDECODABLE_SITES: usize = 128;
                // How many distinct sites get the full diagnostics dump and
                // stack trace. A guest walking through a data blob reaches a
                // new site every few instructions, so one line per site would
                // flood the log.
                const MAX_UNDECODABLE_SITE_DIAGNOSTICS: usize = 8;
                if let GuestTrapKind::UndecodableInstruction { encoding } = trap_kind {
                    let site_count = self
                        .cpu_skipped_instruction_sites
                        .entry(pc)
                        .or_insert(0);
                    *site_count = site_count.saturating_add(1);
                    let site_count = *site_count;
                    let distinct_sites = self.cpu_skipped_instruction_sites.len();
                    if distinct_sites > MAX_UNDECODABLE_SITES {
                        if site_count == 1 {
                            log_no_panic!(
                                "Warning: Undecodable instruction at {:#x} (encoding \
                                 {:#x}) is one of {} distinct sites stepped over in \
                                 this run, so the PC is being treated as running \
                                 through data rather than code. Recovering the call \
                                 instead of skipping it. LR={:#x}.",
                                pc,
                                encoding,
                                distinct_sites,
                                lr
                            );
                        }
                    } else {
                        if site_count == 1 || site_count == 1024 || site_count == 1 << 20 {
                            log_no_panic!(
                                "Warning: {:?} at {:#x} (encoding {:#x}, thumb={}, {} bytes) \
                                 is inside a code section but is not a trap instruction, \
                                 so dynarmic could not decode it. Skipping it as a no-op \
                                 and continuing at {:#x}. LR={:#x}. (seen {} times at this \
                                 site)",
                                error,
                                pc,
                                encoding,
                                is_thumb,
                                instruction_len,
                                pc.wrapping_add(instruction_len),
                                lr,
                                site_count
                            );
                        }
                        if site_count == 1 {
                            if distinct_sites <= MAX_UNDECODABLE_SITE_DIAGNOSTICS {
                                log_no_panic!(
                                    "Undecodable instruction diagnostics: {}. {}.",
                                    self.dump_guest_code_around(pc, is_thumb),
                                    self.describe_call_site_before(lr)
                                );
                                self.stack_trace_current();
                            } else if distinct_sites == MAX_UNDECODABLE_SITE_DIAGNOSTICS + 1 {
                                log_no_panic!(
                                    "Warning: Undecodable instructions at {} distinct \
                                     sites so far; suppressing further per-site \
                                     diagnostics and stack traces.",
                                    distinct_sites
                                );
                            }
                        }
                        self.cpu.regs_mut()[cpu::Cpu::PC] = pc.wrapping_add(instruction_len);
                        return;
                    }
                }

                // Track repeated occurrences of the same bypass site.
                const BYPASS_LIMIT: u32 = 256;
                const LOG_RATE: u32 = 32;
                let key = (pc, lr);
                let count = if self.cpu_error_bypass_last == Some(key) {
                    self.cpu_error_bypass_count = self.cpu_error_bypass_count.saturating_add(1);
                    self.cpu_error_bypass_count
                } else {
                    self.cpu_error_bypass_last = Some(key);
                    self.cpu_error_bypass_count = 1;
                    1
                };
                self.cpu_error_bypass_total = self.cpu_error_bypass_total.saturating_add(1);
                let total = self.cpu_error_bypass_total;

                // Independently track how many times in a row we've faked a
                // return to the SAME LR, ignoring the faulting PC. The `(pc,
                // lr)` counter above resets to 1 whenever the faulting PC
                // changes, so a guest that keeps calling through a bad/nil
                // function pointer from a single call site — trapping at a
                // handful of *different* garbage addresses but always
                // returning to the same LR — never trips `BYPASS_LIMIT` and
                // the emulator wedges forever.
                //
                // This is exactly the Rush Rally 2 startup hang: the faulting
                // PC alternates between 0x4000 and 0x36c6ec30 while LR stays
                // 0x2639a3, so the pair counter oscillates around 1 and the
                // process spins until it's killed. Bounding the number of
                // consecutive same-LR fake returns turns that infinite hang
                // into a clean, actionable panic. We allow a larger budget
                // here than `BYPASS_LIMIT` so genuinely recoverable cases
                // (which do make forward progress and eventually settle on a
                // stable LR) are unaffected.
                const LR_BYPASS_LIMIT: u32 = 4096;
                let lr_count = if self.cpu_error_bypass_last_lr == Some(lr) {
                    self.cpu_error_bypass_lr_count = self.cpu_error_bypass_lr_count.saturating_add(1);
                    self.cpu_error_bypass_lr_count
                } else {
                    self.cpu_error_bypass_last_lr = Some(lr);
                    self.cpu_error_bypass_lr_count = 1;
                    1
                };

                if lr_count >= LR_BYPASS_LIMIT {
                    panic!(
                        "{error:?} bypass faked a return to LR={:#x} \
                         {} times in a row (most recent faulting PC {:#x}); \
                         giving up to avoid hanging. The guest is repeatedly \
                         calling through a bad/nil function pointer from a \
                         single call site — usually a framework stub that \
                         returned a bogus object the game then dereferences \
                         as a function.",
                        lr, lr_count, pc
                    );
                }

                // Decide how to recover *before* logging so the log says what
                // actually happened.
                //
                // For a deliberate trap (`udf`/`trap`/`bkpt`) inside real
                // code we can reason about the frame: the historical "branch
                // to LR" is only correct if the trapping function has not
                // pushed a frame and LR really is its return address. It is
                // wrong in two common situations:
                //
                //  * LR is stale. Example (Asphalt 8 startup): a function
                //    calls something at 0x1e3c2 (LR=0x1e3c7), that call
                //    returns normally, and ~0x6b bytes later the *same*
                //    function hits a `trap` at 0x1e432. LR still holds
                //    0x1e3c7, so "returning" to it just re-runs the same
                //    code and re-traps — 256 identical bypasses in a row,
                //    then falling off the trap into whatever follows it.
                //  * The trapping function has pushed `{r7, lr}`. Branching to
                //    LR without restoring SP/r7 leaves the caller running on
                //    the callee's frame; its epilogue then pops the callee's
                //    record and "returns" into itself a second time.
                //
                // In both cases popping the validated frame record at r7
                // (restore r7 and SP, return 0 to the saved LR) is the
                // correct recovery, mirroring what `abort()`/`exit()`
                // recovery already does in `libc::cxxabi::unwind_to_app_frame`.
                //
                // The static call-site analysis cannot see through indirect
                // calls (`blx Rm`), so there is also a dynamic escalation:
                // once the very same (PC, LR) pair has been bypassed
                // `UNWIND_AFTER_REPEATS` times, branching to LR has evidently
                // not moved the guest forward, and the frame is popped
                // instead (if one can be validated) long before the
                // `BYPASS_LIMIT` fall-through-the-trap last resort.
                //
                // Outside code sections (wild PC), and inside them but in a
                // data blob rather than an instruction stream, the
                // instruction bytes and call-site analysis are meaningless,
                // so only the dynamic escalation applies on top of the legacy
                // branch-to-LR.
                const UNWIND_AFTER_REPEATS: u32 = 4;
                if let GuestTrapKind::DeliberateTrap { .. } = trap_kind {
                    let sp = self.cpu.regs()[cpu::Cpu::SP];
                    self.last_deliberate_trap_bypass = Some((self.current_thread, sp));
                }
                // Number of wild-PC bypasses after which "fake return to LR"
                // is assumed to be the thing keeping the guest stuck.
                const WILD_PC_UNWIND_AFTER: u32 = 32;
                let recovery = match trap_kind {
                    GuestTrapKind::DeliberateTrap { .. }
                    | GuestTrapKind::MisalignedInstruction { .. } => {
                        self.plan_guest_trap_recovery(pc, lr, count >= UNWIND_AFTER_REPEATS)
                    }
                    GuestTrapKind::OutsideCode | GuestTrapKind::ExecutionInData { .. }
                        if total >= WILD_PC_UNWIND_AFTER
                            && self.lr_is_consistent_with_fault(lr, pc) != Some(true) =>
                    {
                        // LR is not a plausible return address (or there is
                        // no call instruction before it), so "return to LR"
                        // is what bounces the guest between garbage PCs.
                        // Find the real return address on the stack instead.
                        let sp = self.cpu.regs()[cpu::Cpu::SP];
                        match self.validated_stack_return_address(sp) {
                            Some((return_addr, caller_sp)) => GuestTrapRecovery::UnwindStack {
                                return_addr,
                                caller_sp,
                                reason: "LR does not look like a return address and \
                                         branching to it keeps leaving the code",
                            },
                            None => GuestTrapRecovery::ReturnToLr,
                        }
                    }
                    GuestTrapKind::OutsideCode | GuestTrapKind::ExecutionInData { .. }
                        if (lr & !1) == pc =>
                    {
                        GuestTrapRecovery::SkipInstruction {
                            reason: "LR re-enters the same instruction",
                        }
                    }
                    GuestTrapKind::OutsideCode | GuestTrapKind::ExecutionInData { .. }
                        if count >= UNWIND_AFTER_REPEATS =>
                    {
                        match self.validated_guest_frame_record() {
                            Some(frame) => GuestTrapRecovery::UnwindFrame {
                                frame,
                                reason: "branching to LR keeps re-trapping at the same site",
                            },
                            None => match self
                                .validated_stack_return_address(self.cpu.regs()[cpu::Cpu::SP])
                            {
                                Some((return_addr, caller_sp)) => GuestTrapRecovery::UnwindStack {
                                    return_addr,
                                    caller_sp,
                                    reason: "branching to LR keeps re-trapping at the same site \
                                             and the frame-pointer chain is unusable",
                                },
                                None => GuestTrapRecovery::ReturnToLr,
                            },
                        }
                    }
                    _ => GuestTrapRecovery::ReturnToLr,
                };

                // Log the first sight of each new bypass site (and the point
                // where the recovery strategy may escalate), but keyed on the
                // *total* count so guests that cycle through many distinct
                // (PC, LR) pairs stop flooding the log after LOG_RATE lines.
                if total <= LOG_RATE
                    && (count == 1 || count == UNWIND_AFTER_REPEATS || count % LOG_RATE == 0)
                {
                    let trap_description = match trap_kind {
                        GuestTrapKind::DeliberateTrap { encoding } => {
                            format!("deliberate trap instruction, encoding {encoding:#x}")
                        }
                        GuestTrapKind::UndecodableInstruction { encoding } => {
                            format!("undecodable instruction, encoding {encoding:#x}")
                        }
                        GuestTrapKind::MisalignedInstruction { encoding } => format!(
                            "PC is the second halfword of a bl/blx at {:#x}, execution is \
                             misaligned with the instruction stream; encoding {encoding:#x}",
                            pc.wrapping_sub(2)
                        ),
                        GuestTrapKind::ExecutionInData { encoding } => format!(
                            "PC is inside a code section but the bytes there are not an \
                             instruction stream — a literal pool, switch table or other \
                             data; encoding {encoding:#x}"
                        ),
                        GuestTrapKind::OutsideCode => "PC is outside any code section".to_string(),
                    };
                    let recovery_description = match recovery {
                        GuestTrapRecovery::ReturnToLr => {
                            format!("Faking function return to LR ({lr:#x}) to bypass the guest trap.")
                        }
                        GuestTrapRecovery::UnwindFrame { frame, reason } => format!(
                            "Unwinding the frame record at r7={:#x} (saved r7={:#x}, \
                             saved LR={:#x}, caller SP={:#x}) and returning 0 to bypass \
                             the guest trap, because {}.",
                            frame.fp, frame.saved_fp, frame.saved_lr, frame.caller_sp, reason
                        ),
                        GuestTrapRecovery::UnwindStack {
                            return_addr,
                            caller_sp,
                            reason,
                        } => format!(
                            "Unwinding the return address {:#x} from the guest stack \
                             (caller SP={:#x}) and returning 0 to bypass the guest \
                             trap, because {}.",
                            return_addr, caller_sp, reason
                        ),
                        GuestTrapRecovery::SkipInstruction { reason } => format!(
                            "Skipping the faulting instruction (continuing at {:#x}), because {}.",
                            pc.wrapping_add(instruction_len),
                            reason
                        ),
                    };
                    log_no_panic!(
                        "Warning: Ignored {:?} at {:#x} ({}). {} \
                         LR={:#x} cpsr={:#x} thumb={} instruction_len={} \
                         (occurrence {} of at most {})",
                        error,
                        pc,
                        trap_description,
                        recovery_description,
                        lr,
                        self.cpu.cpsr(),
                        is_thumb,
                        instruction_len,
                        count,
                        BYPASS_LIMIT
                    );
                    if count == 1 {
                        log_no_panic!(
                            "Guest trap diagnostics: {}. {}.",
                            self.dump_guest_code_around(pc, is_thumb),
                            self.describe_call_site_before(lr)
                        );
                        if trap_kind != GuestTrapKind::OutsideCode {
                            self.stack_trace_current();
                        }
                    }
                } else if total == LOG_RATE + 1 {
                    log_no_panic!(
                        "Warning: Ignored guest-trap bypass still active \
                         ({} total so far); suppressing further per-site lines until \
                         a new call site appears. Latest PC {:#x}, LR {:#x}.",
                        total,
                        pc,
                        lr
                    );
                }

                if count >= BYPASS_LIMIT {
                    // The same (PC, LR) pair has trapped BYPASS_LIMIT times.
                    // The chosen recovery clearly does not help — the caller
                    // keeps re-entering the faulting site (usually a framework
                    // stub that returned bogus data the guest re-calls into).
                    // Rather than killing the whole emulator, degrade
                    // gracefully: skip past the faulting instruction (treat
                    // the trap as a no-op) so execution continues in the
                    // caller's body, and reset the counters so a later,
                    // different loop still gets a fresh budget. A genuine
                    // infinite hang is still bounded by LR_BYPASS_LIMIT
                    // above and by the per-batch forward-progress reset in
                    // `handle_cpu_state`.
                    if count == BYPASS_LIMIT || count % (BYPASS_LIMIT * 4) == 0 {
                        log_no_panic!(
                            "Warning: {:?} at {:#x} looped {} times with LR={:#x}. \
                             Bypassing it is not making progress, so skipping \
                             the faulting instruction instead. This usually means \
                             a framework stub returned data the guest keeps \
                             re-trapping on.",
                            error,
                            pc,
                            count,
                            lr
                        );
                    }
                    self.cpu.regs_mut()[cpu::Cpu::PC] = pc.wrapping_add(instruction_len);
                    self.cpu_error_bypass_last = None;
                    self.cpu_error_bypass_count = 0;
                    // Deliberately NOT resetting `cpu_error_bypass_last_lr`
                    // and `cpu_error_bypass_lr_count` here. Those two are the
                    // only bound on a guest that keeps trapping at one site,
                    // and wiping them on every BYPASS_LIMIT-th skip restarted
                    // the (PC, LR) counter from zero forever: `LR_BYPASS_LIMIT`
                    // could never be reached and the thread spun indefinitely,
                    // printing "looped 256 times" hundreds of times over
                    // (Asphalt 8 drift event, PC 0x2a00c / LR 0x29fe1). Real
                    // forward progress still clears them in `handle_cpu_state`,
                    // so a genuinely recovering guest is unaffected.
                    return;
                }

                self.apply_guest_trap_recovery(recovery, pc, instruction_len);
                return;
            }

            // A memory abort raised by the one-shot null-write probe (see
            // `touchHLE_cpu_write_impl`). This is the only way to learn which
            // guest instruction stores through NULL, so report it and carry
            // on: the store already reached the null page, and later nil
            // writes are absorbed without probing again.
            if matches!(error, cpu::CpuError::MemoryError) && crate::mem::null_write_probe_pending()
            {
                crate::mem::null_write_probe_clear();
                log_no_panic!(
                    "NULL-WRITE PROBE: the guest stored into the null page. \
                     PC={:#x} LR={:#x} (thread {}, thumb={}). Code at PC: [{}]",
                    self.cpu.regs()[cpu::Cpu::PC],
                    self.cpu.regs()[cpu::Cpu::LR],
                    self.current_thread,
                    is_thumb,
                    self.guest_code_words(self.cpu.regs()[cpu::Cpu::PC])
                );
                self.dump_all_regs();
                self.stack_trace_current();
                return;
            }

            panic!("Error during CPU execution: {error:?}");
        }

        echo!("Debuggable error during CPU execution: {:?}.", error);
        self.enter_debugger(Some(error))
    }

    /// Used to check whether a debugger is connected, and therefore whether
    /// [Environment::enter_debugger] will do something.
    pub fn is_debugging_enabled(&self) -> bool {
        self.gdb_server.is_some()
    }

    /// Suspend execution and hand control to the connected debugger.
    /// You should precede this call with a log message that explains why the
    /// debugger is being invoked. The return value is the same as
    /// [gdb::GdbServer::wait_for_debugger]'s.
    ///
    /// Note that this also yields the thread - take care!
    pub fn enter_debugger(&mut self, reason: Option<cpu::CpuError>) {
        // GDB doesn't seem to manage to produce a useful stack trace, so
        // let's print our own.
        self.stack_trace_current();

        self.yield_thread(ThreadBlock::WaitingForDebugger(reason));
    }

    #[inline(always)]
    /// Respond to the new CPU state (do nothing, execute an SVC or enter
    /// debugging) and decide what to do next.
    fn handle_cpu_state(&mut self, state: cpu::CpuState) -> ThreadNextAction {
        match state {
            cpu::CpuState::Normal => {
                // The CPU executed a full batch of instructions without
                // trapping: real forward progress. Clear the same-LR bypass
                // runaway counter so an earlier, since-recovered burst of
                // fake returns can't accumulate toward a false-positive
                // panic. (The genuine runaway loop never reaches this state:
                // it produces back-to-back guest-trap errors with no Normal
                // batch in between.)
                self.cpu_error_bypass_last_lr = None;
                self.cpu_error_bypass_lr_count = 0;
                ThreadNextAction::Continue
            }
            cpu::CpuState::Svc(svc) => {
                // The program counter is pointing at the
                // instruction after the SVC, but we want the
                // address of the SVC itself.
                let svc_pc = self.cpu.regs()[cpu::Cpu::PC] - 4;
                match svc {
                    dyld::Dyld::SVC_RETURN_TO_HOST => {
                        assert!(
                            svc_pc == self.dyld.return_to_host_routine().addr_without_thumb_bit()
                        );
                        // Normal return from host-to-guest call.
                        ThreadNextAction::ReturnToHost
                    }
                    dyld::Dyld::SVC_LAZY_LINK
                    | dyld::Dyld::SVC_LAZY_LINK_RET_FLAG
                    | dyld::Dyld::SVC_LINKED_FUNCTIONS_BASE.. => {
                        if let Some(f) = self.dyld.get_svc_handler(
                            &self.bins,
                            &mut self.mem,
                            &mut self.cpu,
                            svc_pc,
                            svc,
                        ) {
                            // Successfully dispatching a host/linked function
                            // is real forward progress, so clear the same-LR
                            // bypass runaway counter (see `debug_cpu_error`).
                            self.cpu_error_bypass_last_lr = None;
                            self.cpu_error_bypass_lr_count = 0;
                            // Snapshot r0-r7 before the host function runs, so
                            // a nil-page write inside it can report the guest
                            // arguments that produced the nil pointer.
                            {
                                std::sync::atomic::AtomicU32::store(
                                    &crate::environment::LAST_HOST_CALL_THREAD,
                                    self.current_thread as u32,
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                                let regs = self.cpu.regs();
                                for (slot, reg) in
                                    crate::environment::LAST_HOST_CALL_REGS.iter().enumerate()
                                {
                                    std::sync::atomic::AtomicU32::store(
                                        reg,
                                        regs[slot],
                                        std::sync::atomic::Ordering::Relaxed,
                                    );
                                }
                            }
                            f.call_from_guest(self);

                            let guest_control_flow_redirected =
                                std::mem::take(&mut self.guest_control_flow_redirected);
                            if self.guest_termination_requested.is_some() {
                                log_dbg!(
                                    "Guest termination requested on thread {}; \
                                     returning through the host boundary.",
                                    self.current_thread
                                );
                                return ThreadNextAction::ReturnToHost;
                            }
                            if guest_control_flow_redirected {
                                log_dbg!(
                                    "Linked host function redirected guest control flow; \
                                     skipping normal stub return."
                                );
                                return ThreadNextAction::Continue;
                            }

                            // ORIGINAL LOGIC MERGED: Stack zeroing
                            if svc & dyld::Dyld::SVC_LAZY_LINK_RET_FLAG == 0 {
                                if let Some(len) = self.options.zero_stack_after_guest_to_host_call
                                {
                                    log_once!(
                                        "Applying zeroing of stack after guest to host call."
                                    );
                                    let start = self.cpu.regs()[cpu::Cpu::SP] - len;
                                    self.mem
                                        .bytes_at_mut(mem::Ptr::from_bits(start), len)
                                        .fill(0);
                                }
                            }

                            // On entry_size 4 return here since there's
                            // no space to add a ret after the svc call
                            if svc & dyld::Dyld::SVC_LAZY_LINK_RET_FLAG != 0 {
                                let lr = self.cpu.regs()[cpu::Cpu::LR];
                                self.cpu.branch(GuestFunction::from_addr_with_thumb_bit(lr));
                            }
                            ThreadNextAction::Continue
                        } else {
                            // Returning None only means "restart" for the lazy
                            // linker: it rewrote the trapping stub, so the SVC
                            // must be re-executed. Any other unhandled id is a
                            // stray `svc` from guest code, and rewinding PC
                            // would trap on it forever, so skip past it (the
                            // CPU has already advanced PC past the SVC).
                            let lazy_link = svc & !dyld::Dyld::SVC_LAZY_LINK_RET_FLAG
                                == dyld::Dyld::SVC_LAZY_LINK;
                            if lazy_link {
                                self.cpu.regs_mut()[cpu::Cpu::PC] = svc_pc;
                            }
                            ThreadNextAction::Continue
                        }
                    }
                    dyld::Dyld::SVC_THREAD_EXIT => {
                        if self.current_thread == 0 {
                            log_no_panic!("Main thread exited normally (or crashed early). Returning to host.");
                            ThreadNextAction::ReturnToHost
                        } else {
                            log_dbg!(
                                "Thread {} has completed execution via SVC_THREAD_EXIT. Returning to host.",
                                self.current_thread
                            );

                            // Important: do NOT Continue here.
                            //
                            // The thread-exit routine is an SVC followed by a trap/undefined
                            // instruction. If we continue guest execution after handling the SVC,
                            // PC falls through into that trap and loops forever:
                            //   UndefinedInstruction at 0x3000a014 with LR=0x3000a010
                            //
                            // Returning to host lets the coroutine that called into guest code
                            // finish normally. The secondary-thread coroutine will then store the
                            // return value and mark the thread inactive in the existing normal path.
                            ThreadNextAction::ReturnToHost
                        }
                    }
                }
            }
            cpu::CpuState::Error(e) => ThreadNextAction::DebugCpuError(e),
        }
    }

    fn run_inner(&mut self) {
        let initial_thread = self.current_thread;
        if !self.threads[initial_thread].active {
            log_no_panic!(
                "Warning: run_inner called on inactive thread {}. Returning early.",
                initial_thread
            );
            return;
        }
        if self.threads[initial_thread].guest_context.is_some() {
            // This can happen when an app re-enters the run loop from within
            // a callback (e.g. Pocket Army spawns a worker thread whose
            // completion handler tries to resume the main run loop before the
            // previous invocation has returned). Instead of panicking the
            // whole emulator, log and bail — the outer run_inner is still
            // executing and will pick up from where it left off.
            log_no_panic!(
                "Warning: run_inner called on thread {} which already has a \
                 guest_context (re-entrant run loop?). Returning early to \
                 avoid assertion failure.",
                initial_thread
            );
            return;
        }
        if self.guest_termination_requested.is_some() {
            log_dbg!(
                "Guest termination is pending on thread {}; returning to host.",
                initial_thread
            );
            return;
        }

        loop {
            while self
                .remaining_ticks
                .is_none_or(|remaining_ticks| remaining_ticks > 0)
            {
                let state = self
                    .cpu
                    .run_or_step(&mut self.mem, self.remaining_ticks.as_mut());
                let diag_pc = self.cpu.regs()[crate::cpu::Cpu::PC];
                std::sync::atomic::AtomicU32::store(
                    &crate::environment::LAST_GUEST_PC,
                    diag_pc,
                    std::sync::atomic::Ordering::Relaxed,
                );
                std::sync::atomic::AtomicU32::store(
                    &crate::environment::LAST_GUEST_LR,
                    self.cpu.regs()[crate::cpu::Cpu::LR],
                    std::sync::atomic::Ordering::Relaxed,
                );
                let ring_i = std::sync::atomic::AtomicUsize::load(
                    &crate::environment::GUEST_PC_RING_IDX,
                    std::sync::atomic::Ordering::Relaxed,
                );
                std::sync::atomic::AtomicU32::store(
                    &crate::environment::GUEST_PC_RING[ring_i % 32],
                    diag_pc,
                    std::sync::atomic::Ordering::Relaxed,
                );
                std::sync::atomic::AtomicUsize::store(
                    &crate::environment::GUEST_PC_RING_IDX,
                    ring_i.wrapping_add(1),
                    std::sync::atomic::Ordering::Relaxed,
                );

                // Asphalt 8 (com.gameloft.asphalt8) v1.1.0 compatibility hacks,
                // ported from the touchHLE-XaView fork. The game deliberately
                // calls abort() when its DRM/network checks fail, which looks
                // like a silent emulator crash. These unwinds skip the checks.
                if self
                    .bundle
                    .bundle_identifier()
                    .starts_with("com.gameloft.asphalt8")
                {
                    let pc = self.cpu.regs()[Cpu::PC];
                    // BypassAsphaltDRM: deep stack unwind past the license check
                    if pc == 0x00600ac4 {
                        log!(
                            "WARNING: Bypassing Asphalt DRM via deep stack unwind at {:#010x}!",
                            pc
                        );
                        let fp0 = self.cpu.regs()[7];
                        let fp1: GuestUSize = self.mem.read(mem::ConstPtr::from_bits(fp0));
                        let fp2: GuestUSize = self.mem.read(mem::ConstPtr::from_bits(fp1));
                        let target_lr: GuestUSize =
                            self.mem.read(mem::ConstPtr::from_bits(fp2 + 4));
                        self.cpu.regs_mut()[7] = fp2;
                        self.cpu.regs_mut()[Cpu::SP] = fp1 + 8;
                        self.cpu.regs_mut()[0] = 0;
                        self.cpu
                            .branch(abi::GuestFunction::from_addr_with_thumb_bit(target_lr));
                    }
                    // RestoreConditionalUnwinds: network module deadlocks
                    if (pc == 0x00c3296c || pc == 0x00c32bfc) && self.current_thread != 0 {
                        let fp0 = self.cpu.regs()[7];
                        let prev_fp: GuestUSize = self.mem.read(mem::ConstPtr::from_bits(fp0));
                        let target_lr: GuestUSize =
                            self.mem.read(mem::ConstPtr::from_bits(fp0 + 4));
                        let current_lr = self.cpu.regs()[Cpu::LR];
                        if (current_lr & 0xFFFF0000) == 0x005b0000
                            || (target_lr & 0xFFFF0000) == 0x005b0000
                        {
                            log!(
                                "WARNING: Asphalt network deadlock safely unwound at {:#010x}! LR: {:#010x}",
                                pc,
                                target_lr
                            );
                            self.cpu.regs_mut()[7] = prev_fp;
                            self.cpu.regs_mut()[Cpu::SP] = fp0 + 8;
                            self.cpu.regs_mut()[0] = 0;
                            self.cpu
                                .branch(abi::GuestFunction::from_addr_with_thumb_bit(target_lr));
                        }
                    } else if (pc == 0x00c3375c || pc == 0x00c3376c) && self.current_thread != 0 {
                        let fp0 = self.cpu.regs()[7];
                        let fp1: GuestUSize = self.mem.read(mem::ConstPtr::from_bits(fp0));
                        let prev_fp: GuestUSize = self.mem.read(mem::ConstPtr::from_bits(fp1));
                        let target_lr: GuestUSize =
                            self.mem.read(mem::ConstPtr::from_bits(fp1 + 4));
                        if (target_lr & 0xFFFF0000) == 0x005b0000 {
                            log!(
                                "WARNING: Asphalt deep unwind of infinite parser loop! LR: {:#010x}",
                                target_lr
                            );
                            self.cpu.regs_mut()[7] = prev_fp;
                            self.cpu.regs_mut()[Cpu::SP] = fp1 + 8;
                            self.cpu.regs_mut()[0] = 0;
                            self.cpu
                                .branch(abi::GuestFunction::from_addr_with_thumb_bit(target_lr));
                        }
                    } else if pc == 0x00c32b3c {
                        // TargetedDoubleUnwind: smashed stack frame repair
                        log!(
                            "WARNING: Unwinding smashed Asphalt stack frame at {:#010x}! Thread: {}",
                            pc,
                            self.current_thread
                        );
                        let fp0 = self.cpu.regs()[7];
                        let fp1: GuestUSize = self.mem.read(mem::ConstPtr::from_bits(fp0));
                        if fp1 > fp0 && fp1.wrapping_sub(fp0) < 0x1000 {
                            let saved_r4: GuestUSize =
                                self.mem.read(mem::ConstPtr::from_bits(fp1 - 12));
                            let saved_r5: GuestUSize =
                                self.mem.read(mem::ConstPtr::from_bits(fp1 - 8));
                            let saved_r6: GuestUSize =
                                self.mem.read(mem::ConstPtr::from_bits(fp1 - 4));
                            let saved_r7: GuestUSize = self.mem.read(mem::ConstPtr::from_bits(fp1));
                            let saved_lr: GuestUSize =
                                self.mem.read(mem::ConstPtr::from_bits(fp1 + 4));
                            self.cpu.regs_mut()[4] = saved_r4;
                            self.cpu.regs_mut()[5] = saved_r5;
                            self.cpu.regs_mut()[6] = saved_r6;
                            self.cpu.regs_mut()[7] = saved_r7;
                            self.cpu.regs_mut()[Cpu::SP] = fp1 + 8;
                            self.cpu.regs_mut()[0] = 0;
                            self.cpu
                                .branch(abi::GuestFunction::from_addr_with_thumb_bit(saved_lr | 1));
                        } else {
                            log!("FATAL: Asphalt stack chain corrupted beyond fp0!");
                            self.cpu
                                .branch(abi::GuestFunction::from_addr_with_thumb_bit(
                                    0x00a8a1bd | 1,
                                ));
                        }
                    }
                    // BypassAsphaltOverdriveDeadlocks
                    let lr = self.cpu.regs()[Cpu::LR];
                    if (pc == 0x009d7784 && (lr == 0x0039418f || lr == 0x0039419b))
                        || (pc == 0x009d7464 && lr == 0x0078df65)
                        || (pc == 0x009d8334 && lr == 0x001722e1)
                    {
                        log!(
                            "WARNING: Unwinding Asphalt Overdrive deadlock at PC: {:#010x}, LR: {:#010x}",
                            pc,
                            lr
                        );
                        let fp0 = self.cpu.regs()[7];
                        let prev_fp: GuestUSize = self.mem.read(mem::ConstPtr::from_bits(fp0));
                        let target_lr: GuestUSize =
                            self.mem.read(mem::ConstPtr::from_bits(fp0 + 4));
                        self.cpu.regs_mut()[7] = prev_fp;
                        self.cpu.regs_mut()[Cpu::SP] = fp0 + 8;
                        self.cpu.regs_mut()[0] = 0;
                        self.cpu
                            .branch(abi::GuestFunction::from_addr_with_thumb_bit(target_lr));
                    }
                }

                match self.handle_cpu_state(state) {
                    ThreadNextAction::Continue => {}
                    ThreadNextAction::ReturnToHost => return,
                    ThreadNextAction::DebugCpuError(e) => {
                        self.debug_cpu_error(e);
                    }
                }
                if self.remaining_ticks.is_none() {
                    break;
                }
            }
            self.yield_thread(ThreadBlock::NotBlocked);
        }
    }

    /// Yield the current thread, suspending execution and handing control back
    /// to the executor ([Self::run]), waiting until the current `thread_block`
    /// condition is met.
    pub fn yield_thread(&mut self, thread_block: ThreadBlock) {
        assert!(!self.threads[self.current_thread].is_blocked());
        log_dbg!(
            "Thread {} yielding on {:?}",
            self.current_thread,
            thread_block
        );
        unsafe {
            self.threads[self.current_thread].blocked_by = thread_block;
            // The yielder is set up by `with_yielder` when a coroutine starts
            // executing this thread. If we ever reach `yield_thread` without
            // an active yielder (for example when host code invokes us
            // synchronously before the main coroutine has been entered), we
            // have no coroutine to suspend into. Previously `unwrap()` here
            // panicked the entire emulator; instead, log loudly and clear
            // the block so the host-side caller can keep going. This is the
            // best we can do without a host stack to unwind to.
            let yielder = match self.yielder.as_ref() {
                Some(yielder) => yielder,
                None => {
                    log_no_panic!(
                        "Warning: yield_thread called on thread {} with no \
                         active yielder (block={:?}). Treating as no-op so \
                         the host caller can continue.",
                        self.current_thread,
                        self.threads[self.current_thread].blocked_by,
                    );
                    self.threads[self.current_thread].blocked_by = ThreadBlock::NotBlocked;
                    return;
                }
            };
            self.yielder = std::ptr::null();
            let panic_cell = self.panic_cell.clone();
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let env = std::mem::replace(self, Self::new_fake());
                yielder.suspend(env)
            }));
            match res {
                Ok(env) => {
                    let _ = std::mem::replace(self, env);
                    self.yielder = yielder;
                }
                Err(payload) => {
                    let Some(env) = panic_cell.take() else {
                        log_no_panic!("Did not recieve env for coroutine unwind, must abort!");
                        std::process::exit(-1)
                    };
                    let _ = std::mem::replace(self, env);
                    self.yielder = yielder;
                    std::panic::resume_unwind(payload);
                }
            }
        }
        assert!(!self.threads[self.current_thread].is_blocked());
    }

    /// Find the next thread to execute, and set it up to be switched to.
    ///
    /// This also handles all the required bookkeeping (unlocking mutexes,
    /// decrementing semaphores, setting the thread to be unblocked, etc.).
    /// It is not required that the thread is switched to immediately.
    fn schedule_next_thread(&mut self) -> ThreadId {
        // GDB can allow schedule_next_thread to be called twice in a row -
        // we make sure that this works by immediately fufilling conditions
        // (relocking mutexes, decrementing semaphores, etc.)!
        loop {
            // Try to find a new thread to execute, starting with the thread
            // following the one currently executing.
            let mut next_awakening: Option<Instant> = None;
            for i in 0..self.threads.len() {
                let thread_id = (self.current_thread + 1 + i) % self.threads.len();
                let candidate = &mut self.threads[thread_id];

                if !candidate.active {
                    continue;
                }
                match candidate.blocked_by {
                    ThreadBlock::Sleeping(sleeping_until) => {
                        if sleeping_until <= Instant::now() {
                            log_dbg!("Thread {} finished sleeping.", thread_id);
                            candidate.blocked_by = ThreadBlock::NotBlocked;
                            return thread_id;
                        } else {
                            next_awakening = match next_awakening {
                                None => Some(sleeping_until),
                                Some(other) => Some(other.min(sleeping_until)),
                            };
                        }
                    }
                    ThreadBlock::GuestSleeping(due) => {
                        if due <= self.guest_clock.now() {
                            candidate.blocked_by = ThreadBlock::NotBlocked;
                            return thread_id;
                        }
                        let host_due = self.guest_clock.host_deadline(due);
                        next_awakening = Some(next_awakening.map_or(host_due, |d| d.min(host_due)));
                    }
                    ThreadBlock::Mutex(mutex_id) => {
                        if self.mutex_state.mutex_can_be_locked_by(mutex_id, thread_id) {
                            log_dbg!("Thread {} was unblocked due to mutex #{} unlocking, relocking mutex.", thread_id, mutex_id);
                            self.threads[thread_id].blocked_by = ThreadBlock::NotBlocked;
                            self.relock_unblocked_mutex_for_thread(thread_id, mutex_id);
                            return thread_id;
                        }
                    }
                    ThreadBlock::Semaphore(sem) => {
                        // The semaphore a thread is waiting on may have been
                        // destroyed (e.g. sem_destroy / sem_close) while the
                        // thread was still blocked. Rather than panicking, treat
                        // a now-unknown semaphore as "the wait can no longer be
                        // satisfied here" and wake the thread so it can return
                        // from sem_wait (which fails with EINVAL) instead of
                        // deadlocking or aborting the process.
                        let Some(host_sem_rc) =
                            self.libc_state.semaphore.open_semaphores.get_mut(&sem)
                        else {
                            log!(
                                "Warning: thread {} was blocked on semaphore {:?} \
                                 that no longer exists; waking it.",
                                thread_id,
                                sem
                            );
                            self.threads[thread_id].blocked_by = ThreadBlock::NotBlocked;
                            return thread_id;
                        };
                        let mut host_sem = (*host_sem_rc).borrow_mut();
                        if host_sem.value > 0 {
                            log_dbg!(
                                "Thread {} has awaken on semaphore {:?} with value {}",
                                thread_id,
                                sem,
                                host_sem.value
                            );
                            host_sem.value -= 1;
                            host_sem.waiting.remove(&thread_id);
                            self.threads[thread_id].blocked_by = ThreadBlock::NotBlocked;
                            return thread_id;
                        }
                    }
                    ThreadBlock::Condition(cond, deadline) => {
                        let host_cond = self
                            .libc_state
                            .pthread
                            .cond
                            .condition_variables
                            .get_mut(&cond)
                            .unwrap();
                        let mutex = host_cond.curr_mutex.unwrap();
                        if host_cond
                            .waking
                            .front()
                            .is_some_and(|waking_thread| *waking_thread == thread_id)
                            && self.mutex_state.mutex_can_be_locked_by(mutex, thread_id)
                        {
                            log_dbg!("Thread {} is unblocking on cond var {:?}.", thread_id, cond);
                            host_cond.waking.pop_front();
                            self.threads[thread_id].blocked_by = ThreadBlock::NotBlocked;
                            self.relock_unblocked_mutex_for_thread(thread_id, mutex);
                            return thread_id;
                        } else if let Some(deadline) = deadline {
                            let time = SystemTime::now()
                                .duration_since(SystemTime::UNIX_EPOCH)
                                .unwrap();
                            if deadline <= time {
                                log_dbg!(
                                    "Thread {} is timed out on cond var {:?}.",
                                    thread_id,
                                    cond
                                );
                                assert!(!host_cond.timed_out.contains(&thread_id));
                                host_cond.timed_out.insert(thread_id);

                                // FIX 1: Если тред уже был в очереди waking
                                // (ему отправили
                                // сигнал, но он ещё не успел захватить
                                // мьютекс),
                                // удаляем его оттуда вместо паники.
                                host_cond.waking.retain(|&t| t != thread_id);
                                host_cond.waiting.retain(|&t| t != thread_id);

                                // FIX 2: Если мьютекс всё ещё занят другим
                                // тредом при
                                // таймауте, не паникуем, а переводим тред в
                                // ожидание
                                // мьютекса (как в настоящем
                                // pthread_cond_timedwait).
                                if !self.mutex_state.mutex_can_be_locked_by(mutex, thread_id) {
                                    log_dbg!(
                                        "Thread {} timed out on cond var {:?} but mutex is locked, blocking on mutex.",
                                        thread_id,
                                        cond
                                    );
                                    self.threads[thread_id].blocked_by = ThreadBlock::Mutex(mutex);
                                } else {
                                    self.threads[thread_id].blocked_by = ThreadBlock::NotBlocked;
                                    self.relock_unblocked_mutex_for_thread(thread_id, mutex);
                                    return thread_id;
                                }
                            } else {
                                // --- ГЛАВНОЕ ИСПРАВЛЕНИЕ ДЕДЛОКА ---
                                // Если таймаут еще не вышел, вычисляем остаток
                                // времени
                                // и добавляем его в next_awakening
                                // планировщика!
                                // Теперь эмулятор не упадет, а честно уснет до
                                // этого момента.
                                let remaining = deadline - time;
                                let awakening = Instant::now() + remaining;
                                next_awakening = match next_awakening {
                                    None => Some(awakening),
                                    Some(other) => Some(other.min(awakening)),
                                };
                                // ------------------------------------
                            }
                        }
                    }
                    ThreadBlock::Joining(joinee_thread, ptr) => {
                        if !self.threads[joinee_thread].active {
                            log_dbg!(
                                "Thread {} joining with now finished thread {}.",
                                self.current_thread,
                                joinee_thread
                            );
                            // Write the return value, unless the pointer to
                            // write to is null.
                            if !ptr.is_null() {
                                if let Some(rv) = self.threads[joinee_thread].return_value {
                                    self.mem.write(ptr, rv);
                                } else {
                                    log_dbg!(
                                        "Thread {} joined thread {} which has no return value (pthread_exit?); writing NULL",
                                        self.current_thread,
                                        joinee_thread
                                    );
                                    self.mem.write(ptr, Ptr::null());
                                }
                            }
                            self.threads[thread_id].blocked_by = ThreadBlock::NotBlocked;
                            return thread_id;
                        }
                    }
                    ThreadBlock::NotBlocked => {
                        return thread_id;
                    }
                    ThreadBlock::WaitingForDebugger(_) => unreachable!(),
                    ThreadBlock::Suspended(cnt, _) => {
                        // Original enforced assertion
                        assert!(cnt > 0);
                    }
                    ThreadBlock::KeymgrLock(key) => {
                        if self.libc_state.keymgr_processwide_ptr_is_unlocked(key) {
                            self.threads[thread_id].blocked_by = ThreadBlock::NotBlocked;
                            return thread_id;
                        }
                    }
                }
            }

            // All suitable threads are blocked and at least one is asleep.
            // Sleep until one of them wakes up.
            if let Some(next_awakening) = next_awakening {
                let duration = next_awakening.duration_since(Instant::now());
                log_dbg!("All threads blocked/asleep, sleeping for {:?}.", duration);
                std::thread::sleep(duration);
                // Try again, there should be some thread awake now (or
                // there will be soon, since timing is approximate).
                continue;
            } else {
                // All threads are blocked but none are sleeping — potential
                // deadlock. Before panicking, give conditions with timeouts
                // a brief grace period (10ms). This handles the edge case
                // where a condition-wait with timeout hasn't been detected
                // as "sleeping" because the scheduler loop hasn't
                // re-evaluated it yet. After the grace period, if still
                // stuck, abort with a clear diagnostic.
                static DEADLOCK_GRACE_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                let count = DEADLOCK_GRACE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if count < 3 {
                    log!(
                        "Warning: All threads appear blocked (attempt {}/3). \
                         Sleeping 10ms before retrying…",
                        count + 1
                    );
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                DEADLOCK_GRACE_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
                panic!("No active threads, program has deadlocked!");
            }
        }
    }

    fn set_up_initial_env_vars(&mut self) {
        // TODO: Provide all the system environment variables an app might
        // expect to find.
        // Initialize HOME envvar
        let home_value_cstr = self
            .mem
            .alloc_and_write_cstr(self.fs.home_directory().as_str().as_bytes());
        self.env_vars.insert(b"HOME".to_vec(), home_value_cstr);
        // iOS points TMPDIR at the app's writable tmp/ directory, with a
        // trailing slash like confstr(_CS_DARWIN_USER_TEMP_DIR). SQLite's
        // unixTempFileDir() and other POSIX code look here before falling
        // back to "." (the read-only app bundle). See NSTemporaryDirectory:
        // https://developer.apple.com/documentation/foundation/1409211-nstemporarydirectory
        let tmp_dir = format!("{}/", self.fs.home_directory().join("tmp").as_str());
        let tmp_value_cstr = self.mem.alloc_and_write_cstr(tmp_dir.as_bytes());
        self.env_vars.insert(b"TMPDIR".to_vec(), tmp_value_cstr);
    }

    fn get_sorted_bin_indices(&self) -> Result<Vec<usize>, String> {
        let dylib_graph: Vec<BinaryDependencyNode> = self
            .bins
            .iter()
            .map(|bin| BinaryDependencyNode {
                name: bin.name.clone(),
                dependencies: bin.dynamic_libraries.clone(),
            })
            .collect();
        generate_binary_load_order(&dylib_graph)
    }

    /// Run a function using window and options on the parent stack if we are
    /// inside a coroutine, or run it directly if we aren't. Some
    /// [window::Window] functions require to be called inside this function.
    ///
    /// Android's ABI seems to dislike if certain functions aren't called from
    /// the main stack. Since corosensei uses seperate stacks to run
    /// coroutines, Android doesn't recognize it as the main stack, so those
    /// functions need to be run on the main stack. Unfortunately, there's no
    /// documentation of which functions need to be called with this, so we
    /// have to check ourselves.
    pub fn on_parent_stack_in_coroutine<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut window::Window, &mut options::Options) -> R + Send,
    {
        struct WindowWrapper<'a> {
            window: &'a mut window::Window,
        }
        // SAFETY: we're not sending across threads, we're only sending across
        // the coroutine boundary so it's ok.
        unsafe impl Send for WindowWrapper<'_> {}

        const NO_WINDOW_MSG: &str =
            "on_parent_stack_in_coroutine() was called while touchHLE is running in \
             headless mode (no window). This function is only for code paths that \
             need a real window (e.g. OpenGL ES, text input, dialogs); headless-safe \
             callers must check env.window.is_none() first and skip the window-only \
             work instead of routing through here.";

        if !self.yielder.is_null() {
            unsafe {
                let yielder = self.yielder.as_ref().unwrap();
                let wrapped = WindowWrapper {
                    window: self.window.as_mut().expect(NO_WINDOW_MSG),
                };
                let res = yielder.on_parent_stack(|| {
                    let wrapped = wrapped;
                    wrapped.window.on_main_stack = true;
                    f(wrapped.window, self.options.as_mut())
                });
                self.window.as_mut().expect(NO_WINDOW_MSG).on_main_stack = false;
                res
            }
        } else {
            if let Some(w) = self.window.as_mut() {
                w.on_main_stack = true;
            }
            f(
                self.window.as_mut().expect(NO_WINDOW_MSG),
                self.options.as_mut(),
            )
        }
    }
}

impl Drop for Environment {
    // Clean up all the remaining HostContexts. This isn't strictly required,
    // since this should only occur after a sucessful panic or the app ending,
    // but it is a bit cleaner and avoids confusion inside the logs.
    fn drop(&mut self) {
        if self.objc.is_null() {
            return;
        }
        if let Some(w) = self.window.as_mut() {
            w.on_main_stack = false;
        }
        if self.threads.is_empty()
            || self
                .threads
                .iter()
                .all(|thread| thread.host_context.is_none())
        {
            ENVIRONMENT_INSTANCE_EXISTS.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        unsafe {
            let mut env = std::mem::replace(self, Environment::new_fake());
            let panic_cell = env.panic_cell.clone();
            let threads_len = env.threads.len();
            for i in 0..threads_len {
                let host_context = env.threads[i].host_context.take();
                panic_cell.set(Some(env));
                std::mem::drop(host_context);
                env = panic_cell.take().unwrap_or_else(|| {
                    log_no_panic!(
                        "Did not recieve env from coroutine unwind during drop, must abort!"
                    );
                    std::process::exit(-1)
                });
            }
            *self = env;
        }
        ENVIRONMENT_INSTANCE_EXISTS.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Decode the direct call instruction (`bl`/`blx <label>`) that produced the
/// return address `lr`, using the supplied fallible guest-memory readers.
///
/// Returns `(call_site, target)` with both addresses lacking the Thumb bit.
/// Returns `None` for indirect calls (`blx Rm`), for return addresses not
/// preceded by a call instruction, and for unreadable memory.
fn decode_direct_call_before(
    lr: u32,
    read_u16: impl Fn(u32) -> Option<u16>,
    read_u32: impl Fn(u32) -> Option<u32>,
) -> Option<(u32, u32)> {
    let return_addr = lr & !1;
    let lr_is_thumb = (lr & 1) != 0;

    if lr_is_thumb {
        // 16-bit `blx Rm`: 0100 0111 1mmm m000
        let last_hw = read_u16(return_addr.wrapping_sub(2))?;
        if (last_hw & 0xff87) == 0x4780 {
            return None;
        }
        // 32-bit BL / BLX <label>:
        //   hw1 = 11110 S imm10, hw2 = 11 J1 1 J2 imm11 (BL)
        //                        hw2 = 11 J1 0 J2 imm10H 0 (BLX)
        let call_site = return_addr.wrapping_sub(4);
        let hw1 = read_u16(call_site)?;
        let hw2 = last_hw;
        let is_bl_prefix = (hw1 & 0xf800) == 0xf000;
        let is_bl_suffix = (hw2 & 0xd000) == 0xd000;
        let is_blx_suffix = (hw2 & 0xd001) == 0xc000;
        if !is_bl_prefix || !(is_bl_suffix || is_blx_suffix) {
            return None;
        }
        let is_blx = (hw2 & 0x1000) == 0;
        let s = ((hw1 >> 10) & 1) as u32;
        let j1 = ((hw2 >> 13) & 1) as u32;
        let j2 = ((hw2 >> 11) & 1) as u32;
        let i1 = (!(j1 ^ s)) & 1;
        let i2 = (!(j2 ^ s)) & 1;
        let imm10 = (hw1 & 0x03ff) as u32;
        let imm11 = (hw2 & 0x07ff) as u32;
        let raw = (s << 24) | (i1 << 23) | (i2 << 22) | (imm10 << 12) | (imm11 << 1);
        // Sign-extend from 25 bits.
        let imm32 = (((raw << 7) as i32) >> 7) as u32;
        let base = if is_blx {
            call_site.wrapping_add(4) & !3
        } else {
            call_site.wrapping_add(4)
        };
        Some((call_site, base.wrapping_add(imm32)))
    } else {
        let call_site = return_addr.wrapping_sub(4);
        let insn = read_u32(call_site)?;
        // BLX Rm: cccc 0001 0010 1111 1111 1111 0011 mmmm
        if (insn & 0x0fff_fff0) == 0x012f_ff30 {
            return None;
        }
        let imm24 = insn & 0x00ff_ffff;
        let offset = (((imm24 << 8) as i32) >> 6) as u32;
        // The BLX check must come first: its "condition" field is 0b1111.
        if (insn & 0xfe00_0000) == 0xfa00_0000 {
            // BLX <label> (always switches to Thumb)
            let h = (insn >> 24) & 1;
            Some((
                call_site,
                call_site
                    .wrapping_add(8)
                    .wrapping_add(offset)
                    .wrapping_add(h << 1),
            ))
        } else if (insn & 0x0f00_0000) == 0x0b00_0000 {
            // BL <label>
            Some((call_site, call_site.wrapping_add(8).wrapping_add(offset)))
        } else {
            None
        }
    }
}

/// Given a direct call at `call_site` to `target`, could the function that
/// contains `fault_pc` be the callee? If not, the return address produced by
/// that call is stale with respect to the trap at `fault_pc`.
fn direct_call_is_consistent_with_fault(call_site: u32, target: u32, fault_pc: u32) -> bool {
    // The function that trapped must start at or before `fault_pc`, so a
    // call that branched *past* the trap can't have called it.
    if target > fault_pc {
        return false;
    }
    // When the call site precedes the trap, the callee must also start after
    // the call site: functions are contiguous, so a target at or before the
    // call site is either the caller itself (recursion, which is handled via
    // the frame record) or some unrelated function that has since returned.
    if call_site < fault_pc && target <= call_site {
        return false;
    }
    true
}

#[cfg(test)]
mod guest_trap_call_site_tests {
    use super::*;

    fn readers(
        code: &[(u32, u16)],
    ) -> (
        impl Fn(u32) -> Option<u16> + '_,
        impl Fn(u32) -> Option<u32> + '_,
    ) {
        let read_u16 = move |addr: u32| {
            code.iter()
                .find(|(a, _)| *a == addr)
                .map(|(_, v)| *v)
        };
        let read_u32 = move |addr: u32| {
            let lo = code.iter().find(|(a, _)| *a == addr).map(|(_, v)| *v)?;
            let hi = code
                .iter()
                .find(|(a, _)| *a == addr.wrapping_add(2))
                .map(|(_, v)| *v)?;
            Some((lo as u32) | ((hi as u32) << 16))
        };
        (read_u16, read_u32)
    }

    #[test]
    fn decodes_thumb_bl_forward_and_backward() {
        // 0x8000: f000 f802  bl 0x8008
        let code = [(0x8000, 0xf000), (0x8002, 0xf802)];
        let (r16, r32) = readers(&code);
        assert_eq!(decode_direct_call_before(0x8005, r16, r32), Some((0x8000, 0x8008)));

        // 0x8000: f7ff ff00  bl 0x7e04
        let code = [(0x8000, 0xf7ff), (0x8002, 0xff00)];
        let (r16, r32) = readers(&code);
        assert_eq!(decode_direct_call_before(0x8005, r16, r32), Some((0x8000, 0x7e04)));

        // 0x1000: f7ff fffe  bl . (offset -4)
        let code = [(0x1000, 0xf7ff), (0x1002, 0xfffe)];
        let (r16, r32) = readers(&code);
        assert_eq!(decode_direct_call_before(0x1005, r16, r32), Some((0x1000, 0x1000)));
    }

    #[test]
    fn decodes_thumb_blx_label_with_word_alignment() {
        // 0x8002: f000 e800  blx 0x8004 (Align(PC, 4) + 0)
        let code = [(0x8002, 0xf000), (0x8004, 0xe800)];
        let (r16, r32) = readers(&code);
        assert_eq!(decode_direct_call_before(0x8007, r16, r32), Some((0x8002, 0x8004)));
    }

    #[test]
    fn indirect_thumb_calls_are_not_decoded() {
        // 0x8002: 4798  blx r3
        let code = [(0x8000, 0xf000), (0x8002, 0x4798)];
        let (r16, r32) = readers(&code);
        assert_eq!(decode_direct_call_before(0x8005, r16, r32), None);
    }

    #[test]
    fn non_call_instructions_before_lr_are_not_decoded() {
        // 0x8000: 2000 bx lr-ish garbage, 0x8002: 4770 bx lr
        let code = [(0x8000, 0x2000), (0x8002, 0x4770)];
        let (r16, r32) = readers(&code);
        assert_eq!(decode_direct_call_before(0x8005, r16, r32), None);
        // Unreadable memory.
        let (r16, r32) = readers(&[]);
        assert_eq!(decode_direct_call_before(0x8005, r16, r32), None);
    }

    #[test]
    fn decodes_arm_bl_and_blx() {
        // 0x1000: eb000000  bl 0x1008
        let code = [(0x1000, 0x0000), (0x1002, 0xeb00)];
        let (r16, r32) = readers(&code);
        assert_eq!(decode_direct_call_before(0x1004, r16, r32), Some((0x1000, 0x1008)));

        // 0x1000: ebfffffe  bl . (offset -8)
        let code = [(0x1000, 0xfffe), (0x1002, 0xebff)];
        let (r16, r32) = readers(&code);
        assert_eq!(decode_direct_call_before(0x1004, r16, r32), Some((0x1000, 0x1000)));

        // 0x1000: fb000000  blx 0x100a (H=1), must not be mistaken for BL
        let code = [(0x1000, 0x0000), (0x1002, 0xfb00)];
        let (r16, r32) = readers(&code);
        assert_eq!(decode_direct_call_before(0x1004, r16, r32), Some((0x1000, 0x100a)));

        // 0x1000: e12fff33  blx r3
        let code = [(0x1000, 0xff33), (0x1002, 0xe12f)];
        let (r16, r32) = readers(&code);
        assert_eq!(decode_direct_call_before(0x1004, r16, r32), None);
    }

    #[test]
    fn stale_lr_is_detected_from_the_call_target() {
        // Asphalt 8 shape: `bl` at 0x1e3c2 (LR=0x1e3c7), trap at 0x1e432 in
        // the same function. A call to anything outside (0x1e3c2, 0x1e432]
        // can't have produced a return address for the trapping function.
        assert!(!direct_call_is_consistent_with_fault(0x1e3c2, 0x1e000, 0x1e432));
        assert!(!direct_call_is_consistent_with_fault(0x1e3c2, 0x1e3c2, 0x1e432));
        assert!(!direct_call_is_consistent_with_fault(0x1e3c2, 0x2a000, 0x1e432));
        // A leaf callee located between the call site and the trap is a
        // plausible owner of the trap, so LR may be genuine.
        assert!(direct_call_is_consistent_with_fault(0x1e3c2, 0x1e400, 0x1e432));
        assert!(direct_call_is_consistent_with_fault(0x1e3c2, 0x1e432, 0x1e432));
        // Callee located before the caller: can't tell, so assume genuine.
        assert!(direct_call_is_consistent_with_fault(0x2000, 0x1000, 0x1010));
        // ...unless the call went past the trap.
        assert!(!direct_call_is_consistent_with_fault(0x2000, 0x3000, 0x1010));
    }
}

#[cfg(test)]
mod guest_cpu_error_tests {
    use super::*;

    #[test]
    fn guest_traps_are_recoverable_without_a_debugger() {
        assert!(Environment::is_recoverable_guest_cpu_error(
            &cpu::CpuError::UndefinedInstruction
        ));
        assert!(Environment::is_recoverable_guest_cpu_error(
            &cpu::CpuError::Breakpoint
        ));
    }

    #[test]
    fn memory_errors_are_not_recoverable_without_a_debugger() {
        assert!(!Environment::is_recoverable_guest_cpu_error(
            &cpu::CpuError::MemoryError
        ));
    }
}

#[cfg(test)]
mod dylib_sorting_tests {
    use std::collections::HashSet;

    use super::*;
    fn create_dylib_graph(bin_configs: &[(&str, &[&str])]) -> Vec<BinaryDependencyNode> {
        bin_configs
            .iter()
            .map(|(name, dependencies)| BinaryDependencyNode {
                name: name.to_string(),
                dependencies: dependencies.iter().map(|s| s.to_string()).collect(),
            })
            .collect()
    }

    /// Verify dylib sort by checking that no dependents are needed
    /// before their import
    fn verify_sort(graph: &[BinaryDependencyNode], sorted_indices: &[usize]) {
        assert_eq!(sorted_indices.len(), graph.len());
        let bin_to_index: HashMap<_, _> = graph
            .iter()
            .enumerate()
            .map(|(idx, node)| (node.name.as_str(), idx))
            .collect();
        let mut loaded_dylibs = HashSet::new();

        for &index in sorted_indices {
            let current_bin = graph.get(index).unwrap();
            for dependency in current_bin
                .dependencies
                .iter()
                .map(|path| path.strip_prefix("/usr/lib/").unwrap_or(path.as_str()))
            {
                // Ignore dependencies that are not included in packaged dylibs
                let Some(&dylib_index) = bin_to_index.get(dependency) else {
                    continue;
                };

                assert!(loaded_dylibs.contains(&dylib_index));
            }

            loaded_dylibs.insert(index);
        }
    }

    #[test]
    fn test_no_dependencies() {
        let dylib_graph = create_dylib_graph(&[]);
        let sorted_indices = generate_binary_load_order(&dylib_graph).unwrap();
        verify_sort(&dylib_graph, &sorted_indices);
    }

    #[test]
    fn test_single_bin() {
        let dylib_graph = create_dylib_graph(&[("A", &[])]);
        let sorted_indices = generate_binary_load_order(&dylib_graph).unwrap();

        verify_sort(&dylib_graph, &sorted_indices);
    }

    #[test]
    fn test_linear_dependencies() {
        // A -> B -> C -> D
        let dylib_graph = create_dylib_graph(&[
            ("A", &[]),
            ("B", &["/usr/lib/A"]),
            ("C", &["/usr/lib/B"]),
            ("D", &["/usr/lib/C"]),
        ]);

        let sorted_indices = generate_binary_load_order(&dylib_graph).unwrap();

        verify_sort(&dylib_graph, &sorted_indices);
    }

    #[test]
    fn test_diamond_dependencies() {
        // A -> B -> D
        //  \-> C -/
        let dylib_graph =
            create_dylib_graph(&[("A", &[]), ("B", &["A"]), ("C", &["A"]), ("D", &["B", "C"])]);
        let sorted_indices = generate_binary_load_order(&dylib_graph).unwrap();

        verify_sort(&dylib_graph, &sorted_indices);
    }

    #[test]
    fn test_with_isolated_nodes() {
        // A -> B
        // C
        // D
        let dylib_graph = create_dylib_graph(&[("A", &[]), ("B", &["A"]), ("C", &[]), ("D", &[])]);
        let sorted_indices = generate_binary_load_order(&dylib_graph).unwrap();

        verify_sort(&dylib_graph, &sorted_indices);
    }

    #[test]
    fn test_complex_dependency_graph() {
        // A -> B -> D
        // A -> C -> E
        // F -> G
        // H
        let dylib_graph = create_dylib_graph(&[
            ("A", &[]),
            ("B", &["A"]),
            ("C", &["A"]),
            ("D", &["B"]),
            ("E", &["C"]),
            ("F", &[]),
            ("G", &["F"]),
            ("H", &[]),
        ]);
        let sorted_indices = generate_binary_load_order(&dylib_graph).unwrap();

        verify_sort(&dylib_graph, &sorted_indices);
    }

    #[test]
    fn test_with_external_dependencies() {
        let dylib_graph = create_dylib_graph(&[
            ("A", &["external1"]),
            ("B", &["A", "external2"]),
            ("C", &["B"]),
        ]);
        let sorted_indices = generate_binary_load_order(&dylib_graph).unwrap();

        verify_sort(&dylib_graph, &sorted_indices);
    }

    #[test]
    fn test_cycle() {
        // A -> B -> C -> A
        let dylib_graph = create_dylib_graph(&[("A", &["C"]), ("B", &["A"]), ("C", &["B"])]);
        let result = generate_binary_load_order(&dylib_graph);

        assert!(
            result.is_err(),
            "Sort should detect cycle and return an error"
        );
    }

    #[test]
    fn test_self_dependency() {
        let dylib_graph = create_dylib_graph(&[("A", &["A"])]);
        let result = generate_binary_load_order(&dylib_graph);

        assert!(
            result.is_err(),
            "Sort should detect self-dependency as a cycle and return an error"
        );
    }

    #[test]
    fn test_transitive_dependencies_and_aliases_are_loaded_once() {
        let roots = vec![
            "/usr/lib/libstdc++.6.dylib".to_owned(),
            "/usr/lib/libstdc++.6.0.9.dylib".to_owned(),
        ];
        let mut visited = Vec::new();
        let loaded = load_transitive_dependencies(&roots, |path| {
            visited.push(path.to_owned());
            let dependencies = match path {
                "/usr/lib/libstdc++.6.dylib" | "/usr/lib/libstdc++.6.0.9.dylib" => {
                    Some(vec!["/usr/lib/libgcc_s.1.dylib".to_owned()])
                }
                "/usr/lib/libgcc_s.1.dylib" => {
                    Some(vec!["/usr/lib/libSystem.B.dylib".to_owned()])
                }
                _ => None,
            };
            Ok(dependencies.map(|dependencies| (path.to_owned(), dependencies)))
        })
        .unwrap();

        assert_eq!(
            loaded,
            vec![
                "/usr/lib/libstdc++.6.dylib".to_owned(),
                "/usr/lib/libgcc_s.1.dylib".to_owned(),
            ]
        );
        assert_eq!(
            visited,
            vec![
                "/usr/lib/libstdc++.6.dylib".to_owned(),
                "/usr/lib/libgcc_s.1.dylib".to_owned(),
                "/usr/lib/libSystem.B.dylib".to_owned(),
            ]
        );
    }
}
