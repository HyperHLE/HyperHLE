/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Beni, NOT Own Cheat Engine
//!
//! Built-in cheat / debug engine for fuckingHLEforCoolGames.
//! Operates exclusively on emulated guest memory. Disabled by default;
//! when unused it has essentially zero effect on normal execution.

use crate::cpu::Cpu;
use crate::mem::{GuestUSize, Mem, MutPtr, Ptr};
use crate::paths;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Value types the scanner / watches understand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float32,
    Float64,
}

impl ValueType {
    pub fn size(self) -> GuestUSize {
        match self {
            ValueType::Int8 | ValueType::UInt8 => 1,
            ValueType::Int16 | ValueType::UInt16 => 2,
            ValueType::Int32 | ValueType::UInt32 | ValueType::Float32 => 4,
            ValueType::Int64 | ValueType::UInt64 | ValueType::Float64 => 8,
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "i8" | "int8" => Some(ValueType::Int8),
            "i16" | "int16" => Some(ValueType::Int16),
            "i32" | "int32" => Some(ValueType::Int32),
            "i64" | "int64" => Some(ValueType::Int64),
            "u8" | "uint8" => Some(ValueType::UInt8),
            "u16" | "uint16" => Some(ValueType::UInt16),
            "u32" | "uint32" => Some(ValueType::UInt32),
            "u64" | "uint64" => Some(ValueType::UInt64),
            "f32" | "float" | "float32" => Some(ValueType::Float32),
            "f64" | "double" | "float64" => Some(ValueType::Float64),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ValueType::Int8 => "Int8",
            ValueType::Int16 => "Int16",
            ValueType::Int32 => "Int32",
            ValueType::Int64 => "Int64",
            ValueType::UInt8 => "UInt8",
            ValueType::UInt16 => "UInt16",
            ValueType::UInt32 => "UInt32",
            ValueType::UInt64 => "UInt64",
            ValueType::Float32 => "Float32",
            ValueType::Float64 => "Float64",
        }
    }
}

/// How a scan compares values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanMode {
    Exact,
    Unknown,
    Increased,
    Decreased,
    Changed,
    Unchanged,
}

impl ScanMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "exact" | "=" => Some(ScanMode::Exact),
            "unknown" | "?" => Some(ScanMode::Unknown),
            "increased" | "inc" | "+" => Some(ScanMode::Increased),
            "decreased" | "dec" | "-" => Some(ScanMode::Decreased),
            "changed" | "~" => Some(ScanMode::Changed),
            "unchanged" | "same" => Some(ScanMode::Unchanged),
            _ => None,
        }
    }
}

/// A single scan hit (guest address + last seen raw bytes).
#[derive(Clone, Debug)]
pub struct ScanResult {
    pub address: u32,
    pub bytes: Vec<u8>,
}

/// A watched / frozen guest address.
#[derive(Clone, Debug)]
pub struct WatchEntry {
    pub address: u32,
    pub value_type: ValueType,
    pub name: String,
    pub frozen: bool,
    /// Raw bytes to write when frozen (little-endian).
    pub frozen_bytes: Vec<u8>,
}

/// A named toggleable cheat belonging to a profile.
#[derive(Clone, Debug)]
pub struct CheatEntry {
    pub name: String,
    pub enabled: bool,
    /// Optional fixed addresses this cheat freezes (address, type, value bytes).
    pub freezes: Vec<(u32, ValueType, Vec<u8>)>,
    /// Optional speed multiplier this cheat applies while enabled (1.0 = none).
    pub speed_multiplier: Option<f64>,
}

/// Per-game profile stored under user data.
#[derive(Clone, Debug, Default)]
pub struct CheatProfile {
    pub game_id: String,
    pub cheats: Vec<CheatEntry>,
}

