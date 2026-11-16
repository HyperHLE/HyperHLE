/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Repair the pinned Dynarmic ARM64 memory-abort emitter in a private copy.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const EMITTER: &str = "src/dynarmic/backend/arm64/emit_arm64_a32.cpp";
const OLD: &str = "    code.LDAR(Xscratch0, Xhalt);\n    code.TST(Xscratch0, static_cast<u32>(HaltReason::MemoryAbort));";
const FIXED: &str = "    code.LDAR(Wscratch0, Xhalt);\n    code.TST(Wscratch0, static_cast<u32>(HaltReason::MemoryAbort));";

pub fn prepare(source: &Path, destination: &Path) -> io::Result<PathBuf> {
    let emitter = source.join(EMITTER);
    let original = fs::read_to_string(&emitter)?.replace("\r\n", "\n");
    if original.contains(FIXED) && !original.contains(OLD) {
        return Ok(source.to_owned());
    }
    if original.matches(OLD).count() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Dynarmic memory-abort emitter changed; review its 32-bit halt-flag access",
        ));
    }
    // HaltReason is a 32-bit atomic. An LDAR X reads eight bytes and faults
    // when its address is only four-byte aligned, as on AArch64 Linux.
    // Preserve the debug abort check and use the correct register width.
    let fixed = original.replace(OLD, FIXED);
    copy_tree(source, destination, &emitter, fixed.as_bytes())?;
    Ok(destination.to_owned())
}

fn copy_tree(source: &Path, destination: &Path, emitter: &Path, fixed: &[u8]) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(destination)? {
        let entry = entry?;
        if entry.file_name() == ".git" || !source.join(entry.file_name()).exists() {
            if entry.path().is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to, emitter, fixed)?;
        } else {
            let bytes = if from == emitter {
                fixed.to_vec()
            } else {
                fs::read(&from)?
            };
            // Avoid changing timestamps and recompiling untouched C++ files.
            if fs::read(&to).ok().as_deref() != Some(bytes.as_slice()) {
                fs::write(&to, bytes)?;
                fs::set_permissions(to, fs::metadata(from)?.permissions())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_copy_preserves_source_and_incremental_builds() {
        let root =
            std::env::temp_dir().join(format!("hyperhle-dynarmic-patch-{}", std::process::id()));
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir_all(source.join(EMITTER).parent().unwrap()).unwrap();
        fs::write(source.join(EMITTER), OLD).unwrap();
        fs::write(source.join(".git"), "gitdir: unused").unwrap();
        fs::write(source.join("other.cpp"), "original").unwrap();
        assert_eq!(prepare(&source, &target).unwrap(), target);
        assert_eq!(fs::read_to_string(source.join(EMITTER)).unwrap(), OLD);
        assert_eq!(fs::read_to_string(target.join(EMITTER)).unwrap(), FIXED);
        assert!(!target.join(".git").exists());
        let modified = fs::metadata(target.join(EMITTER))
            .unwrap()
            .modified()
            .unwrap();
        fs::write(source.join("other.cpp"), "updated").unwrap();
        prepare(&source, &target).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("other.cpp")).unwrap(),
            "updated"
        );
        assert_eq!(
            fs::metadata(target.join(EMITTER))
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
        fs::write(source.join(EMITTER), FIXED).unwrap();
        assert_eq!(prepare(&source, &target).unwrap(), source);
        fs::write(source.join(EMITTER), OLD.replace('\n', "\r\n")).unwrap();
        assert_eq!(prepare(&source, &target).unwrap(), target);
        assert_eq!(fs::read_to_string(target.join(EMITTER)).unwrap(), FIXED);
        fs::write(source.join(EMITTER), "unknown emitter").unwrap();
        assert_eq!(
            prepare(&source, &target).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::remove_dir_all(root).unwrap();
    }
}
