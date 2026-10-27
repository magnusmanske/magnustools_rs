// The apps in `public/` are built into the binary (src/web/apps.rs).
fn main() {
    println!("cargo:rerun-if-changed=public");
}
