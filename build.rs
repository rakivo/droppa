fn main() {
    println!("cargo:rerun-if-changed=./index.js");
    println!("cargo:rerun-if-changed=./index.html");

    if cfg!(windows) {
        println!("cargo:rustc-link-lib=static=raylib");
        println!("cargo:rustc-link-search=native=.\\lib\\windows");
        println!("cargo:rerun-if-changed=.\\lib\\windows\raylib.lib");
    } else {
        println!("cargo:rustc-link-lib=dylib=raylib");
        println!("cargo:rustc-link-search=native=./lib/linux");
        println!("cargo:rerun-if-changed=./lib/linux/libraylib.so");
    }
}
