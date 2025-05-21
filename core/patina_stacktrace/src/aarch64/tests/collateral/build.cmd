@echo off

:: "C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Auxiliary\Build\vcvarsamd64_arm64.bat"
:: "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvarsarm64.bat"
:: editbin.exe /rebase:base=0x180000000 "C:\r\qemu_rust_bins\target\x86_64-unknown-uefi\debug\deps\qemu_q35_dxe_core-9eb1e2f68d36020c.efi"

del *.dll 2> nul
del *.exe 2> nul
del *.exp 2> nul
del *.lib 2> nul
del *.obj 2> nul
del *.pdb 2> nul

armasm64 -o getsp.obj getsp.asm
cl /LD /MT aarch64.c getsp.obj /Zi /link /DEBUG /INCREMENTAL:NO
cl test.c /Zi /link /DEBUG /INCREMENTAL:NO
link -dump -unwindinfo aarch64.dll > unwindinfo.txt

del *.obj 2> nul
del *.exp 2> nul
del *.lib 2> nul
