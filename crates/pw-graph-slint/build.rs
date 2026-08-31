use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=ui");
    let ui = PathBuf::from("ui/main.slint");
    // Slint's generated compiler can use a deep recursive stack on Windows.
    // Keep that implementation detail off the process' small build-script
    // stack so a normal Windows cargo build does not abort with
    // STATUS_STACK_OVERFLOW.
    // Debug info is what makes Slint's ElementHandle test API able to see the
    // rendered tree, so the panel layouts can be asserted from Rust tests
    // instead of only by eye. Debug builds only: release keeps the smaller
    // generated code.
    let debug_build = std::env::var("PROFILE").as_deref() != Ok("release");
    let result = std::thread::Builder::new()
        .name("slint-compiler".into())
        .stack_size(64 * 1024 * 1024)
        // `CompilerConfiguration` is not `Send`, so it is built on the worker.
        .spawn(move || {
            let configuration =
                slint_build::CompilerConfiguration::new().with_debug_info(debug_build);
            slint_build::compile_with_config(ui, configuration)
        })
        .expect("could not start the Slint compiler thread")
        .join()
        .expect("Slint compiler thread panicked");
    result.expect("failed to compile the application UI");
}
