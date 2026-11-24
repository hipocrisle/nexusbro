// Source-only verification never authorizes an executable component bundle.
fn main() {
 let out=std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
 std::fs::write(out.join("youtube-component-pins.rs"),"const SEALED_YOUTUBE_MANIFEST: &str = \"\";\n").unwrap();
}