/// Runtime diagnostics snapshot.
#[derive(Clone, Debug, Default)]
pub struct Diagnostics {
    pub guest_pc: u32,
    pub guest_lr: u32,
    pub frame: u64,
    pub emulated_time_secs: f64,
    pub architecture: &'static str,
    pub active_cheats: usize,
    pub speed: f64,
    pub paused: bool,
}

/// The full Beni engine state. Lives inside Environment.
pub struct Beni {
    /// Master switch. When false, every public method is a no-op.
    pub enabled: bool,
    /// Guest execution speed multiplier. 1.0 = normal.
    pub speed: f64,
    /// When true the main loop should not advance guest time/CPU.
    pub paused: bool,
    /// Request a single frame step then re-pause.
    pub step_frame: bool,
    /// Current scan value type.
    pub scan_type: ValueType,
    /// Current scan mode.
    pub scan_mode: ScanMode,
    /// Exact-value target (as f64; integers are truncated on write).
    pub scan_value: f64,
    /// Results of the last scan (capped).
    pub scan_results: Vec<ScanResult>,
    /// Previous scan snapshot for relative searches.
    previous_snapshot: HashMap<u32, Vec<u8>>,
    /// Live watch list.
    pub watches: Vec<WatchEntry>,
    /// Active profile for the current game.
    pub profile: CheatProfile,
    /// Frame counter for diagnostics.
    pub frame_count: u64,
    /// Whether the status panel should be printed periodically.
    pub show_ui: bool,
    /// Max results kept from a scan (prevents OOM on unknown scans).
    max_results: usize,
}

impl Default for Beni {
    fn default() -> Self {
        Beni {
            enabled: false,
            speed: 1.0,
            paused: false,
            step_frame: false,
            scan_type: ValueType::Int32,
            scan_mode: ScanMode::Exact,
            scan_value: 0.0,
            scan_results: Vec::new(),
            previous_snapshot: HashMap::new(),
            watches: Vec::new(),
            profile: CheatProfile::default(),
            frame_count: 0,
            show_ui: false,
            max_results: 10_000,
        }
    }
}

impl Beni {
    pub fn new(enabled: bool) -> Self {
        let mut b = Beni::default();
        b.enabled = enabled;
        b
    }

    /// Preset speed multipliers.
    pub fn set_speed_preset(&mut self, preset: &str) {
        if !self.enabled {
            return;
        }
        let v = match preset {
            "0.25" | "0.25x" => 0.25,
            "0.5" | "0.5x" => 0.5,
            "1" | "1x" | "1.0" => 1.0,
            "1.5" | "1.5x" => 1.5,
            "2" | "2x" => 2.0,
            "4" | "4x" => 4.0,
            "8" | "8x" => 8.0,
            other => other.parse::<f64>().unwrap_or(1.0).clamp(0.01, 64.0),
        };
        self.speed = v;
        log!("[Beni] Speed set to {:.2}x", self.speed);
    }

    pub fn pause(&mut self) {
        if self.enabled {
            self.paused = true;
            log!("[Beni] Paused");
        }
    }

    pub fn resume(&mut self) {
        if self.enabled {
            self.paused = false;
            self.step_frame = false;
            log!("[Beni] Resumed");
        }
    }

    pub fn request_step_frame(&mut self) {
        if self.enabled {
            self.step_frame = true;
            self.paused = false;
            log!("[Beni] Step frame");
        }
    }

    /// Called once per presented frame from the GLES present path.
    pub fn on_frame(&mut self, mem: &mut Mem, cpu: &Cpu, startup_secs: f64) {
        if !self.enabled {
            return;
        }
        self.frame_count = self.frame_count.wrapping_add(1);

        // Apply freezes from watches.
        for w in &self.watches {
            if w.frozen && !w.frozen_bytes.is_empty() {
                self.write_bytes_safe(mem, w.address, &w.frozen_bytes);
            }
        }

        // Apply freezes from enabled profile cheats.
        for cheat in &self.profile.cheats {
            if !cheat.enabled {
                continue;
            }
            for (addr, _ty, bytes) in &cheat.freezes {
                self.write_bytes_safe(mem, *addr, bytes);
            }
        }

        // If we were stepping, re-pause after this frame.
        if self.step_frame {
            self.step_frame = false;
            self.paused = true;
        }

        if self.show_ui && self.frame_count % 60 == 0 {
            self.print_status(cpu, startup_secs);
        }
    }

