/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! In-emulator game trainer ("Cheat Engine"-style), driven either by the
//! on-screen overlay UI (see [crate::trainer_ui]) or by text hack files.
//!
//! Hack files live under `touchHLE_hacks/` in the user data directory:
//!
//! - `hacks.txt` — global file. Lines are either app-scoped
//!   `com.example.App: 0x1234=100` (the format used by
//!   `com.lego.NinjagoSpinjitzuScavengerHunt`-style lists) or bare
//!   `0x1234=100` lines that belong to the most recent `[app.id]` section
//!   header. `#` starts a comment.
//! - `<app-id>.txt` — per-app file with bare `0x1234=100` lines. The
//!   overlay's "SAVE HACK" button appends here, so hacks survive restarts.
//!
//! Every hack line is applied once when the app starts (and when the file
//! changes on disk), and can additionally be marked as frozen with a
//! `# freeze` comment, which makes the trainer re-assert the value
//! continuously.

use crate::mem::{ConstVoidPtr, GuestUSize, Mem};
use crate::trainer_ui::{self, TrainerCmd};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod bulk;
pub mod classify;
pub mod watch;
use watch::{Activity, Change, Snapshot};
use classify::{analyze_batch, Analysis, ResultFilter};
use bulk::{apply_bulk, plan_bulk, BulkPlan};

struct PendingBulk {
    plan: BulkPlan,
    created_at: Instant,
}

/// Data type of a searched/set memory value. All values are little-endian,
/// matching ARM memory layout.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum VType {
    /// Automatic: searches all concrete types, per-result type resolution.
    Auto,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
    /// IEEE 754 double. Modern-ish games store money and timers as `double`
    /// (`CFNumber`/`NSNumber` round-trips, engine math), and a 32-bit scan
    /// can never match one.
    F64,
    U64,
    I64,
}

impl VType {
    pub const ALL: [VType; 11] = [
        VType::Auto,
        VType::I32,
        VType::U32,
        VType::I16,
        VType::U16,
        VType::I8,
        VType::U8,
        VType::F32,
        VType::F64,
        VType::U64,
        VType::I64,
    ];

    pub fn size(self) -> GuestUSize {
        match self {
            VType::Auto => 4,
            VType::U8 | VType::I8 => 1,
            VType::U16 | VType::I16 => 2,
            VType::U32 | VType::I32 | VType::F32 => 4,
            VType::F64 | VType::U64 | VType::I64 => 8,
        }
    }

