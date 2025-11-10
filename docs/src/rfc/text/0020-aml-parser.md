# RFC: `ACPI SDT AML Handler`

This RFC proposes a Rust-based AML (ACPI Machine Language) service that extends the existing ACPI service by
providing a safe, ergonomic interface for parsing and modifying AML bytecode in firmware during the DXE phase.
The implementation mirrors the ACPI SDT protocol’s functionality for AML traversal and patching
and is designed mainly for firmware use rather than OS-level interpretation of AML bytecode.
It defines a structured system of AML handles for navigating AML streams and a trait-based `AmlParser` service for operations
such as opening tables, iterating operands, modifying values, and traversing child or sibling nodes.
The goal is to replace legacy C-based AML handling with a type-safe Rust service that supports ACPI 2.0+.
Future extensions may include extending this infrastructure for application-side AML interpretation
within a `patina-acpi` crate.

## Change Log

- 2025-10-1: Initial RFC created.
- 2025-10-22: Update iteration interface and address prior art.
- 2025-10-28: Move to FCP.
- 2025-10-28: Add notes based on offline discussion of AML buffer parsing.
- 2025-11-10: Completed RFC.

## Motivation

This RFC is an extension of the [ACPI service](0005-acpi.md).
Similar to the ACPI service, this Rust-based AML service will provide a safer and
more ergonomic interface for parsing and modifying AML bytecode.

## Technology Background

Compiled ACPI tables such as the DSDT and SSDTs are composed of AML bytecode.

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

1. Redesign existing C firmware AML interaction functionality into a safe, easy-to-use Rust service.
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
This would involve mostly exposing and abstracting out common lower-level parsing code
(such as parsing NameSegs, reading PkgLengths, etc).
For `AmlParser` to consume the `aml` crate parsing code as a dependency, it has to:

1. Allow external consumers of the crate to directly use parsing code.
2. Change these parsing interfaces to accept some kind of generic buffer,
rather than depending on the `Interpreter` struct (which can only be constructed in a post-boot OS environment, not firmware).

