fn main() {
    // Embed the manifest in the app binary only; test binaries must run unelevated.
    embed_resource::compile_for("resources/app.rc", ["cc-cleaner"], embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