    /// Effective speed after active cheats (product of multipliers).
    pub fn effective_speed(&self) -> f64 {
        if !self.enabled {
            return 1.0;
        }
        let mut s = self.speed;
        for c in &self.profile.cheats {
            if c.enabled {
                if let Some(m) = c.speed_multiplier {
                    s *= m;
                }
            }
        }
        s.clamp(0.01, 64.0)
    }

    /// Whether the guest should be held (paused).
    pub fn should_pause(&self) -> bool {
        self.enabled && self.paused
    }

    // ------------------------------------------------------------------
    // Memory scanner
    // ------------------------------------------------------------------

    pub fn first_scan(&mut self, mem: &Mem) {
        if !self.enabled {
            return;
        }
        self.scan_results.clear();
        self.previous_snapshot.clear();
        let size = self.scan_type.size();
        let target = self.encode_value(self.scan_value);

        // Scan first 256 MiB of guest address space via fallible API.
        let mem_len = mem_bytes_len(mem);
        let mut addr: u32 = 0x1000; // skip null page
        while (addr as usize) + (size as usize) <= mem_len {
            if let Some(bytes) = read_bytes_safe(mem, addr, size) {
                let hit = match self.scan_mode {
                    ScanMode::Exact => bytes == target,
                    ScanMode::Unknown => true,
                    ScanMode::Increased
                    | ScanMode::Decreased
                    | ScanMode::Changed
                    | ScanMode::Unchanged => true,
                };
                if hit {
                    self.previous_snapshot.insert(addr, bytes.clone());
                    if self.scan_results.len() < self.max_results {
                        self.scan_results.push(ScanResult {
                            address: addr,
                            bytes,
                        });
                    }
                }
            }
            addr = addr.saturating_add(size);
            if addr == 0 {
                break;
            }
        }
        log!(
            "[Beni] First scan ({:?} {:?}): {} hits (capped at {})",
            self.scan_mode,
            self.scan_type,
            self.scan_results.len(),
            self.max_results
        );
    }

    pub fn next_scan(&mut self, mem: &Mem) {
        if !self.enabled {
            return;
        }
        if self.previous_snapshot.is_empty() {
            log!("[Beni] No previous scan — run First Scan first");
            return;
        }
        let size = self.scan_type.size();
        let target = self.encode_value(self.scan_value);
        let mut new_results = Vec::new();
        let mut new_snapshot = HashMap::new();

        for (addr, old_bytes) in &self.previous_snapshot {
            if let Some(bytes) = read_bytes_safe(mem, *addr, size) {
                let hit = match self.scan_mode {
                    ScanMode::Exact => bytes == target,
                    ScanMode::Unknown => true,
                    ScanMode::Increased => compare_bytes(&bytes, old_bytes, self.scan_type) > 0,
                    ScanMode::Decreased => compare_bytes(&bytes, old_bytes, self.scan_type) < 0,
                    ScanMode::Changed => bytes != *old_bytes,
                    ScanMode::Unchanged => bytes == *old_bytes,
                };
                if hit {
                    new_snapshot.insert(*addr, bytes.clone());
                    if new_results.len() < self.max_results {
                        new_results.push(ScanResult {
                            address: *addr,
                            bytes,
                        });
                    }
                }
            }
        }
        self.scan_results = new_results;
        self.previous_snapshot = new_snapshot;
        log!(
            "[Beni] Next scan ({:?}): {} hits remaining",
            self.scan_mode,
            self.scan_results.len()
        );
    }

    // ------------------------------------------------------------------
    // Watch list
    // ------------------------------------------------------------------

