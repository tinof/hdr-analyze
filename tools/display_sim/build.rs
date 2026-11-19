//! Links libplacebo 7.387.0, statically, and compiles `src/tm_shim.c` against it.
//!
//! The library comes from `scripts/build-libplacebo.sh`. pkg-config finds it: with
//! `LIBPLACEBO_PREFIX` set, `<prefix>/lib/pkgconfig` goes in front of `PKG_CONFIG_PATH`;
//! without, the script's default prefix (`~/.cache/hdr-analyze/libplacebo-v7.387.0`) goes behind
//! it. Any other version fails the build: the tool's numbers are this version's curves.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const VERSION: &str = "7.387.0";

fn pkg_config_path() -> OsString {
    let explicit = env::var_os("LIBPLACEBO_PREFIX").map(PathBuf::from);
    let prefix = explicit.clone().unwrap_or_else(|| {
        let home = env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
        home.join(".cache/hdr-analyze/libplacebo-v7.387.0")
    });
    let ours = prefix.join("lib").join("pkgconfig");
    let theirs: Vec<PathBuf> = env::var_os("PKG_CONFIG_PATH")
        .map(|value| env::split_paths(&value).collect())
        .unwrap_or_default();
    let dirs: Vec<PathBuf> = if explicit.is_some() {
        std::iter::once(ours).chain(theirs).collect()
    } else {
        theirs.into_iter().chain(std::iter::once(ours)).collect()
    };
    env::join_paths(dirs).expect("PKG_CONFIG_PATH entries contain no separator")
}

fn probe(emit: bool) -> pkg_config::Library {
    pkg_config::Config::new()
        .exactly_version(VERSION)
        .statik(true)
        .cargo_metadata(emit)
        .probe("libplacebo")
        .unwrap_or_else(|error| {
            panic!(
                "\n\ntools/display_sim needs libplacebo {VERSION}, built by \
                 scripts/build-libplacebo.sh (set LIBPLACEBO_PREFIX if it installed elsewhere).\n\
                 pkg-config said: {error}\n"
            )
        })
}

fn main() {
    println!("cargo:rerun-if-changed=src/tm_shim.c");
    println!("cargo:rerun-if-env-changed=LIBPLACEBO_PREFIX");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    env::set_var("PKG_CONFIG_PATH", pkg_config_path());

    // Include paths and defines first, without link flags: the shim must come before
    // libplacebo on the link line, or GNU ld drops the objects the shim needs.
    let library = probe(false);
    let mut build = cc::Build::new();
    build.file("src/tm_shim.c").includes(&library.include_paths);
    for (name, value) in &library.defines {
        build.define(name, value.as_deref());
    }
    build.flag_if_supported("-std=c11").compile("tm_shim");
    probe(true);

    if let Ok(dir) = pkg_config::get_variable("libplacebo", "pcfiledir") {
        println!(
            "cargo:rerun-if-changed={}",
            Path::new(&dir).join("libplacebo.pc").display()
        );
    }
    for dir in &library.link_paths {
        let archive = dir.join("libplacebo.a");
        if archive.is_file() {
            println!("cargo:rerun-if-changed={}", archive.display());
        }
    }
}