This conversation can be tracked through a [Github issue in the `acpi` crate](https://github.com/rust-osdev/acpi/issues/260).

## Rust Code

### ACPI/AML Utility Crate

While the `AmlParser` service has functionality specific only to the UEFI ACPI SDT protocol,
it will be useful for extensibility to extract basic AML reading and patching capability into a separate utility crate,
`patina-acpi`. `AmlParser` will depend on this crate.

For example, basic parsing functionality like reading the package length, parsing name segments, etc.,
will be included in this utility crate.

Unlike the `AmlParser` struct, this crate will operate independently of the ACPI table system--
it does not directly deal with ACPI structures like the XSDT.
Instead, it receives a generic buffer that represents AML bytecode beginning at the start of some ACPI table.
The advantage of this paradigm is that this code can be reused for other parsing applications,
and can be reused partially *from* the existing Rust `acpi` crate.

This proposed `patina-acpi` utility crate crate shares functionality with the Rust  `os-dev acpi` crate; as such,
it remains to be seen if collaboration is possible to expose this shared parsing functionality,
or if `patina-acpi` will borrow/fork from `acpi`.

### AML Handles

By spec definition, an AML handle is an opaque handle returned from opening a DSDT or SSDT,
on which AML traversal and patching operations can be performed.

Internally, each `AmlHandle` object, aliased as `AmlSdtHandleInternal`,
represents a cursor within an AML stream. Each handle can be conceptualized as a "node"
in the AML object tree of parent-child-relationships.

```rust
pub(crate) struct AmlSdtHandleInternal<'t> {
    table: &'t mut AcpiTable,
    offset: usize,
    size: usize,
    name: NameSeg,
    byte_encoding: AmlByteEncoding,
    parent_end: Option<usize>,
}

impl AmlSdtHandleInternal {
    fn new(table: &'t mut AcpiTable, offset: usize, size: usize) -> Self {
        Self {
            table,
            offset,
            size,
            byte_encoding: AmlByteEncoding::default(),
            parent_end: None,
        }
    }

    pub fn table_key() -> TableKey {
        self.table.table_key
    }
}

pub type AmlHandle<'t> = AmlSdtHandleInternal<'t>;
```

The `size` of an `AmlSdtHandleInternal` refers to its full size, including any children (`TermList`).

The `offset` refers to its offset within the AML stream of the table.
Offset 0 is the start of the AML stream, and the highest offset is at the end of `table_length`.

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

### AML Paths

A path in AML refers to a specific node in the tree hierarchy.
It is represented by a series of `NameSegs`:
4-character uppercase ASCII strings that mark a specific location in the AML namespace.

```rust
/// NameSegs are always 4 ASCII characters long.
pub struct NameSeg([u8; 4]);

/// A path can have any number of NameSegs.
pub type AmlPath = Vec<NameSeg>;
```

### AML Trait Interface

The `AmlParser` service generally derives from the ACPI SDT protocol, and allows for traversal of the AML object tree.

```rust
pub(crate) trait AmlParser {  
  // Opens a table's AML stream for parsing. The table should be a DSDT or SSDT. 
  // The resulting handle is an opaque object on which further AML operations can be performed.
  // It points to the first (root) node in the AML stream.
  unsafe fn open_table(&self, table_key: TableKey) -> Result<AmlHandle, AmlError>;

  // Iterates over the options (operands) of an opened AML handle.
  fn iter_operands(&self, handle: AmlHandle) -> Result<Vec<AmlOperand>, AmlError>;

  // Sets the option (operand) at a particular index to the given value.
  fn set_operand(&self, handle: AmlHandle, idx: usize, new_val: AmlOperand) -> Result<(), AmlError>;

  // Finds the AML node at a specific known path.
  // AML paths take the format: \\_AA.BBBB.CCCC....
  fn find_path(&self, path: AmlPath) -> Result<AmlHandle, AmlError>;

  type AmlIter: Iterator<Item = Result<AmlHandle, AmlError>>

  // Iterates over all nodes of the tree (in a depth-first order).
  fn iter(&self) -> Self::AmlIter;
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
and sets up its fields as an `AmlSdtHandleInternal`.

#### `iter_operands`

Iterates over a handle's `operands`.

#### `set_operand`

Sets the operand at `idx` to `new_val` (by writing to the bytecode buffer).

This should also update the table's checksum; the table can be obtained by using the stored `table` information:

```rust
let table = handle.table_key();
table.update_checksum();
```

#### Iteration

The iteration of AML namespaces is complex. While other service implementations can provide any implementation of
`iter`, the `StandardAmlParser` will use two functions under the hood:

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

The child derives `table` from its parent handle, and computes `parent_end` from the handle on which `get_child` is called.

##### **get_sibling**

As stated above, `get_sibling` and `get_child` together provide a full set of traversal operations.

In AML, children are consecutive, so the next sibling of a node is at `offset + size`.

There are no more siblings when `offset + size` >= `parent_end`.

The new handle derives `parent_end` from the sibling on which `get_sibling` is called.
The only exception is the "root" node -- the node on which `open_table` is initially called,
since this node has no siblings and no parent from which to derive `parent_end`.
As such, the `parent_end` of this table is simply at the end of the table, which is `table_length - ACPI_HEADER_SIZE`.

Note that this only gives *subsequent* siblings and not previous ones;
it is standard in AML implementations that nodes operate like singly-linked lists
with knowledge of their forward links (subsequent siblings) but no backward links (previous siblings).

##### `iter`

`iter` iterates over all nodes (handles) in the current AML namespace.
The particular implementation of `StandardAmlParser` does so in a DFS manner, but other
service implementers can chose any implementation that traverses all nodes.

With `get_sibling` and `get_child`,
`iter` can be implemented using a standard preorder stack-based tree traversal algorithm.
A stack is initialized with the first node in the AML bytecode (the "root").
Then, on each subsequent iteration, the node at the top of the stack is popped,
and its child (`get_child`) and sibling (`get_sibling`) are enqueued (in that order).

##### `find_path`

`find_path` traverses the tree in a similar matter as `iter`, but looks for a specific named node.
Given a series of `NameSeg`s as an `AmlPath`,
it uses `get_sibling` at each level to find the node matching that level's `NameSeg`,
then proceeds onto the next level using `get_child` until the node at the end of the path is found.

The name of a node can be read easily as all named nodes follow the format:

```plain-text
[Opcode] [PkgLength] [NameSeg] [...]
```

### Handle Lifetime

Since each handle points to memory within an ACPI table, a handle cannot outlive its corresponding table.
This is guaranteed at compile-time by the `table: &'t mut AmlTable,` within the `AmlSdtHandleInternal<'t>`.
(The main case this addresses is when a table is uninstalled but an open handle still references the table.)

## Guide-Level Explanation

The general flow for using the `AmlParser` service will be:

1. Set up and install necessary tables with the `AcpiProvider` service.
2. Open a DSDT or SSDT with `open_table`.
3. Traverse as necessary through `iter` and `find_path`.
4. Make necessary modifications through `set_operand`.

### Example

For example, suppose `AmlParser` is being used to parse the following DSDT
and patch `VAL0` from `0x00` (invalid value) to `0x99` (some valid value):

<!-- cspell:disable -->
```plain-text
0000: 44 53 44 54 54 00 00 00 02 7D 4F 45 4D 49 44 20  DSDTT....}OEMID 
0010: 45 58 41 4D 50 20 20 20 20 31 00 00 00 41 4D 4C  EXAMP    1...AML
0020: 10 1B 5C 5F 53 42 82 15 44 45 56 30 08 56 41 4C  ..\_SB..DEV0.VAL
0030: 30 0A 00 82 0A 43 48 4C 44 08 56 41 4C 31 0A 20  0...CHLD.VAL1. 
0040: 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00  ................
0050: 00 00 00 00                                      ....
```

The first 36 bytes are table header attributes (signature, length, revision, etc).
Thus the trailing AML bytecode is:

```plain-text
                  53 42 82 15 44 45 56 30 08 56 41 4C  ..\_SB..DEV0.VAL