    pub fn add_watch(&mut self, address: u32, value_type: ValueType, name: String) {
        if !self.enabled {
            return;
        }
        self.watches.push(WatchEntry {
            address,
            value_type,
            name,
            frozen: false,
            frozen_bytes: Vec::new(),
        });
        log!("[Beni] Watch added: {:#010x} ({})", address, value_type.as_str());
    }

    pub fn remove_watch(&mut self, address: u32) {
        self.watches.retain(|w| w.address != address);
    }

    pub fn freeze(&mut self, address: u32, mem: &Mem) {
        if !self.enabled {
            return;
        }
        if let Some(w) = self.watches.iter_mut().find(|w| w.address == address) {
            if let Some(bytes) = read_bytes_safe(mem, address, w.value_type.size()) {
                w.frozen_bytes = bytes;
                w.frozen = true;
                log!("[Beni] Frozen {:#010x}", address);
            }
        }
    }

    pub fn unfreeze(&mut self, address: u32) {
        if let Some(w) = self.watches.iter_mut().find(|w| w.address == address) {
            w.frozen = false;
            log!("[Beni] Unfrozen {:#010x}", address);
        }
    }

    pub fn edit_value(&mut self, mem: &mut Mem, address: u32, value: f64) {
        if !self.enabled {
            return;
        }
        let ty = self
            .watches
            .iter()
            .find(|w| w.address == address)
            .map(|w| w.value_type)
            .unwrap_or(self.scan_type);
        let bytes = encode_typed(value, ty);
        self.write_bytes_safe(mem, address, &bytes);
        if let Some(w) = self.watches.iter_mut().find(|w| w.address == address) {
            if w.frozen {
                w.frozen_bytes = bytes;
            }
        }
        log!("[Beni] Wrote {} to {:#010x}", value, address);
    }

    // ------------------------------------------------------------------
    // Profiles
    // ------------------------------------------------------------------

