use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=ui");
    let ui = PathBuf::from("ui/main.slint");
    // Slint's generated compiler can use a deep recursive stack on Windows.
    // Keep that implementation detail off the process' small build-script
    // stack so a normal Windows cargo build does not abort with
    // STATUS_STACK_OVERFLOW.
    let result = std::thread::Builder::new()
        .name("slint-compiler".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || slint_build::compile(ui))
        .expect("could not start the Slint compiler thread")
        .join()
        .expect("Slint compiler thread panicked");
    result.expect("failed to compile the application UI");
}
