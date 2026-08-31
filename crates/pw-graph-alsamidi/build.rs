#[cfg(target_os = "linux")]
fn main() {
    // Build scripts are compiled for the host, so `cfg!(target_os)` alone
    // would probe ALSA even when a Linux machine is cross-compiling the app
    // for Windows. Cargo exposes the package target explicitly here.
    if std::env::var_os("CARGO_CFG_TARGET_OS").as_deref() != Some(std::ffi::OsStr::new("linux")) {
        return;
    }
    if std::env::var_os("CARGO_FEATURE_ALSA").is_none() {
        return;
    }

    let alsa = pkg_config::Config::new()
        .probe("alsa")
        .expect("the alsa feature requires ALSA development files");
    let mut build = cc::Build::new();
    build
        .file("native/alsa_shim.c")
        .flag_if_supported("-std=c11");
    for include_path in alsa.include_paths {
        build.include(include_path);
    }
    build.compile("pw_graph_alsa_shim");
    println!("cargo:rerun-if-changed=native/alsa_shim.c");
}

#[cfg(not(target_os = "linux"))]
fn main() {}