    pub fn load_profile(&mut self, game_id: &str) {
        self.profile = CheatProfile {
            game_id: game_id.to_string(),
            cheats: Vec::new(),
        };
        let path = profile_path(game_id);
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let parts: Vec<&str> = line.split('|').collect();
                if parts.len() < 2 {
                    continue;
                }
                let name = parts[0].to_string();
                let enabled = parts[1] == "1" || parts[1].eq_ignore_ascii_case("true");
                let speed = parts.get(2).and_then(|s| s.parse().ok());
                let mut freezes = Vec::new();
                if let Some(freeze_str) = parts.get(3) {
                    for entry in freeze_str.split(';') {
                        let bits: Vec<&str> = entry.split(':').collect();
                        if bits.len() == 3 {
                            if let (Ok(addr), Some(ty), Ok(val)) = (
                                u32::from_str_radix(bits[0].trim_start_matches("0x"), 16),
                                ValueType::from_str(bits[1]),
                                bits[2].parse::<f64>(),
                            ) {
                                freezes.push((addr, ty, encode_typed(val, ty)));
                            }
                        }
                    }
                }
                self.profile.cheats.push(CheatEntry {
                    name,
                    enabled,
                    freezes,
                    speed_multiplier: speed,
                });
            }
            log!(
                "[Beni] Loaded profile '{}' ({} cheats) from {}",
                game_id,
                self.profile.cheats.len(),
                path.display()
            );
        } else {
            log!("[Beni] No profile for '{}' (will create on save)", game_id);
        }
    }

    pub fn save_profile(&self) {
        if self.profile.game_id.is_empty() {
            return;
        }
        let path = profile_path(&self.profile.game_id);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut out = String::from("# Beni cheat profile\n# name|enabled|speed|addr:type:value;...\n");
        for c in &self.profile.cheats {
            let speed = c
                .speed_multiplier
                .map(|s| s.to_string())
                .unwrap_or_default();
            let freezes: Vec<String> = c
                .freezes
                .iter()
                .map(|(a, t, b)| {
                    format!("{:x}:{}:{}", a, t.as_str(), decode_typed(b, *t))
                })
                .collect();
            out.push_str(&format!(
                "{}|{}|{}|{}\n",
                c.name,
                if c.enabled { "1" } else { "0" },
                speed,
                freezes.join(";")
            ));
        }
        match fs::write(&path, out) {
            Ok(()) => log!("[Beni] Profile saved to {}", path.display()),
            Err(e) => log!("[Beni] Failed to save profile: {}", e),
        }
    }

    pub fn toggle_cheat(&mut self, index: usize) {
        if let Some(c) = self.profile.cheats.get_mut(index) {
            c.enabled = !c.enabled;
            log!(
                "[Beni] Cheat '{}' {}",
                c.name,
                if c.enabled { "ON" } else { "OFF" }
            );
        }
    }

    // ------------------------------------------------------------------
    // Diagnostics / UI dump
    // ------------------------------------------------------------------

    pub fn diagnostics(&self, cpu: &Cpu, startup_secs: f64) -> Diagnostics {
        let regs = cpu.regs();
        Diagnostics {
            guest_pc: regs[15],
            guest_lr: regs[14],
            frame: self.frame_count,
            emulated_time_secs: startup_secs,
            architecture: "ARMv6/ARMv7",
            active_cheats: self.profile.cheats.iter().filter(|c| c.enabled).count(),
            speed: self.effective_speed(),
            paused: self.paused,
        }
    }

    pub fn print_status(&self, cpu: &Cpu, startup_secs: f64) {
        let d = self.diagnostics(cpu, startup_secs);
        log!("========== Beni, NOT Own Cheat Engine ==========");
        log!("Game: {}", self.profile.game_id);
        log!(
            "Speed: {:.2}x  |  Paused: {}  |  Frame: {}",
            d.speed,
            d.paused,
            d.frame
        );
        log!(
            "PC: {:#010x}  LR: {:#010x}  Time: {:.1}s  Arch: {}",
            d.guest_pc,
            d.guest_lr,
            d.emulated_time_secs,
            d.architecture
        );
        log!(
            "-- Scanner ({:?} {:?}) hits: {} --",
            self.scan_mode,
            self.scan_type,
            self.scan_results.len()
        );
        for (i, r) in self.scan_results.iter().take(16).enumerate() {
            log!(
                "  [{:2}] {:#010x} = {}",
                i,
                r.address,
                decode_typed(&r.bytes, self.scan_type)
            );
        }
        if self.scan_results.len() > 16 {
            log!("  ... +{} more", self.scan_results.len() - 16);
        }
        log!("-- Watch List --");
        for w in &self.watches {
            log!(
                "  {:#010x} | {} | {} | {}",
                w.address,
                w.value_type.as_str(),
                w.name,
                if w.frozen { "FROZEN" } else { "live" }
            );
        }
        log!("-- Cheats --");
        for (i, c) in self.profile.cheats.iter().enumerate() {
            log!(
                "  [{}] {} {}",
                i,
                if c.enabled { "ON " } else { "OFF" },
                c.name
            );
        }
        log!("================================================");
    }

    pub fn toggle_ui(&mut self) {
        self.show_ui = !self.show_ui;
        log!(
            "[Beni] UI dump {}",
            if self.show_ui {
                "ON (every 60 frames)"
            } else {
                "OFF"
            }
        );
    }

    // ------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------

    fn encode_value(&self, v: f64) -> Vec<u8> {
        encode_typed(v, self.scan_type)
    }

    fn write_bytes_safe(&self, mem: &mut Mem, addr: u32, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let ptr: MutPtr<u8> = Ptr::from_bits(addr);
        // bytes_at_mut already null-checks / OOB-checks and redirects safely.
        let slice = mem.bytes_at_mut(ptr, bytes.len() as GuestUSize);
        let n = slice.len().min(bytes.len());
        slice[..n].copy_from_slice(&bytes[..n]);
    }
}

