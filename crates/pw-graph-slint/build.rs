use slint_build::CompilerConfiguration;
use std::collections::HashMap;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=../../vendor/slint-node-editor/node-editor.slint");
    println!(
        "cargo:rerun-if-changed=../../vendor/slint-node-editor/node-editor-building-blocks.slint"
    );

    // Keep the pinned node-editor submodule pristine. qpwgraph cards contain
    // interactive controls, so generate an app-local compatibility copy where
    // BaseNode's drag surface can be restricted to the header.
    let source_root = PathBuf::from("../../vendor/slint-node-editor");
    let generated_root = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set"))
        .join("slint-node-editor");
    std::fs::create_dir_all(&generated_root).expect("create generated Slint library directory");
    let editor = std::fs::read_to_string(source_root.join("node-editor.slint"))
        .expect("read node-editor.slint");
    std::fs::write(generated_root.join("node-editor.slint"), editor)
        .expect("write generated node-editor.slint");
    let blocks = std::fs::read_to_string(source_root.join("node-editor-building-blocks.slint"))
        .expect("read node-editor-building-blocks.slint");
    let blocks = blocks
        .replace(
            "    in property <length> node-height: 100px;\n",
            "    in property <length> node-height: 100px;\n    // Optional dedicated drag handle for nodes with interactive bodies.\n    in property <length> drag-handle-height: node-height;\n",
        )
        .replace(
            "        height: root.node-height;\n\n        property <bool> shift-held:",
            "        height: root.drag-handle-height;\n\n        property <bool> shift-held:",
        );
    assert!(
        blocks.contains("drag-handle-height: node-height"),
        "node-editor BaseNode layout changed upstream"
    );
    std::fs::write(
        generated_root.join("node-editor-building-blocks.slint"),
        blocks,
    )
    .expect("write generated node-editor-building-blocks.slint");

    let mut library_paths = HashMap::new();
    library_paths.insert(
        "slint-node-editor".to_owned(),
        generated_root,
    );

    let configuration = CompilerConfiguration::default().with_library_paths(library_paths);
    slint_build::compile_with_config("ui/main.slint", configuration)
        .expect("failed to compile the Slint preview UI");
}
