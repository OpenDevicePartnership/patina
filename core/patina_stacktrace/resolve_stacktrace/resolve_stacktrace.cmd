@echo off
setlocal

REM Get the full path to the directory where this script is located
set SCRIPT_DIR=%~dp0

REM Remove trailing backslash (optional)
if "%SCRIPT_DIR:~-1%"=="\" set SCRIPT_DIR=%SCRIPT_DIR:~0,-1%

REM Run cargo with the manifest path in the same directory as the script
cargo run --quiet --manifest-path "%SCRIPT_DIR%\Cargo.toml"

endlocal