0030: 30 0A 00 82 0A 43 48 4C 44 08 56 41 4C 31 0A 20  0...CHLD.VAL1. 
0040: 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00  ................
0050: 00 00 00 00  
```
<!-- cspell:enable -->

In plain ASL this translates to:

```asl
Scope (\_SB)
{
    Device (DEV0)
    {
        Name (VAL0, 0x00)
        Device (CHLD)
        {
            Name (VAL1, 0x20)
        }
    }
}
```

To begin parsing the DSDT, `open_table(dsdt_key)` is called to obtain a `dsdt_handle` to the start of the bytecode.
(If the DSDT key is not known, `iter_tables` can be used to find the desired DSDT/SSDT.)

```rust
let dsdt_handle = aml_parser_service.open_table(dsdt_key);
```

Assume that `VAL0` is some known or configured firmware path with some known meaning (such as a feature being enabled).

```rust
const IS_VAL_ENABLED_PATH: &str = "\\_SB.DEV0.VAL0";

impl NameSeg {
    fn from_str(s: &str) { ... }
}

let node_handle = aml_parser_service.find_path(NameSeg::from_str(IS_VAL_ENABLED_PATH));
for (idx, option) in aml_parser_service.iter_operands(node_handle).enumerate() {
    if option.name == "VAL0" {
        aml_parser_service.set_operand(node_handle, idx, AmlOperand::ByteConst(0x99));
    }
}
```

In memory, the new bytecode should look like:

```plain-text
                  53 42 82 15 44 45 56 30 08 56 41 4C  ..\_SB..DEV0.VAL
0030: 30 0A *99* 82 0A 43 48 4C 44 08 56 41 4C 31 0A 20  0...CHLD.VAL1. 
0040: 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00  ................
0050: 00 00 00 00  
```

And the DSDT's header will also have a modified checksum.
