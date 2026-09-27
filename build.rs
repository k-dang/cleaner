use std::{env, fs, path::Path};

fn main() {
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
    let rc = format!(
        "#define RT_MANIFEST 24\n1 RT_MANIFEST \"{}\"\n",
        manifest_path.display().to_string().replace('\\', "\\\\")
    );
    fs::write(&rc_path, rc).unwrap();

    println!("cargo:rerun-if-changed=resources/app.manifest");
    // Embed the manifest in the app binary only; test binaries never need it.
    embed_resource::compile_for(&rc_path, ["cc-cleaner"], embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
