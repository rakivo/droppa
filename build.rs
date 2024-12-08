fn main() {
    println!("cargo:rerun-if-changed=./src/index.js");
    println!("cargo:rerun-if-changed=./src/index.html");
}
