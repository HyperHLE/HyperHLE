/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
use std::env;
use std::path::Path;

/*fn rerun_if_changed(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.to_str().unwrap());
}*/
fn link_search(path: &Path) {
    println!("cargo:rustc-link-search=native={}", path.to_str().unwrap());
}
fn link_lib(lib: &str) {
    if cfg!(feature = "static") {
        println!("cargo:rustc-link-lib=static={lib}");
    } else {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }
}

fn link_framework(framework: &str) {
    println!("cargo:rustc-link-lib=framework={framework}");
}

/// Best-effort check for whether a shared library (e.g. `sndio`) is installed
/// on the host Linux system, so we only emit a `-l<lib>` flag the linker can
/// actually satisfy. Checks the common library directories and any paths in
/// `LD_LIBRARY_PATH`/`LIBRARY_PATH`.
fn linux_has_lib(lib: &str) -> bool {
    let filename = format!("lib{lib}.so");
    let mut dirs: Vec<String> = vec![
        "/usr/lib".into(),
        "/usr/lib64".into(),
        "/usr/lib/x86_64-linux-gnu".into(),
        "/usr/lib/aarch64-linux-gnu".into(),
        "/lib".into(),
        "/lib64".into(),
        "/usr/local/lib".into(),
        "/usr/local/lib64".into(),
    ];
    for var in ["LD_LIBRARY_PATH", "LIBRARY_PATH"] {
        if let Ok(val) = env::var(var) {
            dirs.extend(val.split(':').map(|s| s.to_string()));
        }
    }
    dirs.iter().any(|dir| {
        let base = Path::new(dir);
        // Match `libX.so` or any versioned `libX.so.N` in the directory.
        if base.join(&filename).exists() {
            return true;
        }
        std::fs::read_dir(base)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(&format!("{filename}."))
                })
            })
            .unwrap_or(false)
    })
}

fn main() {
    let os = env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS was not set");
    if cfg!(feature = "static") {
        let package_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = package_root.join("../../..");

        let mut build = cmake::Config::new(workspace_root.join("vendor/openal-soft"));

        // Prevent any out dir re-use for Android. For some reason, this causes
        // weird linker errors when cross-compiling on macOS.
        let out_dir = Path::new(&env::var("OUT_DIR").unwrap()).join(&os);
        if os.eq_ignore_ascii_case("android") {
            let _ = std::fs::remove_dir_all(&out_dir);
        }
        build.out_dir(out_dir);

        build.define("LIBTYPE", "STATIC");

        // Don't build extras, we don't need them and they can encounter issues
        // when cross-compiling.
        build.define("ALSOFT_UTILS", "OFF");
        build.define("ALSOFT_NO_CONFIG_UTIL", "ON");
        build.define("ALSOFT_EXAMPLES", "OFF");

        if os.eq_ignore_ascii_case("linux") {
            // Make Linux release builds actually include normal desktop audio.
            // Without these, OpenAL Soft can silently build with only sndio/oss/null/wave,
            // which makes Pulse/ALSA impossible to use at runtime.
            build.define("ALSOFT_BACKEND_PULSEAUDIO", "ON");
            build.define("ALSOFT_BACKEND_ALSA", "ON");

            // Fail the build if the common Linux audio backends are missing,
            // instead of producing a no-sound binary.
            build.define("ALSOFT_REQUIRE_PULSEAUDIO", "ON");
            build.define("ALSOFT_REQUIRE_ALSA", "ON");
        }

        let openal_soft_out = build.build();

        link_search(&openal_soft_out.join("lib"));
        // some Linux systems
        link_search(&openal_soft_out.join("lib64"));

        // Some dependencies of OpenAL Soft.
        if os.eq_ignore_ascii_case("linux") {
            // Linux OpenAL Soft backend dependencies for static OpenAL builds.
            // PulseAudio and ALSA are required above (ALSOFT_REQUIRE_*), so
            // they are always linked.
            println!("cargo:rustc-link-lib=dylib=pulse");
            println!("cargo:rustc-link-lib=dylib=asound");

            // sndio is optional: OpenAL Soft only builds its sndio backend when
            // the library is present at configure time. Unconditionally
            // emitting `-lsndio` breaks the final link on the many Linux
            // distributions that don't ship it (e.g. Amazon Linux, RHEL).
            // Only request it when we can actually find the shared object.
            if linux_has_lib("sndio") {
                println!("cargo:rustc-link-lib=dylib=sndio");
            }
        }
        if os.eq_ignore_ascii_case("android") {
            println!("cargo:rustc-link-lib=dylib=OpenSLES");
        }
        if os.eq_ignore_ascii_case("macos") {
            link_framework("AudioToolbox");
            link_framework("CoreAudio");
            link_framework("CoreFoundation");
        }
    }

    // see also src/audio/openal.rs
    link_lib(if os.eq_ignore_ascii_case("windows") {
        "OpenAL32"
    } else {
        "openal"
    });
    // rerun-if-changed seems to not work if pointed to a directory :(
    //rerun_if_changed(&workspace_root.join("vendor/openal-soft"));
}
