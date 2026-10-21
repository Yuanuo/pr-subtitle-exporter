@echo off
REM 编译脚本（x86_64-pc-windows-gnu 工具链）
REM Rust 的 gnu 目标链接时需要 as.exe / dlltool.exe 等 mingw 工具。
REM 优先使用本机已解压好的 w64devkit；没有则退回 rustup 自带的精简目录。
set W64DEVKIT=C:\tools\w64dk2\w64devkit\w64devkit\bin
if exist "%W64DEVKIT%\gcc.exe" (
    set PATH=%W64DEVKIT%;%PATH%
) else (
    set PATH=%USERPROFILE%\.rustup\toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained;%PATH%
)
cargo build --release %*