    /// Next type in the cycling order used by the overlay's type button.
    pub fn next(self) -> VType {
        let idx = Self::ALL.iter().position(|&t| t == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    /// Parse a value without silently truncating it to the target width.
    /// Hex input denotes raw bits; decimal F32 input denotes a float value.
    pub fn parse(self, text: &str) -> Option<u64> {
        let text = text.trim();
        if self == VType::Auto {
            return auto_types().iter().find_map(|t| t.parse(text));
        }
        if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            let bits = u64::from_str_radix(hex, 16).ok()?;
            return (bits == self.mask_bits(bits)).then_some(bits);
        }
        let bits = match self {
            VType::Auto => unreachable!(),
            VType::U8 => text.parse::<u8>().ok()? as u64,
            VType::I8 => text.parse::<i8>().ok()? as u64,
            VType::U16 => text.parse::<u16>().ok()? as u64,
            VType::I16 => text.parse::<i16>().ok()? as u64,
            VType::U32 => text.parse::<u32>().ok()? as u64,
            VType::I32 => text.parse::<i32>().ok()? as u64,
            VType::F32 => {
                let value = text.parse::<f32>().ok()?;
                if !value.is_finite() {
                    return None;
                }
                value.to_bits() as u64
            }
            VType::F64 => {
                let value = text.parse::<f64>().ok()?;
                if !value.is_finite() {
                    return None;
                }
                value.to_bits()
            }
            VType::U64 => text.parse::<u64>().ok()?,
            VType::I64 => text.parse::<i64>().ok()? as u64,
        };
        Some(self.mask_bits(bits))
    }

    fn mask_bits(self, value: u64) -> u64 {
        match self {
            VType::Auto => value & 0xFFFFFFFF,
            VType::U8 | VType::I8 => value & 0xFF,
            VType::U16 | VType::I16 => value & 0xFFFF,
            VType::U32 | VType::I32 | VType::F32 => value & 0xFFFFFFFF,
            VType::F64 | VType::U64 | VType::I64 => value,
        }
    }

    /// Format a raw bit pattern for display.
    pub fn format(self, bits: u64) -> String {
        match self {
            VType::Auto => format!("{}", bits as u32 as i32),
            VType::U8 => format!("{}", bits as u8),
            VType::I8 => format!("{}", bits as u8 as i8),
            VType::U16 => format!("{}", bits as u16),
            VType::I16 => format!("{}", bits as u16 as i16),
            VType::U32 => format!("{}", bits as u32),
            VType::I32 => format!("{}", bits as u32 as i32),
            VType::F32 => format!("{:.4}", f32::from_bits(bits as u32)),
            VType::F64 => format!("{:.4}", f64::from_bits(bits)),
            VType::U64 => format!("{}", bits),
            VType::I64 => format!("{}", bits as i64),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            VType::Auto => "AUTO",
            VType::U8 => "U8",
            VType::I8 => "I8",
            VType::U16 => "U16",
            VType::I16 => "I16",
            VType::U32 => "U32",
            VType::I32 => "I32",
            VType::F32 => "F32",
            VType::F64 => "F64",
            VType::U64 => "U64",
            VType::I64 => "I64",
        }
    }

    fn read_le(bytes: &[u8], offset: usize, size: usize) -> u64 {
        let mut value = 0u64;
        for i in 0..size {
            value |= (bytes[offset + i] as u64) << (8 * i);
        }
        value
    }

    fn address_range_is_valid(self, mem: &Mem, addr: u32) -> bool {
        addr >= mem.null_segment_size()
            && (addr as u64 + self.size() as u64) <= u32::MAX as u64 + 1
    }

    /// Read the raw bits at `addr` for this type.
    pub fn read_at(self, mem: &Mem, addr: u32) -> Option<u64> {
        if !self.address_range_is_valid(mem, addr) {
            return None;
        }
        let bytes = mem.get_bytes_fallible(ConstVoidPtr::from_bits(addr), self.size())?;
        let bytes = bytes.get(..self.size() as usize)?;
        Some(Self::read_le(bytes, 0, bytes.len()))
    }

    /// Write raw bits without falling back to Mem's invalid-address sink.
    pub fn write_at(self, mem: &mut Mem, addr: u32, bits: u64) -> bool {
        if !self.address_range_is_valid(mem, addr) {
            return false;
        }
        let Some(bytes) = mem.get_bytes_fallible_mut(ConstVoidPtr::from_bits(addr), self.size()) else {
            return false;
        };
        bytes.copy_from_slice(&bits.to_le_bytes()[..self.size() as usize]);
        true
    }
}

/// A memory address found by a search, with its current raw bit pattern.
#[derive(Copy, Clone, Debug)]
pub struct SearchResult {
    pub addr: u32,
    pub vtype: VType,
    pub bits: u64,
    /// Set during a live-value refresh when the value at this address
    /// changed since the previous refresh (shown highlighted in the UI).
    pub changed: bool,
    pub analysis: Analysis,
}

/// A hack patch: either applied once from a hack file, or continuously
/// re-asserted ("frozen").
#[derive(Clone, Debug)]
pub struct Patch {
    pub addr: u32,
    pub vtype: VType,
    pub bits: u64,
    pub freeze: bool,
}

/// Per-app trainer state. Rebuilt whenever the running app changes.
#[derive(Default)]
struct TrainerState {
    app_id: Option<String>,
    /// Results of the most recent search/refine.
    results: Vec<SearchResult>,
    /// Whether a search has been performed (so "REFINE" is meaningful).
    searched: bool,
    analysis_cursor: usize,
    snapshot: Option<Snapshot>,
    activity: Activity,
    activity_cursor: usize,
    pending_bulk: Option<PendingBulk>,
    /// One-shot patches already applied (from hack files).
    applied_hacks: HashSet<(u32, VType, u64)>,
    /// Frozen patches, re-asserted every tick.
    frozen: Vec<Patch>,
    /// Hack files, as (path, mtime); used to detect edits on disk.
    watched_files: Vec<(PathBuf, Option<std::time::SystemTime>)>,
}

pub struct Trainer {
    pub enabled: bool,
    state: TrainerState,
    last_file_check: Instant,
    last_freeze_tick: Instant,
    last_value_refresh: Instant,
    dump_counter: u32,
}

const HACKS_DIR: &str = "touchHLE_hacks";
const GLOBAL_HACKS_FILE: &str = "hacks.txt";
/// Don't try to search regions larger than this. The cap exists because the
/// scan is synchronous: it runs on the emulator thread while the game is
/// paused between frames, so a multi-gigabyte region would look like a hang.
///
/// 64 MiB was too low in practice: engines that keep the player profile or
/// their asset bookkeeping in one big `mmap` (Asphalt 8 among them) put the
/// interesting numbers in a region this large or larger, and the trainer then
/// silently never looked at them at all.
const MAX_SCAN_ALLOCATION: GuestUSize = 512 * 1024 * 1024;
/// Hard cap on stored search results (memory + UI sanity).
const MAX_RESULTS: usize = 500_000;
/// Confirmation expires rather than leaving a hidden armed bulk operation.
const BULK_CONFIRM_TIMEOUT: Duration = Duration::from_secs(15);
/// Cap on dump file lines.
const MAX_DUMP_LINES: usize = 200_000;

impl Trainer {
    pub fn new(enabled: bool) -> Trainer {
        Trainer {
            enabled,
            state: TrainerState::default(),
            last_file_check: Instant::now() - Duration::from_secs(60),
            last_freeze_tick: Instant::now(),
            last_value_refresh: Instant::now(),
            dump_counter: 0,
        }
    }

    /// Turns the trainer on or off while an app is running, so the quick
    /// menu's "Cheat Engine" row takes effect without restarting anything.
    ///
    /// Switching it off also drops the app the trainer was attached to and
    /// hides the overlay; switching it back on makes the next `tick()`
    /// re-announce the running app (which reloads that app's saved hacks).
    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        self.state.app_id = None;
        if !enabled {
            crate::trainer_ui::reset_for_app(None);
        }
    }

