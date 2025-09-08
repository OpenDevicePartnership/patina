# RFC: `Firmware Filesystem Crate`

The Firmware File System (FFS) is defined in the Platform Initialization (PI) specification. Presently, both the struct
definitions that describe the FFS structures as well as the business logic that provides APIs for accessing FFS data
are implemented in the mu_rust_pi crate [https://github.com/microsoft/mu_rust_pi/blob/main/src/fw_fs.rs](https://github.com/microsoft/mu_rust_pi/blob/main/src/fw_fs.rs).
The rest of the mu_rust_pi crate is bare spec structure definitions, with only the FFS module having extensive business
logic in this crate. This RFC proposes implementing a new FFS-specific crate in Patina to hold the FFS logic. This will
allow mu_rust_pi to be a spec-definition only crate. In addition, this RFC proposes a number of enhancements to the FFS
implementation to allow usage models outside the core (e.g. in development of command-line utilities).

## Change Log

- 2025-06-12: Initial RFC created.

## Motivation

There are a number of motivations for this RFC:

1. Narrow scope of mu_rust_pi: The existing FFS implementation in mu_rust_pi is inconsistent with the rest of that crate
which otherwise consists of
raw structure definitions from the PI spec. Moving the FFS implementation (except for the spec structures) out of
mu_rust_pi allows it to be focused on a single concern (providing canonical Rust structure definitions for the PI spec).
2. Coalesce section extractor implementation: Moving the FFS logic into Patina allows for the patina_section_extractor
to merge with the FFS crate.
3. Improve/refactor FFS implementation: The FFS implementation was some of the earliest code in Patina, and parts of it
will benefit from significant refactoring. In particular, the `SectionExtractor` trait, the behavior of the iterators
(returning `Option<Result<x>>`), the naming conventions for structures, and the overall borrowing model need to be
re-examined.
4. Add FFS generation capabilities: The FFS implementation presently only supports read-only data access on FFS, and
does not support FFS generation. There are a number of use cases, particularly for tooling, that would benefit from an
implementation that supports both read access to FFS instances as well as programatic generation of FFS data.

## Technology Background

The Firmware File System (FFS) defined in the Platform Initialization (PI) spec provides a structured data organization
for files used in PI compliant firmware (most UEFI implementations). It provides a number of benefits for use in typical
firmware storage devices such as NOR flash.

## Goals

1. Move FFS implementation from mu_rust_pi into a library crate in Patina and move patina_section_extractor into it.
2. Refactor FFS implementation to clean up design and implementation choices.
3. Add support for FFS generation.

## Requirements

1. Retain existing capabilities of FFS as used by Patina core. Breaking changes to the API are in scope, but removal of
existing capabilities of the mu_rust_pi implementation are not.
2. Crate produces an API for reading and accessing FFS data (existing functionality)
3. Crate produces an API for generating new FFS volumes and modifying existing volumes and serializing them back into
PI-spec confirmant representations (new functionality)

## Prior Art

- mu_rust_pi implementation: <https://github.com/microsoft/mu_rust_pi/blob/main/src/fw_fs.rs>
- edk2 FV implementation: <https://github.com/tianocore/edk2/blob/master/MdeModulePkg/Core/Dxe/FwVol>
- edk2 section extraction implementation: <https://github.com/tianocore/edk2/tree/master/MdeModulePkg/Core/Dxe/SectionExtraction>
- edk2 build tools (GenFfs and GenFv): <https://github.com/tianocore/edk2/tree/master/BaseTools/Source/C>
- UefiTool ffs parser: <https://github.com/LongSoft/UEFITool/blob/new_engine/common/ffsparser.cpp>
- PI spec FFS section: <https://uefi.org/specs/PI/1.9/V3_Design_Discussion.html#firmware-storage-design-discussion>

## Alternatives

Other alternatives considered:

1. Do nothing. Retain existing design point and develop FFS generation capability independently. This would reduce the
cohesion of the design, and limit the availability of the generation capability. In addition, one of the learnings of
the evolution of Patina was that it makes sense to properly organize and co-locate related code (see: RFC
0006-patina-repo.md).

2. Develop FFS generation capability at current location in mu_rust_pi. See motivation section above for the concerns
with this approach, but basically, current FFS implementation is the only "implementation" in mu_rust_pi which is
otherwise concerned with defining spec structures for PI.

3. Move both FFS read and generation into an independent crate outside patina. This is a potential option, (see "crate
location" section in "Unresolved Questions", above), but there are synergies with section extraction and sdk
implementations that make sense to put it in the core.

## Rust Code Design

This RFC does not contain a detailed API for the proposed changes as developing/extension of the API is part of the
proposed work to be done if this RFC is accepted. The following section sketches out a high-level design.

Draft PRs for proposed changes are here:
[https://github.com/OpenDevicePartnership/patina/pull/706](https://github.com/OpenDevicePartnership/patina/pull/706)
[https://github.com/microsoft/mu_rust_pi/pull/84](https://github.com/microsoft/mu_rust_pi/pull/84)

### Main Data Objects

The PI spec for FFS has 3 main data objects: "Firmware Volumes" consisting of a collection of "Firmware Files," which in
turn consist of "Sections" some of which may be "encapsulation" sections that contain additional "Sections". The FFS API
will mirror this structure, having `Volume`, `File` and `Section` structures corresponding to each of these 3 data
objects.

### Ownership

There are two main use cases for FFS generation: fast read-only access to FFS in a constrained environment (such as the
Patina dispatcher), and more heavy-weight read/write access to process, modify, and generate new FFS instances for host-
side utilities.

For fast read-only access, a zero-copy "borrowed" version of the FFS data objects and corresponding APIs
is proposed. This version of the API operates on a supplied byte slice, and only makes allocations as necessary to
expand sections. This API would largely match what is implemented in mu_rust_pi today.

For FFS generation and manipulation, an "owned" version of the FFS data objects and corresponding APIs is proposed. This
version of the API maintains the data for collections at each level (e.g. a `Volume` would have a `Vec<File>` member)
instead of accessing an underlying data buffer. In addition to having the same types of read accessors as the "borrowed"
version, this API would additionally have write accessors that would allow manipulation of the FFS data, as well as
serialization APIs (e.g. `to_vec(&self) -> Vec<u8>`) that would produce a byte-array representation of the FFS.

### Iterators

Since each level of the data hierarchy in FFS is composed of a collection of sub-elements (i.e. an FFS `Volume` contains
a collection of FFS `File`s), each of the proposed data structures will provide a method to obtain an iterator over its
contents.

### Crate Location

The proposed crate will be implemented in the `sdk/patina_ffs` path within the Patina repo.

## Guide-Level Explanation

Documentation (including examples and tests) will be developed as standard rustdoc for the proposed patina_ffs crate.
The module level-documentation in the crate should be sufficient as a guide-level explanation for the proposed APIs.
