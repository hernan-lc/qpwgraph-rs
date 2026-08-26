#[cfg(target_os = "linux")]
fn main() {
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
