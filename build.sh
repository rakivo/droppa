set -xe

OUT=droppa
RUSTFLAGS="-lraylib -L./lib"
export RUSTFLAGS
cargo build --release
[ -f "$OUT" ] && rm "$OUT"
cp ./target/release/$OUT .
