//! Embeds the Windows resources (the app icon) in kiritori.exe.

fn main() {
    println!("cargo:rerun-if-changed=kiritori.rc");
    println!("cargo:rerun-if-changed=assets/kiritori.ico");
    embed_resource::compile("kiritori.rc", embed_resource::NONE).manifest_required().unwrap();
}