    /// Called from the main loop. `app_id` is the identifier of the
    /// currently running app, if any.
    pub fn tick(&mut self, mem: &mut Mem, app_id: Option<&str>, objc: &crate::objc::ObjC) {
        if !self.enabled {
            return;
        }
        if self.state.app_id.as_deref() != app_id {
            // App changed: reset search state and load its hacks.
            log!(
                "trainer: app changed to {:?}; resetting search state",
                app_id
            );
            let enabled = self.enabled;
            self.state = TrainerState::default();
            self.state.app_id = app_id.map(String::from);
            self.load_and_apply_hacks(mem);
            trainer_ui::reset_for_app(app_id);
            self.last_file_check = Instant::now();
        }

        if self.state.pending_bulk.as_ref().is_some_and(|pending| {
            pending.created_at.elapsed() >= BULK_CONFIRM_TIMEOUT
        }) {
            self.state.pending_bulk = None;
            trainer_ui::publish_bulk_preview(false);
            trainer_ui::publish_status("PREVIEW EXPIRED: TAP SET ALL".to_string());
        }

        // Pick up commands from the overlay UI.
        for cmd in trainer_ui::take_commands() {
            let inspect = matches!(&cmd, TrainerCmd::InspectSelection);
            self.handle_command(mem, cmd);
            if inspect { self.describe_selection(mem, objc); }
        }

        // Re-assert frozen values ~20 times per second.
        if self.last_freeze_tick.elapsed() >= Duration::from_millis(50) {
            self.last_freeze_tick = Instant::now();
            self.apply_frozen(mem);
        }

        // Live-update the values shown in the results list ~4 times per
        // second, flagging addresses whose value changed since last time —
        // spend coins in-game and the matching row lights up.
        if self.last_value_refresh.elapsed() >= Duration::from_millis(250) {
            let seconds = self.last_value_refresh.elapsed().as_secs_f32();
            self.last_value_refresh = Instant::now();
            self.refresh_live_values(mem, Some(objc), seconds);
        }

        // Watch hack files for external edits (e.g. edited over ADB or a
        // file manager) — re-check at most once per second.
        if self.last_file_check.elapsed() >= Duration::from_secs(1) {
            self.last_file_check = Instant::now();
            self.reload_changed_files(mem);
        }
    }

    /// Show the actual matched field name when runtime metadata is available.
    fn describe_selection(&self, mem: &Mem, objc: &crate::objc::ObjC) {
        let Some(addr) = trainer_ui::selected_address() else { return; };
        let Some(t) = trainer_ui::selected_result_type(addr) else { return; };
        if let Some((base, size)) = mem.live_allocations().into_iter().find(|&(base, size)| {
            base <= addr && addr as u64 + t.size() as u64 <= base as u64 + size as u64
        }) {
            if let Some(name) = objc.diagnostic_scalar_field(mem, base, size, addr, t.size(), classify::encoding(t)) {
                trainer_ui::publish_status(format!("FIELD {} [{}]; verify in game", name, t.name()));
            }
        }
    }

    fn refresh_watch_value(&self, mem: &Mem) {
        let Some(target @ (addr, vtype)) = trainer_ui::watch_target() else { return; };
        let found = self.state.results.iter().any(|r| r.addr == addr && r.vtype == vtype);
        let live = mem.live_allocations().iter().any(|&(base, size)| {
            base <= addr && addr as u64 + vtype.size() as u64 <= base as u64 + size as u64
        });
        trainer_ui::publish_watch_value(target, if found && live { vtype.read_at(mem, addr) } else { None });
    }

