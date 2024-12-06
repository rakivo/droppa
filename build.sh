set -xe

OUT=droppa

RUSTFLAGS="-lraylib -L./lib" cargo build --release
[ -f "$OUT" ] && rm "$OUT"
cp ./target/release/$OUT .
