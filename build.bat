set OUT=droppa
set RUSTFLAGS=-lraylib -L.\lib\win
cargo build --release
if exist %OUT% del %OUT%
copy .\target\release\%OUT%.exe %OUT%.exe
