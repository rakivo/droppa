fn main() {
    println!("cargo:rerun-if-changed=./src/index-desktop.js");
    println!("cargo:rerun-if-changed=./src/index-desktop.html");
    println!("cargo:rerun-if-changed=./src/index-mobile.js");
    println!("cargo:rerun-if-changed=./src/index-mobile.html");
}
