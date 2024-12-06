@echo off
setlocal enabledelayedexpansion
set ECHO_MODE=on
set EXIT_ON_ERROR=1

set OUT=droppa
set RUSTFLAGS=-lraylib -L./lib/win

call :run_command cargo build --release
if exist %OUT% del /q %OUT%
copy /y .\target\release\%OUT% .
exit /b 0
:run_command
if "%ECHO_MODE%"=="on" echo running: %*
if "%EXIT_ON_ERROR%"=="1" (%* || exit /b 1) else (%*)
exit /b
