fn main() {
    if std::env::var_os("CARGO_FEATURE_PIPEWIRE").is_none() {
        return;
    }

    let pipewire = pkg_config::Config::new()
        .probe("libpipewire-0.3")
        .expect("the pipewire feature requires libpipewire-0.3 development files");

    let mut build = cc::Build::new();
    build
        .file("native/pipewire_shim.c")
        .flag_if_supported("-std=c11");
    for include_path in pipewire.include_paths {
        build.include(include_path);
    }
    build.compile("pw_graph_pipewire_shim");

    println!("cargo:rerun-if-changed=native/pipewire_shim.c");
}
