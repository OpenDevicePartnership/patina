# RFC: `ACPI SDT AML Handler`

This RFC proposes a Rust-based AML (ACPI Machine Language) service that extends the existing ACPI service by
providing a safe, ergonomic interface for parsing and modifying AML bytecode in firmware during the DXE phase.
The implementation mirrors the ACPI SDT protocol’s functionality for AML traversal and patching
and is designed mainly for firmware use rather than OS-level interpretation.
It defines a structured system of AML handles for navigating AML streams and a trait-based `AmlParser` service for operations
such as opening tables, iterating operands, modifying values, and traversing child or sibling nodes.
The goal is to replace legacy C-based AML handling with a type-safe Rust service that supports ACPI 2.0+.
Future extensions may include extending this infrastructure for application-side AML interpretation 
within a `patina-acpi` crate.

## Change Log

- 2025-10-1: Initial RFC created.

## Motivation

This RFC is an extension of the [ACPI service](0005-acpi.md).
Similar to the ACPI service, this Rust-based AML service will provide a safer and
more ergonomic interface for parsing and modifying AML bytecode.

## Technology Background

AML bytecode is encoded mainly in the body of the DSDT and SSDT.
More details about the layouts of these tables can be found in the [ACPI Specification, Vol. 5](https://uefi.org/specs/ACPI/6.5/05_ACPI_Software_Programming_Model.html?highlight=ssdt).
The specifics of AML grammar can be found in the [ACPI Specification, Vol. 20](https://uefi.org/specs/ACPI/6.5/20_AML_Specification.html).

Like the `AcpiProvider` service, this AML parser supports only ACPI 2.0+.

This RFC discusses only the UEFI spec-defined AML handling behavior.
**It does not attempt to implement functionality for interpreting or executing the AML namespace in the OS domain,
which becomes relevant only after UEFI boot has completed.**

### Protocol

The Rust AML implementation derives its format from the [ACPI SDT protocol](https://uefi.org/specs/PI/1.8/V5_ACPI_System_Desc_Table_Protocol.html),
which includes basic functionality for parsing and patching AML bytecode after ACPI tables are installed.  

## Goals

Provide a comprehensive Rust implementation for AML parsing and patching on the firmware DXE side.
Secondarily, implement the rest of the ACPI SDT protocol relating to AML functionality.  

## Requirements

1. Redesign the existing C firmware AML implementation into a safe, easy-to-use Rust service.
2. Implement firmware-side AML parsing: traversal and patching of AML bytecode as opcodes and operands.
3. Use the Rust service (*1.*) to implement the C ACPI SDT protocol.

## Prior Art

The [ACPI SDT protocol](https://uefi.org/specs/PI/1.8/V5_ACPI_System_Desc_Table_Protocol.html)
is a spec-defined UEFI PI protocol for retrieving and parsing ACPI tables.
There are many existing implementations, such as [edk2's AcpiTableDxe](https://github.com/tianocore/edk2/blob/edb5331f787519d1abbcf05563c7997453be2ef5/MdeModulePkg/Universal/Acpi/AcpiTableDxe/AmlChild.c#L4).

An (incomplete) implementation for application-side interpretation of AML bytecode exists in the [Rust `acpi` crate](https://github.com/rust-osdev/acpi).
While the intent of the `acpi` crate is to interpret and execute AML after boot,
there is significant overlap in the low-level parsing code between the firmware and application side of AML functionality.

## Alternatives + Open Questions

The [Rust `acpi` crate](https://github.com/rust-osdev/acpi)
already provides some functionality for interpreting AML bytecode.
However, it is incomplete and provides limited public interfaces;
it also does not deal with firmware-side protocols or parsing.

This leaves two main paths for the Patina AML implementation:

1. Design and implement a new Rust AML service from the ground up,
without explicitly utilizing the existing `acpi` crate.
(`acpi` has MIT license, so it may be possible to borrow some snippets/implementations with proper attribution.)
   - Pros: Interfaces and implementations can be tailored to Patina needs.
   - Cons: Repeated work.
2. Design and implement the Rust AML service
while using the `acpi` crate as a dependency and parsing through its public interfaces.
(This may involve contributing to the `acpi` crate to improve its public interfaces.)
    - Pros: Less repeated code, especially for parsing.
    - Cons: `acpi` has limited public interfaces, which may constrain the development of the Rust ACPI service.
It primarily focuses on looking up and executing AML in the application space,
with less support for actually walking through and modifying the firmware-side AML object tree.

There is ongoing conversation with the owner of the `acpi` crate about
borrowing certain implementations and modifying the public interfaces
to be more friendly to the Patina ACPI implementation.
This conversation can be tracked through a [Github issue in the `acpi` crate](https://github.com/rust-osdev/acpi/issues/260).

## Rust Code

### ACPI/AML Utility Crate

While the `AmlParser` service has functionality specific only to the UEFI ACPI SDT protocol,
it will be useful for extensibility to extract basic AML reading and patching capability into a seperate utility crate,
`patina-acpi`. `AmlParser` will depend on this crate.

For example, basic parsing functionality like reading the package length, parsing name segments, etc.,
will be included in this utility crate.

The proposed `patina-acpi` crate shares functionality with the existing `acpi` crate; as such,
it remains to be seen if collaboration is possible to expose this shared parsing functionality,
or if `patina-acpi` will borrow/fork from `acpi`.

### AML Handles

By spec definition, an AML handle is an opaque handle returned from opening a DSDT or SSDT,
on which AML traversal and patching operations can be performed.

Internally, each `AmlHandle` object, aliased as `AmlSdtHandleInternal`,
represents a cursor within an AML stream. Each handle can be conceptualized as a "node"
in the AML object tree of parent-child-relationships.

```rust
pub(crate) struct AmlSdtHandleInternal {
    table_key: TableKey,
    offset: usize,
    size: usize,
    modified: bool,
    byte_encoding: AmlByteEncoding,
    parent_end: Option<usize>,
}

impl AmlSdtHandleInternal {
    fn new(table_key: TableKey, offset: usize, size: usize) -> Self {
        Self {
            table_key,
            offset,
            size,
            modified: false,
            byte_encoding: AmlByteEncoding::default(),
            parent_end: None,
        }
    }
}

pub type AmlHandle = AmlSdtHandleInternal;
```

The `size` of an `AmlSdtHandleInternal` refers to its full size, including any children (`TermList`).

The `offset` refers to its offset with the AML stream of the table.
Offset 0 is the start of the AML stream, and the highest offset is at the end of `table_length`.

`modified` ensures the corresponding table (which can be retrieved through `table_key`) has an updated checksum
if the contents are modified by `set_option`.

Each handle stores the `parent_end` of its parent node, which is the parent's `size` + `offset` (useful for retrieving siblings).

Each handle also stores its own byte encoding, specifying its own layout:

```rust
#[derive(PartialEq, Eq, Hash)]
struct AmlByteEncoding {
    opcode: AmlOpcode,
    operands: Vec<AmlOperand>,
    attributes: AmlOpAttributes,
}

bitflags! {
    pub struct AmlOpAttributes: u32 {
        /// If opcode has a pkg_length field.
        const HAS_PKG_LENGTH  = 0x0000_0001;

        /// If opcode has children.
        const HAS_CHILD_OBJ   = 0x0000_0002;
    }
}

/// Represents the possible opcodes. 
pub enum AmlOpcode {
    BaseOp(BaseOpcode),
    ExtOp(ExtOpcode),
}

pub enum BaseOpcode {
    ZeroOp,
    AliasOp,
    ...
}

pub enum ExtOpcode {
    MutexOp,
    EventOp,
    ...
}

pub enum AmlOperand {
    Opcode(AmlOpcode),
    Name(AmlNameString), // Represents a NameString (AML path). Not to be confused with a string literal 
    ...
}
```

### AML Trait Interface

The `AmlParser` service generally derives from the ACPI SDT protocol, and allows for traversal of the AML object tree.

```rust
pub(crate) trait AmlParser {  
  // Opens a table's AML stream for parsing. The table should be a DSDT or SSDT. 
  // The resulting handle is an opaque object on which further AML operations can be performed.
  // It points to the first (root) node in the AML stream.
  unsafe fn open_table(&self, table_key: TableKey) -> Result<AmlHandle, AmlError>;

  // Closes a handle for modification and traversal.
  // The handle will no longer be valid after it is closed.
  // Update the corresponding table's checksum if the handle is `modified`. 
  fn close_handle(&self, handle: AmlHandle) -> Result<(), AmlError>;

  // Iterates over the options (operands) of an opened AML handle.
  fn iter_options(&self, handle: AmlHandle) -> Result<Vec<AmlOperand>, AmlError>;

  // Sets the option (operand) at a particular index to the given value.
  fn set_option(&self, handle: AmlHandle, idx: usize, new_val: AmlOperand) -> Result<(), AmlError>;

  // Returns the first child of an AML node. 
  fn get_children(&self, handle: AmlHandle) -> Result<impl Iterator<AmlHandle>, AmlError>;

  // Returns the next sibling of an AML node.
  fn get_sibling(&self, handle: AmlHandle) -> Result<impl Iterator<AmlHandle>, AmlError>;

  // The above two functions are intended to provide a complete traversal implementation.
  // For example, to get all the children of a node, find the first child through `get_child`, then use `get_sibling` on each subsequent child. In both cases, `None` indicates no child/sibling. 
}
```

The canonical implementation will be provided by `StandardAmlParser`.

```rust
#[derive(IntoService)]
#[service(dyn AmlParser)]
struct StandardAmlParser {
    actives_handles: HashSet<AmlHandle>,
}

impl AmlParser for StandardAmlParser { ... }
```

### Trait Implementation

#### `open_table`

Finds the table (usually DSDT or SSDT) referenced by `table_key` and returns a handle for further AML operations.
Internally this also parses the bytes of the referenced node at the start of the table's AML stream
and sets up its fields as an `AmlSdtHandleInternal`, then adds it to `active_handles`.

#### `close_sdt`

Finds the table corresponding to the node's `table_key` field and if modified, updates its checksum.
Removes the node from `active_handles`.

#### `iter_options`

Iterates over a handle's `operands`.

#### `set_option`

Sets the operand at `idx` to `new_val` and sets `modified` = `true` for the handle.

#### Iteration

The iteration of AML namepsaces is complex. While other service implementations can provide any implementation of
`get_children` and `get_siblings`, the `StandardAmlParser` will use two functions under the hood:

```rust
// Returns the first child of an AML node. 
fn get_child(&self, handle: AmlHandle) -> Result<Option<AmlHandle>, AmlError>;

// Returns the next sibling of an AML node.
fn get_sibling(&self, handle: AmlHandle) -> Result<Option<AmlHandle>, AmlError>;
```

##### **get_child**

First check if `HAS_CHILD_OBJ` is `true` in `attributes` (if there are no children, this function returns None.
This is not to be confused with the outer `Result<Option<AmlHandle>, AmlError>`,
which considers `None` / no children as a success case.)

AML objects are encoded in memory as such:

```plain-text
opcode | pkg_length | [ operands ] | [ TermList (children) ]
```

So the first child of an object is at `offset + sizeof(pkg_length) + sizeof(operands)`.
Once discovered this child becomes an active `AmlSdtHandleInternal`.

The child derives `table_key` from its parent handle, and computes `parent_end` from the handle on which `get_child` is called.

##### **get_sibling**

As stated above, `get_sibling` and `get_child` together provide a full set of traversal operations.

In AML, children are consecutive, so the next sibling of a node is at `offset + size`.

There are no more siblings when `offset + size` >= `parent_end`.

The new handle derives `parent_end` from the sibling on which `get_sibling` is called.
The only exception is the "root" node -- the node on which `open_table` is initially called,
since this node has no siblings and no parent from which to derive `parent_end`.
As such, the `parent_end` of this table is simply at the end of the table, which is `table_length - ACPI_HEADER_SIZE`.

##### **Iterators: `get_children` and `get_siblings`**

When `get_children(handle)` is called, it initializes the iterator state `self.current = get_child(handle)`.
`next` advances to the next sibling of the child, i.e. `self.current = get_sibling(self.current)`.

When `get_siblings(handle)` is called, it initializes the iterator state `self.current = get_sibling(handle)`.
`next` advanced to the next sibling of the current node, i.e. `self.current = get_sibling(self.current)`.
Note that this only gives *subsequent* siblings and not previous ones;
it is standard in AML implementations that nodes operate like singly-linked lists
with knowledge of their forward links (subsequent siblings) but no backward links (previous siblings).

## Guide-Level Explanation

The general flow for using the `AmlParser` service will be:

1. Set up and install necessary tables with the `AcpiProvider` service.
2. Open a DSDT or SSDT with `open_table`.
3. Traverse as necessary through `get_child`, `get_sibling`, and `get_option`.
4. Make necessary modifications through `set_option`.
5. During traversal, close nodes which no longer need to be accessed through `close_handle`.
6. When traversal / patching is complete, `close_handle` on the root node (originally opened with `open_table`).
