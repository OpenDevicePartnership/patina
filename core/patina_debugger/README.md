# Patina Debugger

The Patina Debugger provides a `no_std` GDB Remote client that is intended to be installed in a Patina boot-time core
environment, such as the DXE Core (the [`patina_dxe_core`](https://crates.io/crates/patina_dxe_core) crate). It
consumes a [patina::serial::SerialIO](https://docs.rs/patina/latest/patina/serial/trait.SerialIO.html) transport,
registers architecture-specific exception handlers through
[patina_internal_cpu::interrupts](https://docs.rs/patina_internal_cpu/latest/patina_internal_cpu/interrupts/index.html),
and exposes a policy-driven interface for bringing up interactive debugging.

> Note: The debugger is implemented fully in software and does not require any proprietary tools, licenses, or hardware
> unlocking.

## Why use the debugger?

A debugger is essential for diagnosing complex issues. While serial logging is useful, it may not clarify complicated
failures. The debugger lets you observe code execution, inspect variables and memory, and change system state during
execution to diagnose behavior.

Examples of errors easier to diagnose with a debugger:

- **Memory corruptions** – Use data breakpoints to catch these.
- **Page Faults** – Inspect the stack and variables at failure.
- **Unexpected Behavior** – Step through functions to analyze execution.

### Advantages over a hardware debugger

Hardware debuggers (JTAG) are powerful but need special hardware, configuration, and licenses.
The self-hosted debugger is lightweight and tightly integrated with Patina, offering features like:

- Breaking on module load
- Catching exceptions, panics, and asserts directly
- Customized [debugger commands](#monitor-commands))

## Capabilities

- Tracks loaded modules and surfaces module-aware breakpoints to support halting.
- Implements the GDB Remote Serial Protocol over a `SerialIO` transport.
- Hooks exception vectors through `InterruptManager` to support debugging on x86_64 and AArch64.
- Coordinates with Patina logging via `DebuggerLoggingPolicy` to suspend, disable, or allow logging while a target is
  paused.
- Includes WinDbg interoperability workarounds and knowledge of mapping internal Patina structures to make inspecting
  those structures easier in the debugger.

## Platform Integration

1. Instantiate a `PatinaDebugger` with the platform UART configuration (for example, `Uart16550::Io { base: 0x3F8 }`).
2. Apply any policy overrides such as `.with_force_enable`, `.with_log_policy`, or `.without_transport_init` when
   logging shares the transport.
3. Register the debugger using `patina_debugger::set_debugger(&DEBUGGER)` before the Patina DXE Core starts dispatching
   components.
4. Call `patina_debugger::initialize(&mut interrupt_manager)` during platform bring-up so the core installs exception
   handlers and optionally triggers the initial breakpoint.
5. Use the static facade (`poll_debugger`, `notify_module_load`, `breakpoint`, `enabled`) inside Patina components or
   platform code as needed.

Integration examples are documented in
[`docs/src/integrate/dxe_core.md`](https://opendevicepartnership.github.io/patina/integrate/dxe_core.html#62-debugger-configuration).

In addition, active examples are available in the
[patina-dxe-core-qemu](https://github.com/OpenDevicePartnership/patina-dxe-core-qemu) repository:

- [QEMU Q35](https://github.com/OpenDevicePartnership/patina-dxe-core-qemu/blob/main/bin/q35_dxe_core.rs)
  - Intel platform with serial debug over UART 16550 with I/O port access

- [QEMU SBSA](https://github.com/OpenDevicePartnership/patina-dxe-core-qemu/blob/main/bin/sbsa_dxe_core.rs)
  - AArch64 platform with serial debug over UART PL011 with MMIO access

## Feature flags

- `windbg_workarounds` (default): adjusts protocol behaviors for Windbg compatibility, including suppressing repeated
  reads that stall early-boot sessions.
- `alloc`: replaces static communication buffers with dynamically allocated storage and enables monitor command
  registration; this requires a functional allocator but unlocks richer diagnostics.

---

## Configuring the Debugger

### Step 1: Set up the struct

Instantiate the static `PatinaDebugger` struct to match your device. The main configuration is
setting the debugger transport, usually a serial port. If only one serial port is available, it may
be shared with logging. In this case use `without_transport_init()` to avoid port contention.

Example setup:

```rust
#[cfg(feature = "enable_debugger")]
const _ENABLE_DEBUGGER: bool = true;
#[cfg(not(feature = "enable_debugger"))]
const _ENABLE_DEBUGGER: bool = false;

#[cfg(feature = "build_debugger")]
static DEBUGGER: patina_debugger::PatinaDebugger<UartPl011> =
    patina_debugger::PatinaDebugger::new(UartPl011::new(0x6000_0000))
        .without_transport_init()
        .with_force_enabled(_ENABLE_DEBUGGER);
```

Debugging configuration is critical to proper functionality. Read the
[Patina Debugger documentation](https://docs.rs/patina_debugger/latest/patina_debugger/) for full configuration options.

> Note: It is recommended to use a compile time feature flag to build the debugger, including instantiating the
> static struct, as this saves significant file space when the debugger is not enabled. It has been shown to save
> 60k - 200k of binary size depending on the platform. Debug builds should default to having this feature flag enabled;
> this helps to encourage debugger use and ensure that the platform FV is large enough to accommodate the debugger's
> added size. A separate feature, as shown in the examples, may be used to enable the debugger.

### Step 2: Install the debugger

In the platform initialization routine, call `set_debugger` to install the debugger
**prior to calling the Patina core**. This will install the global debugger so that
it is available in the core.

```rust
#[cfg(feature = "build_debugger")]
patina_debugger::set_debugger(&DEBUGGER);
```

Just because the debugger is installed, does not mean that the debugger is enabled
or active. Installing is a no-op without enablement.

### Step 3: Enable the debugger

Enable the debugger at compile time by enabling the debugger feature, e.g. in the examples above this would be
`cargo make build --features enable_debugger`. This causes Patina to break early and wait for the debugger. If
successful, on boot you should see the following (if error logging is enabled) followed by a hang.

```text
ERROR - ************************************
ERROR - ***  Initial debug breakpoint!   ***
ERROR - ************************************
```

This means the debugger is waiting for a connection. If you do not see this hang,
then confirm that the debugger is enabled and installed prior to calling the core.

You can also enable the debugger at runtime using the `enable` routine, but use caution.
Dynamic enablement should be carefully thought through to ensure proper platform security.
See the [Security Considerations section](#security-considerations) for more details.

### Step 4: Verify the transport

After the initial breakpoint, monitor the debug port for the following packet.
Note that the debug port and the logging port may not be the same depending on
the platform configuration.

```text
$T05thread:01;#07
```

This packet signals a break to the debug software. If you do not see it, check your transport
configuration and hardware port settings. Some console software will not print
synchronously or will filter certain traffic, if you do not see the packet then try using
putty or a similar simple monitor to check for the traffic.

### Step 5: Connect the debugger

Once the breakpoint and transport are confirmed, connect your debugging software. Any GDB remote
protocol debugger should work. WinDbg is recommended and best supported by the Patina team.
See the [WinDbg Debugging page](https://opendevicepartnership.github.io/patina/dev/debugging/windbg_debugging.html)
for details.

GDB also works, but symbols may not resolve since Patina uses PE images with PDB symbols.

### Step 6: Set up the panic handler

To break into the debugger on a panic, add a manual breakpoint in the panic handler. Only do this
when the debugger is enabled:

```rust
if patina_debugger::enabled() {
    patina_debugger::breakpoint();
}
```

As an aside, `patina_debugger::breakpoint()` can be useful to placing in other locations
of interest while debugging to ensure you catch a specific function or scenario.

### Security Considerations

When enabling the debugger through any runtime enablement mechanism, it is critical
that the platform consider the security impacts. The platform should be certain
that the configuration or policy that is used to enable the debugger comes from
an authenticated source and that the enablement of the debugger is properly captured
in the TPM measurements (PCR7 is recommended) through the appropriate `EV_EFI_ACTION`
measurement **BEFORE** enabling the debugger. Allowing the debugger to be dynamically
enabled in production in an unauthenticated or unmeasured way would be a significant
security bypass.

## Debugger Functionality

The debugger supports most core features via the GDB remote protocol. Extra features use monitor
commands.

| Feature                       | State        | Notes                                  |
|-------------------------------|--------------|----------------------------------------|
| Memory Read/Write             | Supported    |                                        |
| General Purpose Register R/W  | Supported    |                                        |
| Instruction Stepping          | Supported    |                                        |
| Interrupt break               | Supported    |                                        |
| System Register Access        | Partial      | Read via monitor commands              |
| SW Breakpoints                | Supported    |                                        |
| Watchpoints / Data Breakpoints| Supported    |                                        |
| HW Breakpoints                | Unsupported  | Not needed with SW breakpoints         |
| Break on module load          | Supported    | Via monitor command                    |
| Reboot                        | Supported    | Via monitor command                    |
| Multicore Support             | Unsupported  | BSP only; multicore may be added later |

### Monitor commands

Monitor commands are interpreted by the Patina debugger. They allow dynamic actions from the
debugger. Use `!monitor <command>` in WinDbg or `monitor <command>` in GDB. For a full
enumeration use the `help` command, but here are some core commands:

| Command     | Description                                           |
|-------------|-------------------------------------------------------|
| `help`      | Lists monitor commands                                |
| `?`         | Shows debugger info and current break                 |
| `mod`       | Module functions: list modules, break on load         |
| `arch`      | Architecture-specific functions, e.g., dump registers |

Patina components and the core can register their own custom monitor commands using the
`patina_debugger::add_monitor_command` command. This can be used to parse complicated
structures, invoke hardware functionality, or change behavior of the component.
