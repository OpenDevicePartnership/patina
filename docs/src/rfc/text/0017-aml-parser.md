# RFC: `ACPI SDT AML Handler`

TODO

## Change Log

- 2025-10-1: Initial RFC created.

## Motivation

This RFC is an extension of the [ACPI service](0005-acpi.md).
Similar to the ACPI service, this Rust-based AML service will provide a safer and
more ergnonic interface for parsing and interpreting AML bytecode.

## Technology Background

AML bytecode is encoded mainly in the body of the DSDT and SSDT. 
More details about the layouts of these tables can be found in the [ACPI Specification, Vol. 5](https://uefi.org/specs/ACPI/6.5/05_ACPI_Software_Programming_Model.html?highlight=ssdt).
The specifics of AML grammar can be found in the [ACPI Specification, Vol. 20](https://uefi.org/specs/ACPI/6.5/20_AML_Specification.html).

## Goals

Provide a comprehensive Rust implementation for AML parsing, interpretation, and execution. 

## Requirements
1. Redesign the existing C firmware AML implementation into a a safe, easy-to-use Rust service.
2. Implement firmware-side AML parsing: conduct static boot-time validation and expose namespace structures. 
3. Implement application-side AML interpretation and execution: 
dynamically interpret AML opcodes, resolve runtime options, and execute methods on demand.    
1. Use the Rust service (*1.*) to implement the C ACPI SDT protocol.

## Prior Art
The [ACPI SDT protocol](https://uefi.org/specs/PI/1.8/V5_ACPI_System_Desc_Table_Protocol.html) is a spec-defined UEFI PI protocol for retrieving and parsing ACPI tables.
There are many existing implementations, such as [edk2's AcpiTableDxe](https://github.com/tianocore/edk2/blob/edb5331f787519d1abbcf05563c7997453be2ef5/MdeModulePkg/Universal/Acpi/AcpiTableDxe/AmlChild.c#L4).

An (incomplete) implementation for application-side interpretation of AML bytecode exists in the [Rust `acpi` crate](https://github.com/rust-osdev/acpi).

## Alternatives + Open Questions

The [Rust `acpi` crate](https://github.com/rust-osdev/acpi) already provides some functionality for interpreting AML bytecode. However, it is incomplete and provides limited public interfaces; it also does not deal with firmware-side protocols or parsing. 

This leaves two main paths for the Patina AML implmentation:
1. Design and implement a new Rust AML service from the ground up, without explicitly utilizing the existing `acpi` crate. (`acpi` has MIT license, so it may be possible to borrow some snippets/implementations with proper attribution.)
   - Pros: Interfaces and implementations can be tailored to Patina needs.
   - Cons: Repeated work.
2. Design and implement the Rust AML service while using the `acpi` crate as a dependency and parsing through its public interfaces. (This may involve contributing to the `acpi` crate to improve its public interfaces.)
    - Pros: Less repeated code, especially for parsing. 
    - Cons: `acpi` has limited public interfaces, which may constrain the development of the Rust ACPI service. It primarily focuses on looking up and executing AML in the application space, with less support for actually walking through and modifying the firmware-side AML object tree.

This RFC favors the former (1), as the `acpi` crate seems generally too constraining to be used as a direct dependency for the Rust AML service.

## Rust Code Design

The `AmlParser` service generally derives from the ACPI SDT protocol, and allows for traversal of the AML object tree.

```rust
pub(crate) trait AmlParser {
  // Opens a table for traversal, loading its namespace structures. The table should be a DSDT or SSDT. 
  // The resulting handle is an opaque object on which further AML operations can be performed.
  fn open_sdt(&self, table_key: TableKey) -> Result<AmlHandle, AmlError>;

  // Closes a table for traversal, and removes its namespace from access.
  // The handle will no longer be valid after it is closed.
  fn close_sdt(&self, handle: AmlHandle) -> Result<(), AmlError>;

  // Iterates over the options (operands) of an opened AML handle.
  fn iter_options(&self, handle: AmlHandle) -> Result<Vec<AmlData>, AmlError>;

  // Sets the option (operand) at a particular index to the given value.
  fn set_option(&self, handle: AmlHandle, AmlData) -> Result<(), AmlError>;

  // Returns the first child of an AML node. 
  fn get_child(&self, handle: AmlHandle) -> Result<Option<AmlHandle>, AmlError>;

  // Returns the next sibling of an AML node.
  fn get_sibling(&self, handle: AmlHandle) -> Result<Option<AmlHandle>, AmlError>;

  // The above two functions are intended to provide a complete traversal implementation.
  // For example, to get all the children of a node, find the first child through `get_child`, then use `get_sibling` on each subsequent child. In both cases, `None` indicates no child/sibling. 
}
```

While the AML handles are opaque to the consumer, they contain private fields that are used internally in the `AmlParser` service.

```rust
pub(crate) struct AmlSdtHandleInternal {
    table_key: TableKey,
    offset: usize,
    size: usize,
    modified: bool,
}

impl AmlSdtHandleInternal {
    fn new(table_key: TableKey, offset: usize, size: usize) -> Self {
        Self { table_key, offset, size, modified: false }
    }
}

pub type AmlHandle = AmlSdtHandleInternal;

// Sentinel node for traversal. 
const ROOT_NODE: AmlSdtHandleInternal =
    AmlSdtHandleInternal { table_key: TableKey(0), offset: 0, size: 0, modified: false };
```

By storing its own `size` (derived from the node's `pkg_length`) and `offset` (within the table's AML bytecode stream), each `AmlSdtHandleInternal` (representing an AML node) will allow for lazy parsing when a traversal function is called on it. 

`modified` ensures the corresponding table (which can be retrieved through `table_key`) has an updated checksum if the contents are modified between `open_sdt` and `close_sdt`. This primarily occurs through `set_option`. 

`AmlData` represents the various operands that can be returned from `GetOption`.
```rust
pub enum AmlData {
    None,
    Opcode(u8),
    NameString(AmlNameStringPath),
    OpFn(AmlOp),
    UnsignedInt(u64),
    StringLiteral(String),
    Child(AmlHandle),
}
```

## Guide-Level Explanation

The general flow for using the `AmlParser` service will be:
1. Set up and install necessary tables with the `AcpiProvider` service.
2. Open a DSDT or SSDT with `open_sdt`.
3. Traverse as necessary through `get_child`, `get_sibling`, and `get_option`. 
4. Make necessary modifications through `set_option`.
5. Mark the node as closed for modifications with `close_sdt`.