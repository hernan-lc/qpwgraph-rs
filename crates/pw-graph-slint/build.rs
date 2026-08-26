use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=ui");
    let ui = PathBuf::from("ui/main.slint");
    slint_build::compile(ui).expect("failed to compile the application UI");
}
