set -x

RUSTFLAGS="-lraylib -L./lib" cargo build --release
rm ./droppa
cp ./target/release/droppa .