    /// Explicit address/type from WATCH, never the main editor's selection.
    fn set_watch_value(&mut self, mem: &mut Mem, addr: u32, vtype: VType, text: &str) -> Result<u64, &'static str> {
        if vtype == VType::Auto || !self.state.results.iter().any(|r| r.addr == addr && r.vtype == vtype) {
            return Err("RESULT EXPIRED: SEARCH AGAIN");
        }
        let bits = vtype.parse(text).ok_or("BAD VALUE: CHECK TYPE / RANGE")?;
        let mut allocations = mem.live_allocations();
        allocations.sort_unstable_by_key(|a| a.0);
        if bulk::containing_allocation(&allocations, addr, vtype.size()).is_none()
            || vtype.read_at(mem, addr).is_none()
        {
            return Err("ADDRESS NO LONGER LIVE");
        }
        let end = addr as u64 + vtype.size() as u64;
        if self.state.frozen.iter().any(|p| {
            (p.addr as u64) < end && (addr as u64) < p.addr as u64 + p.vtype.size() as u64
        }) {
            return Err("FROZEN RANGE: UNFREEZE FIRST");
        }
        // A changing value is expected here. Validate identity/range, not
        // equality with a historical event's old value.
        if !vtype.write_at(mem, addr, bits) { return Err("WRITE FAILED"); }
        self.state.snapshot = None;
        record_trainer_write(mem, &mut self.state.results, addr, vtype.size());
        Ok(bits)
    }

    fn handle_command(&mut self, mem: &mut Mem, cmd: TrainerCmd) {
        if !matches!(&cmd, TrainerCmd::SetAll { .. }) {
            if self.state.pending_bulk.take().is_some() {
                trainer_ui::publish_status("BULK PREVIEW CANCELLED".to_string());
            }
            trainer_ui::publish_bulk_preview(false);
        }
        // A comparison experiment must not include the trainer's own edits.
        if matches!(&cmd, TrainerCmd::Set { .. } | TrainerCmd::SetAll { .. }
            | TrainerCmd::Freeze { .. } | TrainerCmd::ApplyHack { .. }) {
            self.state.snapshot = None;
        }
        match cmd {
            TrainerCmd::WatchSet { addr, vtype, text } => {
                let status = match self.set_watch_value(mem, addr, vtype, &text) {
                    Ok(bits) => {
                        trainer_ui::publish_live_values(&self.state.results);
                        format!("SET 0x{:08X} = {}", addr, vtype.format(bits))
                    }
                    Err(reason) => reason.to_string(),
                };
                self.refresh_watch_value(mem);
                trainer_ui::publish_watch_status((addr, vtype), status);
            }
            TrainerCmd::Mark => {
                if self.state.results.is_empty() {
                    trainer_ui::publish_status("SEARCH FIRST, THEN MARK".to_string());
                } else {
                    self.state.snapshot = Some(Snapshot::capture(mem, &self.state.results));
                    trainer_ui::publish_status("MARKED: PLAY, THEN CHANGED / SAME / UP / DOWN".to_string());
                }
            }
            TrainerCmd::Compare(filter) => {
                let Some(snapshot) = self.state.snapshot.take() else {
                    trainer_ui::publish_status("TAP MARK BEFORE THE GAME ACTION".to_string());
                    return;
                };
                snapshot.retain(mem, &mut self.state.results, filter);
                self.state.snapshot = Some(Snapshot::capture(mem, &self.state.results));
                self.state.activity = Activity::default();
                trainer_ui::clear_activity();
                trainer_ui::publish_results(&self.state.results, self.state.results.len());
                trainer_ui::publish_status(format!("KEPT {}: BASELINE UPDATED", self.state.results.len()));
            }
            TrainerCmd::ClearActivity => {
                self.state.activity = Activity::default();
                trainer_ui::clear_activity();
            }
            TrainerCmd::CancelBulk | TrainerCmd::InspectSelection => {}
            TrainerCmd::RefreshView => {
                trainer_ui::publish_live_values(&self.state.results);
                self.refresh_watch_value(mem);
            }
            TrainerCmd::Search { vtype, text } => {
                let Some(_) = vtype.parse(&text) else {
                    trainer_ui::publish_status(format!("BAD VALUE: {}", text));
                    return;
                };
                let mut results = search_all(mem, vtype, &text, None);
                self.state.analysis_cursor = 0;
                analyze_batch(mem, &mut results, &mut self.state.analysis_cursor);
                self.state.snapshot = None;
                self.state.activity = Activity::default();
                trainer_ui::clear_activity();
                self.state.results = results.clone();
                self.state.searched = true;
                trainer_ui::publish_results(&results, results.len());
                trainer_ui::publish_status(format!(
                    "SEARCH {}: {} HITS",
                    vtype.name(),
                    results.len()
                ));
                log!("trainer: search {} {} -> {} hits", vtype.name(), text, results.len());
            }
            TrainerCmd::Refine { vtype, text } => {
                if !self.state.searched {
                    trainer_ui::publish_status("NO PRIOR SEARCH".to_string());
                    return;
                }
                let Some(_) = vtype.parse(&text) else {
                    trainer_ui::publish_status(format!("BAD VALUE: {}", text));
                    return;
                };
                let results = search_all(mem, vtype, &text, Some(&self.state.results));
                self.state.snapshot = None;
                self.state.activity = Activity::default();
                trainer_ui::clear_activity();
                self.state.results = results.clone();
                trainer_ui::publish_results(&results, results.len());
                trainer_ui::publish_status(format!(
                    "REFINE {}: {} HITS",
                    vtype.name(),
                    results.len()
                ));
            }
            TrainerCmd::Reset => {
                self.state.snapshot = None;
                self.state.activity = Activity::default();
                trainer_ui::clear_activity();
                self.state.results.clear();
                self.state.analysis_cursor = 0;
                self.state.searched = false;
                trainer_ui::publish_results(&[], 0);
                trainer_ui::publish_status("SEARCH RESET".to_string());
            }
            TrainerCmd::Set { vtype, text } => {
                let addr = trainer_ui::selected_address();
                let Some(addr) = addr else {
                    trainer_ui::publish_status("NO RESULT SELECTED".to_string());
                    return;
                };
                let Some((t, bits)) = self.resolve_value(mem, vtype, &text, addr) else {
                    trainer_ui::publish_status(format!("BAD VALUE: {}", text));
                    return;
                };
                let ok = t.write_at(mem, addr, bits);
                if ok {
                    record_trainer_write(mem, &mut self.state.results, addr, t.size());
                    trainer_ui::publish_live_values(&self.state.results);
                }
                trainer_ui::publish_status(if ok {
                    format!("SET 0x{:X} = {}", addr, t.format(bits))
                } else {
                    "WRITE FAILED (BAD ADDR?)".to_string()
                });
                log!("trainer: set 0x{:X} = {} ({})", addr, text, t.name());
            }
            TrainerCmd::SetAll { vtype, text, confirm, safe_mode, filter } => {
                let previous = self.state.pending_bulk.take();
                trainer_ui::publish_bulk_preview(false);
                let plan = match plan_bulk(mem, &self.state.results, vtype, &text, safe_mode, filter) {
                    Ok(plan) => plan,
                    Err(reason) => {
                        trainer_ui::publish_status(reason.to_string());
                        return;
                    }
                };
                // Re-plan even on confirmation: if anything changed, show
                // the new counts and require a fresh explicit confirmation.
                if confirm && previous.as_ref().is_some_and(|pending| {
                    pending.created_at.elapsed() < BULK_CONFIRM_TIMEOUT
                        && pending.plan == plan
                }) {
                    match apply_bulk(mem, &mut self.state.results, &plan) {
                        Ok(written) => {
                            trainer_ui::publish_live_values(&self.state.results);
                            trainer_ui::publish_status(format!(
                                "WROTE {} SKIPPED {}", written, plan.skipped,
                            ));
                            log!("trainer: bulk wrote {}, skipped {}, safe mode {}", written, plan.skipped, safe_mode);
                        }
                        Err(reason) => trainer_ui::publish_status(reason.to_string()),
                    }
                } else {
                    trainer_ui::publish_status(if safe_mode {
                        format!("CHECKED {} SKIP {}: STILL RISKY", plan.writes.len(), plan.skipped)
                    } else {
                        format!("SAFE OFF: {} WRITES / CRASH RISK", plan.writes.len())
                    });
                    self.state.pending_bulk = Some(PendingBulk {
                        plan, created_at: Instant::now(),
                    });
                    trainer_ui::publish_bulk_preview(true);
                }
            }
            TrainerCmd::Freeze { vtype, text } => {
                let Some(addr) = trainer_ui::selected_address() else {
                    trainer_ui::publish_status("NO RESULT SELECTED".to_string());
                    return;
                };
                let (t, bits) = if text.trim().is_empty() {
                    // No value typed: freeze whatever is currently there.
                    let t = self.result_type(addr, vtype);
                    (t, t.read_at(mem, addr).unwrap_or(0))
                } else {
                    let Some(pair) = self.resolve_value(mem, vtype, &text, addr) else {
                        trainer_ui::publish_status(format!("BAD VALUE: {}", text));
                        return;
                    };
                    pair
                };
                let vtype = t;
                /* frozen handled by tick */
                self.state.frozen.push(Patch {
                    addr,
                    vtype,
                    bits,
                    freeze: true,
                });
                if vtype.write_at(mem, addr, bits) {
                    record_trainer_write(mem, &mut self.state.results, addr, vtype.size());
                    trainer_ui::publish_live_values(&self.state.results);
                }
                trainer_ui::publish_frozen(self.state.frozen.len());
                trainer_ui::publish_status(format!("FREEZE 0x{:X}", addr));
            }
            TrainerCmd::UnfreezeAll => {
                let count = self.state.frozen.len();
                self.state.frozen.clear();
                trainer_ui::publish_frozen(0);
                trainer_ui::publish_status(format!("UNFROZEN {}", count));
            }
            TrainerCmd::Dump => {
                let results = self.state.results.clone();
                let status = self.write_dump(&results);
                trainer_ui::publish_status(status);
            }
            TrainerCmd::SaveHack { vtype } => {
                let Some(addr) = trainer_ui::selected_address() else {
                    trainer_ui::publish_status("NO RESULT SELECTED".to_string());
                    return;
                };
                let vtype = self.result_type(addr, vtype);
                let bits = vtype.read_at(mem, addr).unwrap_or(0);
                let frozen = self.state.frozen.iter().any(|p| p.addr == addr);
                let status = self.save_hack_line(addr, vtype, bits, frozen);
                trainer_ui::publish_status(status);
            }
            TrainerCmd::ApplyHack { addr, vtype, bits } => {
                let ok = vtype.write_at(mem, addr, bits);
                if ok {
                    record_trainer_write(mem, &mut self.state.results, addr, vtype.size());
                }
                log!(
                    "trainer: hack {} 0x{:X}={} -> {}",
                    vtype.name(),
                    addr,
                    vtype.format(bits),
                    ok
                );
            }
        }
    }

    /// Resolve the concrete type + bits for a Set/Freeze when the UI type is
    /// Auto: use the selected result's own type.
    fn resolve_value(
        &self,
        mem: &Mem,
        vtype: VType,
        text: &str,
        addr: u32,
    ) -> Option<(VType, u64)> {
        let t = self.result_type(addr, vtype);
        let bits = t.parse(text)?;
        Some((t, bits))
    }

    /// Concrete type for an address: the UI type unless it is Auto, in which
    /// case look up the type of the selected search result.
    fn result_type(&self, addr: u32, vtype: VType) -> VType {
        if vtype != VType::Auto {
            return vtype;
        }
        if let Some(t) = trainer_ui::selected_result_type(addr) {
            return t;
        }
        self.state
            .results
            .iter()
            .find(|r| r.addr == addr)
            .map(|r| r.vtype)
            .unwrap_or(VType::I32)
    }

    // --- hack files ---

    fn hacks_dir(&self) -> PathBuf {
        paths_hacks_dir()
    }

    /// Parse a hack file. Returns patches as (addr, vtype, bits, freeze).
    fn parse_hack_text(
        text: &str,
        file_app: Option<&str>,
        current_app: Option<&str>,
    ) -> Vec<(u32, VType, u64, bool)> {
        let mut patches = Vec::new();
        let mut section_app: Option<String> = None;
        for raw_line in text.lines() {
            let line = raw_line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let freeze = raw_line.to_lowercase().contains("freeze");
            // Section header: [com.example.App]
            if line.starts_with('[') && line.ends_with(']') {
                section_app = Some(line[1..line.len() - 1].trim().to_string());
                continue;
            }
            // App-scoped line: com.example.App: 0x1234=100
            let (line_app, rest) = match line.split_once(':') {
                Some((app, rest)) if app.contains('.') && !app.starts_with("0x") => {
                    (Some(app.trim().to_string()), rest.trim())
                }
                _ => (None, line),
            };
            let Some((addr_str, value_str)) = parse_addr_value(rest) else {
                continue;
            };
            // Decide which app this patch belongs to (inline tag wins over
            // section header, which wins over the file's implicit app).
            let patch_app = line_app
                .or(section_app.clone())
                .or_else(|| file_app.map(String::from));
            // Apply only patches that target this app (or untagged ones).
            match patch_app {
                Some(app) if Some(app.as_str()) != current_app => continue,
                _ => {}
            }
            let (vtype, bits) = parse_hack_value(value_str);
            if let Ok(addr) = u32::from_str_radix(addr_str.trim_start_matches("0x"), 16) {
                patches.push((addr, vtype, bits, freeze));
            }
        }
        patches
    }

    fn load_and_apply_hacks(&mut self, mem: &mut Mem) {
        let dir = self.hacks_dir();
        let current_app = self.state.app_id.clone();
        let mut watched = Vec::new();
        // Global file, all apps.
        let global_path = dir.join(GLOBAL_HACKS_FILE);
        if let Ok(text) = std::fs::read_to_string(&global_path) {
            for (addr, vtype, bits, freeze) in
                Self::parse_hack_text(&text, None, current_app.as_deref())
            {
                Self::apply_patch(mem, &mut self.state, addr, vtype, bits, freeze);
            }
        }
        let global_mtime = std::fs::metadata(&global_path)
            .and_then(|m| m.modified())
            .ok();
        watched.push((global_path, global_mtime));
        // Per-app file.
        if let Some(app_id) = current_app.clone() {
            let app_path = dir.join(format!("{}.txt", app_id));
            if let Ok(text) = std::fs::read_to_string(&app_path) {
                for (addr, vtype, bits, freeze) in
                    Self::parse_hack_text(&text, Some(&app_id), Some(&app_id))
                {
                    Self::apply_patch(mem, &mut self.state, addr, vtype, bits, freeze);
                }
            }
            let app_mtime = std::fs::metadata(&app_path)
                .and_then(|m| m.modified())
                .ok();
            watched.push((app_path, app_mtime));
        }
        self.state.watched_files = watched;
        log!("trainer: hack files loaded for {:?}", self.state.app_id);
    }

    fn apply_patch(
        mem: &mut Mem,
        state: &mut TrainerState,
        addr: u32,
        vtype: VType,
        bits: u64,
        freeze: bool,
    ) {
        if !state.applied_hacks.insert((addr, vtype, bits)) {
            return; // already applied (e.g. duplicate line)
        }
        if vtype.write_at(mem, addr, bits) {
            state.snapshot = None;
            record_trainer_write(mem, &mut state.results, addr, vtype.size());
        }
        log!(
            "trainer: hack {} 0x{:X}={}",
            vtype.name(),
            addr,
            vtype.format(bits)
        );
        if freeze {
            state.frozen.push(Patch {
                addr,
                vtype,
                bits,
                freeze: true,
            });
            trainer_ui::publish_frozen(state.frozen.len());
        }
    }

    fn reload_changed_files(&mut self, mem: &mut Mem) {
        let mut reload = false;
        for (path, mtime) in &mut self.state.watched_files {
            let current = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            if current != *mtime {
                *mtime = current;
                reload = true;
            }
        }
        if reload {
            log!("trainer: hack files changed on disk; reloading");
            self.load_and_apply_hacks(mem);
        }
    }

    fn apply_frozen(&self, mem: &mut Mem) {
        for patch in &self.state.frozen {
            if let Some(current) = patch.vtype.read_at(mem, patch.addr) {
                if current != patch.bits {
                    patch.vtype.write_at(mem, patch.addr, patch.bits);
                }
            }
        }
    }

    /// Re-read the current value at every stored result address and publish
    /// the updated list to the UI, marking rows whose value changed since the
    /// previous refresh. This makes the right address "light up" when the
    /// in-game value changes (e.g. coins are spent).
    fn refresh_live_values(&mut self, mem: &mut Mem, objc: Option<&crate::objc::ObjC>, seconds: f32) {
        if self.state.results.is_empty() {
            return;
        }
        // Include aliases of frozen values, not only exact start addresses.
        let frozen: HashSet<_> = self.state.frozen.iter().flat_map(|p| {
            (0..p.vtype.size()).filter_map(move |offset| p.addr.checked_add(offset))
        }).collect();
        self.state.activity.changed_last_sample = 0;
        let now = Instant::now();
        let mut allocations = mem.live_allocations();
        allocations.sort_unstable_by_key(|a| a.0);
        let len = self.state.results.len();
        // Rotate traversal so a flood is not permanently biased to high addresses.
        for step in 0..len {
            let i = (self.state.activity_cursor + step) % len;
            let r = &mut self.state.results[i];
            if bulk::containing_allocation(&allocations, r.addr, r.vtype.size()).is_none() {
                r.analysis = Analysis::default();
                r.changed = false;
                continue;
            }
            if let Some(current) = r.vtype.read_at(mem, r.addr) {
                if !frozen.is_empty() && (0..r.vtype.size()).filter_map(|offset| r.addr.checked_add(offset))
                    .any(|addr| frozen.contains(&addr))
                {
                    r.analysis.reset_history();
                } else {
                    if current != r.bits {
                        // Bound feed work even if every stored hit is changing.
                        if self.state.activity.changed_last_sample < 256 {
                            self.state.activity.record(Change {
                                addr: r.addr, vtype: r.vtype, before: r.bits, after: current, at: now,
                            });
                        } else {
                            self.state.activity.changed_last_sample += 1;
                        }
                    }
                    r.analysis.observe_timed(r.vtype, r.bits, current, seconds);
                }
                r.changed = current != r.bits;
                r.bits = current;
            } else {
                r.analysis = Analysis::default();
            }
        }
        self.state.activity_cursor = (self.state.activity_cursor + 67) % len;
        classify::analyze_batch_with_objects(mem, &mut self.state.results, &mut self.state.analysis_cursor, objc);
        self.refresh_watch_value(mem);
        trainer_ui::publish_activity(&self.state.activity.entries, self.state.activity.changed_last_sample, len);
        trainer_ui::publish_live_values(&self.state.results);
    }

    // --- dump & save ---

    fn write_dump(&mut self, results: &[SearchResult]) -> String {
        if results.is_empty() {
            return "NOTHING TO DUMP".to_string();
        }
        let dir = self.hacks_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return "DUMP FAILED (NO DIR)".to_string();
        }
        let app_tag = self
            .state
            .app_id
            .as_deref()
            .unwrap_or("unknown")
            .replace(['/', '\\', ':'], "_");
        self.dump_counter += 1;
        let path = dir.join(format!(
            "dump_{}_{}.txt",
            app_tag, self.dump_counter
        ));
        let mut text = String::new();
        let count = results.len().min(MAX_DUMP_LINES);
        for result in &results[..count] {
            text.push_str(&format!(
                "0x{:08X}\t{}\t{}\t{}\t{}\t{}\n",
                result.addr,
                result.vtype.name(),
                result.vtype.format(result.bits),
                format_bits_hex(result.bits),
                result.analysis.category.label(),
                result.analysis.description()
            ));
        }
        match std::fs::write(&path, text) {
            Ok(()) => {
                log!("trainer: dumped {} results to {:?}", count, path);
                format!("DUMPED {} -> {:?}", count, path.file_name().unwrap_or_default())
            }
            Err(e) => {
                log!("trainer: dump failed: {}", e);
                "DUMP WRITE FAILED".to_string()
            }
        }
    }

    fn save_hack_line(&self, addr: u32, vtype: VType, bits: u64, frozen: bool) -> String {
        let dir = self.hacks_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return "SAVE FAILED (NO DIR)".to_string();
        }
        let Some(app_id) = self.state.app_id.clone() else {
            return "NO APP (CANNOT SAVE)".to_string();
        };
        let path = dir.join(format!("{}.txt", app_id));
        let line = format!(
            "0x{:X}={} # {}{}",
            addr,
            vtype.format(bits),
            vtype.name(),
            if frozen { " freeze" } else { "" }
        );
        use std::io::Write;
        let result = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| writeln!(f, "{}", line));
        match result {
            Ok(()) => {
                log!("trainer: saved hack to {:?}: {}", path, line);
                format!("SAVED {:?}", path.file_name().unwrap_or_default())
            }
            Err(e) => {
                log!("trainer: save hack failed: {}", e);
                "SAVE FAILED".to_string()
            }
        }
    }
}