fn encode_typed(v: f64, ty: ValueType) -> Vec<u8> {
    match ty {
        ValueType::Int8 => (v as i8).to_le_bytes().to_vec(),
        ValueType::Int16 => (v as i16).to_le_bytes().to_vec(),
        ValueType::Int32 => (v as i32).to_le_bytes().to_vec(),
        ValueType::Int64 => (v as i64).to_le_bytes().to_vec(),
        ValueType::UInt8 => (v as u8).to_le_bytes().to_vec(),
        ValueType::UInt16 => (v as u16).to_le_bytes().to_vec(),
        ValueType::UInt32 => (v as u32).to_le_bytes().to_vec(),
        ValueType::UInt64 => (v as u64).to_le_bytes().to_vec(),
        ValueType::Float32 => (v as f32).to_le_bytes().to_vec(),
        ValueType::Float64 => v.to_le_bytes().to_vec(),
    }
}

fn decode_typed(bytes: &[u8], ty: ValueType) -> f64 {
    match ty {
        ValueType::Int8 => {
            if bytes.len() >= 1 {
                i8::from_le_bytes([bytes[0]]) as f64
            } else {
                0.0
            }
        }
        ValueType::Int16 => {
            if bytes.len() >= 2 {
                i16::from_le_bytes([bytes[0], bytes[1]]) as f64
            } else {
                0.0
            }
        }
        ValueType::Int32 => {
            if bytes.len() >= 4 {
                i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64
            } else {
                0.0
            }
        }
        ValueType::Int64 => {
            if bytes.len() >= 8 {
                let mut b = [0u8; 8];
                b.copy_from_slice(&bytes[..8]);
                i64::from_le_bytes(b) as f64
            } else {
                0.0
            }
        }
        ValueType::UInt8 => bytes.first().copied().unwrap_or(0) as f64,
        ValueType::UInt16 => {
            if bytes.len() >= 2 {
                u16::from_le_bytes([bytes[0], bytes[1]]) as f64
            } else {
                0.0
            }
        }
        ValueType::UInt32 => {
            if bytes.len() >= 4 {
                u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64
            } else {
                0.0
            }
        }
        ValueType::UInt64 => {
            if bytes.len() >= 8 {
                let mut b = [0u8; 8];
                b.copy_from_slice(&bytes[..8]);
                u64::from_le_bytes(b) as f64
            } else {
                0.0
            }
        }
        ValueType::Float32 => {
            if bytes.len() >= 4 {
                f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64
            } else {
                0.0
            }
        }
        ValueType::Float64 => {
            if bytes.len() >= 8 {
                let mut b = [0u8; 8];
                b.copy_from_slice(&bytes[..8]);
                f64::from_le_bytes(b)
            } else {
                0.0
            }
        }
    }
}

fn compare_bytes(a: &[u8], b: &[u8], ty: ValueType) -> i32 {
    let av = decode_typed(a, ty);
    let bv = decode_typed(b, ty);
    match av.partial_cmp(&bv) {
        Some(std::cmp::Ordering::Greater) => 1,
        Some(std::cmp::Ordering::Less) => -1,
        _ => 0,
    }
}

fn read_bytes_safe(mem: &Mem, addr: u32, size: GuestUSize) -> Option<Vec<u8>> {
    let ptr: Ptr<u8, false> = Ptr::from_bits(addr);
    mem.get_bytes_fallible(ptr.cast_void(), size)
        .map(|s| s.to_vec())
}

fn mem_bytes_len(_mem: &Mem) -> usize {
    // Scan the first 256 MiB of the guest address space.
    0x1000_0000usize
}

fn profile_path(game_id: &str) -> PathBuf {
    let safe: String = game_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    paths::user_data_base_path()
        .join("beni_profiles")
        .join(format!("{}.txt", safe))
}
