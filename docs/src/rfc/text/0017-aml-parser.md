# RFC: `<Title>`

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

This leaves three main paths for the Patina AML implmentation:
1. Design and implement a new Rust AML service from the ground up, without utilizing the existing `acpi` crate.
   - Pros: Interfaces and implementations can be tailored to Patina needs.
   - Cons: Repeated work.
2. Design and implement the firmware side of AML parsing independent of the existing `acpi` crate, and use it only for application-side parsing. This may involve contributing to the `apci` crate to create a more comprehensive public API.
    - Pros: Less work on application side. 
    - Cons: Repeated work on firmware side, since parsing and execution use similar structures and code. Less control over public interfaces.
3. Contribute both firmware and OS code directly to the `acpi` crate, and consume it directly as a Patina dependency. 
    - Pros: Can use both public and private interfaces in `acpi` crate, minimizing repeated work.
    - Cons: May not be appropriate to add firmware code to `acpi`.  

## Rust Code Design

Include diagrams, code snippets, and other design artifacts that are relevant to the proposal. All public facing
APIs should be included in this section. Rationale for the interfaces chosen should be included here as relevant.

## Guide-Level Explanation

Explain the proposal as if it was already included in code documentation and you were teaching it to another Rust
programmer. That generally means:

- Introducing new named concepts.
- Explaining the feature largely in terms of examples.
- Explaining how Rust programmers should *think* about the feature, and how it should impact the way they interact
  with this feature. It should explain the impact as concretely as possible.
- If applicable, describe the differences between teaching this to existing firmware programmers and those learning
  the feature the first time in the Rust codebase.