fn format_bits_hex(bits: u64) -> String {
    match bits {
        0..=0xFF => format!("0x{:02X}", bits),
        0..=0xFFFF => format!("0x{:04X}", bits),
        0..=0xFFFF_FFFF => format!("0x{:08X}", bits),
        _ => format!("0x{:016X}", bits),
    }
}

/// Parse `0x1234=100` (or `1234=100`, or `0x1234=3.14`).
fn parse_addr_value(rest: &str) -> Option<(&str, &str)> {
    let rest = rest.trim();
    let eq = rest.find('=')?;
    let addr = rest[..eq].trim();
    let value = rest[eq + 1..].trim();
    if addr.is_empty() || value.is_empty() {
        return None;
    }
    Some((addr, value))
}

/// Parse the value part of a hack line. Auto-detects: floats (contains '.'
/// or 'e' and parses as f32), negative integers, decimal integers, and
/// hex-integers (0x...). Defaults to I32 for plain integers.
fn parse_hack_value(text: &str) -> (VType, u64) {
    let text = text.trim();
    if let Ok(f) = text.parse::<f32>() {
        if text.contains('.') || text.contains('e') || text.contains('E') {
            return (VType::F32, f.to_bits() as u64);
        }
    }
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        if let Ok(bits) = u64::from_str_radix(hex, 16) {
            // Bare hex values are treated as raw bit patterns for I32.
            return (VType::I32, bits & 0xFFFF_FFFF);
        }
    }
    if let Ok(v) = text.parse::<i64>() {
        return (VType::I32, (v as u64) & 0xFFFF_FFFF);
    }
    if let Ok(v) = text.parse::<u64>() {
        return (VType::I32, v & 0xFFFF_FFFF);
    }
    (VType::I32, 0)
}

