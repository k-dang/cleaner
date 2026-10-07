use std::{env, fs, path::Path};

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        panic!(
            "The desktop app requires Windows. Build or test the portable core with cargo test -p cleaner-core."
        );
    }
    // Release builds require elevation. Debug builds run as the invoking user so
    // `cargo run` works from a normal terminal.
    let level = match env::var("PROFILE").as_deref() {
        Ok("release") => "requireAdministrator",
        _ => "asInvoker",
    };
    let manifest = fs::read_to_string("resources/app.manifest")
        .unwrap()
        .replace("{{EXECUTION_LEVEL}}", level);

    let out = env::var("OUT_DIR").unwrap();
    let manifest_path = Path::new(&out).join("app.manifest");
    fs::write(&manifest_path, manifest).unwrap();
    let rc_path = Path::new(&out).join("app.rc");
    let icon_path = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("resources/app.ico");
    let rc = format!(
        "#define RT_MANIFEST 24\n1 RT_MANIFEST \"{}\"\n1 ICON \"{}\"\n",
        manifest_path.display().to_string().replace('\\', "\\\\"),
        icon_path.display().to_string().replace('\\', "\\\\")
    );
    fs::write(&rc_path, rc).unwrap();

    println!("cargo:rerun-if-changed=resources/app.manifest");
    println!("cargo:rerun-if-changed=resources/app.ico");
    // Embed resources in the app binary only; GPUI loads icon resource 1.
    embed_resource::compile_for(&rc_path, ["cc-cleaner"], embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
