# Hardware Access

Patina components access hardware through several mechanisms, each with Rust-specific safety considerations that
differ from traditional C firmware. This section covers the supported access methods, the crates Patina uses for
each, and the pitfalls to avoid.

- [Memory-Mapped I/O (MMIO)](hardware_access/mmio.md)

## Architectural Interfaces

Various parts of Patina need to access architectural interfaces, e.g. I/O Ports on x86 or reading system registers
on AArch64. Patina has consolidated generally applicable functions in the Patina SDK's arch module. Developers
should prefer using interfaces from there and adding new ones as needed. However, if the architectural interface is
not widely applicable, it should be contained within the module that uses it.

The goal is to limit the use of inline assembly code, particularly around interfaces that need very specific patterns
to operate correctly. Instead, well used and tested interfaces provided by the SDK should be used so that safety
invariants can be maintained.