/// Keep observations honest: edits performed by the trainer (including
/// aliases of the edited byte range) are not evidence of in-game behaviour.
fn record_trainer_write(mem: &Mem, results: &mut [SearchResult], addr: u32, size: u32) {
    let end = addr as u64 + size as u64;
    for r in results {
        if (r.addr as u64) < end && (addr as u64) < r.addr as u64 + r.vtype.size() as u64 {
            if let Some(bits) = r.vtype.read_at(mem, r.addr) { r.bits = bits; }
            r.changed = false;
            r.analysis.reset_history();
        }
    }
}

/// A fresh search and an empty refinement are different operations: an
/// exhausted refinement must never silently restart a whole-memory search.
fn search_all(
    mem: &Mem,
    vtype: VType,
    text: &str,
    previous: Option<&[SearchResult]>,
) -> Vec<SearchResult> {
    if let Some(previous) = previous {
        return previous.iter().filter_map(|result| {
            let t = if vtype == VType::Auto { result.vtype } else { vtype };
            let wanted = t.parse(text)?;
            let bits = t.read_at(mem, result.addr)?;
            (bits == wanted).then_some(SearchResult {
                addr: result.addr,
                vtype: t,
                bits,
                changed: false,
                analysis: if t == result.vtype { result.analysis } else { Analysis::default() },
            })
        }).take(MAX_RESULTS).collect();
    }
    if vtype != VType::Auto {
        return vtype.parse(text).map_or_else(Vec::new, |bits| scan_bits(mem, vtype, bits));
    }

    let mut results = Vec::new();
    // One entry per address and bit pattern: the first type in `auto_types()`
    // order that matches exactly wins. A narrow exact match is preferred over
    // a wider one because writing it cannot clobber the neighbouring bytes,
    // and the address is what locates the field. That does not hide 64-bit
    // storage: a `double` or an integer above 2^32 has a bit pattern no
    // 32-bit type can produce, so only its 64-bit type matches at all.
    let mut seen = HashSet::new();
    for &t in auto_types() {
        // E.g. searching for 600 must not also search for U8(88), and
        // F32(600) must use 600.0's IEEE bits, not the integer bit pattern.
        let Some(pattern) = t.parse(text) else { continue };
        for hit in scan_bits(mem, t, pattern) {
            if seen.insert((hit.addr, hit.bits)) {
                results.push(hit);
                if results.len() >= MAX_RESULTS {
                    return results;
                }
            }
        }
    }
    results
}

