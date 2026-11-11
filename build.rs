//! Apply the link arguments Prism needs to the game's final link (Hearken
//! publishes them; none since Prism 0.1.4), and on Windows link the vendored
//! SDL2 and stage SDL2.dll next to the build output.
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    pass_on_hearken_link_args();
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
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

/// hearken-speech works out Prism's link arguments and the `hearken` facade
/// passes them on; a link argument only reaches the final link, which is this
/// crate's, so the game applies them here. Cargo hands a build script this
/// list only from a direct dependency.
fn pass_on_hearken_link_args() {
    println!("cargo:rerun-if-env-changed=DEP_HEARKEN_LINK_ARGS");
    match env::var("DEP_HEARKEN_LINK_ARGS") {
        Ok(args) => {
            for arg in args.split(';').filter(|arg| !arg.is_empty()) {
                println!("cargo:rustc-link-arg={arg}");
            }
        }
        Err(_) => println!(
            "cargo:warning=Prism's link arguments are missing. Fix: keep hearken in this \
             package's [dependencies]."
        ),
    }
}
