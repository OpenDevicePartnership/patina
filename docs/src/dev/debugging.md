# Debugging

This section explains how to set up and use the Patina Debugger. For design details, see the
[debugging theory of operation page](../dxe_core/debugging.md).

## Introduction to the Debugger

Patina includes a self-hosted debugger that uses the GDB remote protocol over a serial port.
It is implemented fully in software and does not require proprietary tools, licenses, or hardware
unlocking.

### Why use the debugger?

A debugger is essential for diagnosing complex issues. While serial logging is useful, it may not
clarify complicated failures. The debugger lets you observe code execution, inspect variables and
memory, and change system state during execution to diagnose behavior.

Examples of errors easier to diagnose with a debugger:

- **Memory corruptions** – Use data breakpoints to catch these.
- **Page Faults** – Inspect the stack and variables at failure.
- **Unexpected Behavior** – Step through functions to analyze execution.

### Advantages over a hardware debugger

Hardware debuggers (JTAG) are powerful but need special hardware, configuration, and licenses.
The self-hosted debugger is lightweight and tightly integrated with Patina, offering features like:

- Breaking on module load
- Module and symbol enumeration
- Catching exceptions, panics, and asserts directly
- Customized debugger commands ([monitor commands](#monitor-commands))

## Configuring the Debugger

### Step 1: Set up the struct

Instantiate the static `PatinaDebugger` struct to match your device. The main configuration is
setting the debugger transport, usually a serial port. If only one serial port is available, it may
be shared with logging. In this case use `without_transport_init()` to avoid port contention. See
[Patina's QEMU DXE bins](https://github.com/OpenDevicePartnership/patina-dxe-core-qemu/tree/main/bin)
for examples.

Example setup:

```rust
static DEBUGGER: patina_debugger::PatinaDebugger<UartPl011> =
    patina_debugger::PatinaDebugger::new(UartPl011::new(0x6000_0000))
        .without_transport_init()
        .with_force_enabled(false);
```

Debugging configuration is critical to proper functionality. Read the [Patina Debugger documentation](https://github.com/OpenDevicePartnership/patina/blob/main/core/patina_debugger/src/debugger.rs)
for full configuration options.

### Step 2: Install the debugger

In the platform initialization routine, call `set_debugger` to install the debugger
**prior to calling the Patina core**. This will install the global debugger so that
it is available in the core.

```rust
patina_debugger::set_debugger(&DEBUGGER);
```

Just because the debugger is installed, does not mean that the debugger is enabled
or active. Installing is a no-op without enablement.

### Step 3: Enable the debugger

Enable the debugger at compile time with `.with_force_enabled(true)`. This causes Patina to
break early and wait for the debugger. If successful, on boot you should see the following
(if error logging is enabled) followed by a hang.

```text
ERROR - ************************************
ERROR - ***  Initial debug breakpoint!   ***
ERROR - ************************************
```

This means the debugger is waiting for a connection. If you do not see this hang,
then confirm that the debugger is enabled and installed prior to calling the core.

You can also enable the debugger at runtime using the `Configure` routine, but use caution.
Runtime enablement can skip the initial breakpoint and may cause security issues. For development,
prefer force enablement.

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
putty or similar simple monitor to check for the traffic.

### Step 5: Connect the debugger

Once the breakpoint and transport are confirmed, connect your debugging software. Any GDB remote
protocol debugger should work. WinDbg is recommended and best supported by the Patina team.
See the [WinDbg Debugging page](debugging/windbg_debugging.md) for details.

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
