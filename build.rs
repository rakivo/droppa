fn main() {
    println!("cargo:rerun-if-changed=./front/droppa.ico");
    println!("cargo:rerun-if-changed=./front/index-desktop.js");
    println!("cargo:rerun-if-changed=./front/index-desktop.html");
    println!("cargo:rerun-if-changed=./front/index-mobile.js");
    println!("cargo:rerun-if-changed=./front/index-mobile.html");
}