/// Directory for hack/dump files: `<user data>/touchHLE_hacks/`.
fn paths_hacks_dir() -> PathBuf {
    crate::paths::user_data_base_path().join(HACKS_DIR)
}

/// Types probed by an Auto search, in coverage order.
fn auto_types() -> &'static [VType] {
    // Ordered 32-bit integers first: that is what games store money in, so it
    // wins the per-address tie-break in `search_all`. The 64-bit types have to
    // be in here, or AUTO can never match a `double`/`long long` value that no
    // 32-bit pattern can produce (Asphalt 8 stores its gold as one of those).
    &[
        VType::I32,
        VType::U32,
        VType::I16,
        VType::U16,
        VType::I8,
        VType::U8,
        VType::F32,
        VType::F64,
        VType::U64,
        VType::I64,
    ]
}

/// Single-type byte scan (the concrete-type part of `search_all`).
fn scan_bits(mem: &Mem, vtype: VType, want_bits: u64) -> Vec<SearchResult> {
    let size = vtype.size() as usize;
    let mut results = Vec::new();
    for (addr, alloc_size) in mem.live_allocations() {
        if addr < mem.null_segment_size()
            || alloc_size < size as GuestUSize
            || alloc_size > MAX_SCAN_ALLOCATION
        {
            continue;
        }
        let bytes = match mem.get_bytes_fallible(
            ConstVoidPtr::from_bits(addr as _),
            alloc_size,
        ) {
            Some(bytes) => bytes,
            None => continue,
        };
        let base = addr as u32;
        let last = bytes.len() - size;
        let mut offset = 0usize;
        while offset <= last {
            if VType::read_le(bytes, offset, size) == want_bits {
                results.push(SearchResult {
                    addr: base + offset as u32,
                    vtype,
                    bits: want_bits,
                    changed: false,
                    analysis: Analysis::default(),
                });
                if results.len() >= MAX_RESULTS {
                    return results;
                }
            }
            offset += 1;
        }
    }
    results
}

#[cfg(test)]
mod tests;
