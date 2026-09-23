//! Link the vendored SDL2 and stage SDL2.dll next to the build output, and
//! delay-load Prism's screen-reader client DLLs so the game starts on a
//! machine missing any of them (the same arrangement as Freight Fate).
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEP_PRISMER_DELAY_LOAD_DLLS");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        if let Ok(dlls) = env::var("DEP_PRISMER_DELAY_LOAD_DLLS") {
            for dll in dlls.split(';').filter(|d| !d.is_empty()) {
                println!("cargo:rustc-link-arg=/DELAYLOAD:{dll}");
            }
        }
    }
    let vendor =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("vendor/sdl2/windows-x86_64");
    println!("cargo:rerun-if-changed={}", vendor.display());
    println!("cargo:rustc-link-search=native={}", vendor.display());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    if let Some(profile) = out.ancestors().nth(3) {
        for entry in fs::read_dir(&vendor).into_iter().flatten().flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|e| e == "dll") {
                let _ = fs::copy(&p, profile.join(p.file_name().unwrap()));
            }
        }
    }
}
