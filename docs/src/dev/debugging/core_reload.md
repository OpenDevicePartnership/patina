# Core Reload

Debugger core reload is an experimental feature that allows a new Patina core to be loaded and executed from the initial
breakpoint of the existing core. This is intended to be used for rapid development where flashing the device is time
consuming or logistically difficult.

## Enabling Core Reload

To enable core reload, the debugger must first be set up and enabled. Additionally, the `debugger_reload` feature must
be enabled for the `patina_dxe_core` crate. When both of these prerequisites are true, core reload is functional
at the initial breakpoint.

## Reloading the Core

Reloading the core should be done using a debugger extension. Currently this is only supported in WinDbg using the
[UefiExt extension](https://github.com/microsoft/uefi_debug_tools/tree/main/UefiDbgExt). When the extension is installed,
the feature is enabled, and the debugger is broken in on the initial breakpoint, then you can use the `!uefiext.loadcore`
command to load the new core.

```text
kd> !uefiext.loadcore D:\patina-dxe-core-qemu\target\x86_64-unknown-uefi\debug\qemu_q35_dxe_core.efi
Loading image of size 1982464 bytes from D:\patina-dxe-core-qemu\target\x86_64-unknown-uefi\debug\qemu_q35_dxe_core.efi
Compressed image size: 738544 bytes
Transferred 738544 / 738544 bytes (100%) in 20 seconds (288 kbps)...
Image loaded successfully.
... symbols reload prints ...
Executing new core...
```

If the debugger is enabled in the new core, you will see a new break-in on the initial breakpoint. Otherwise it will
continue execution. The `/nogo` option provides the ability to load the new core but not begin execution. This can be
useful for early core debugging, though caution should be used as debugger exception handler transitions may be unstable.

## UART Considerations

The core reload feature requires a large amount of rapid data transfer between the host and the device. Depending on the
UART implementation and support for flow control, this has the potential to overflow hardware FIFO.
[`ComToTcpServer.py`](https://github.com/microsoft/uefi_debug_tools/blob/main/Scripts/ComToTcpServer.py)
has some artificial rate limiting to attempt to prevent this, but it may not work for all devices. If the FIFO fills and
bits are dropped, this can cause corrupted packets or worse hard-hangs as the GDB stub awaits more data that was dropped.
If these occur, consider changing the settings in `ComToTcpServer.py` (if used) and/or implementing flow control.

## How It Works

The reload occurs in several steps coordinated via the debugger monitor command between the debugger
(specifically UefiExt) and the DXE core. The steps are as follows:

1. The debug extension will load the specified file and compress it for faster transfer.
2. The extension uses core reload monitor commands to allocate a buffer in the DXE core of the
   required size to hold the compressed image. The address of this buffer is returned to the debugger.
3. The extension writes the image into the allocated buffer via memory write commands.
4. The extension issues the load command to the DXE core, passing the address and size of the new image.
5. The DXE core decompresses the image using the uefi decompress algorithm.
6. The DXE core parses the image, performs relocations, rebuilds the HOB list, and then
   provides the context (IP, SP, arg0) needed to start execution of the new image back to the
   debugger.
7. The debugger will change the exception context to point to the new image entry point and HOB list and resume execution.
