# RFC: Patina Package

This RFC proposes:

- A new OpenDevicePartnership repo called "`patina-edk2`"
- A new Patina package in the `patina-edk2` repo called `PatinaPkg`

## Change Log

- 2025-07-14: Initial draft of RFC.
- 2025-07-15: Note automated process open under the "Unresolved Questions" section.
- 2025-07-15: Removed `UefiCpuPkg` (in `edk2`) as an allowed dependency at this time.

## Motivation

As the Patina project grows, it is inevitable that it needs to define content that it is made available to interact
with edk2 C code today, such as GUID definitions for HOBs that are exclusively defined by Patina components. There is
not a clear common package to put this content in today - Patina is a project independent of Project Mu and edk2.

This RFC proposes a new repo named `patina-edk2` that will contain a new package called `PatinaPkg`. This repo can
be used as a submodule in any platform that needs to consume Patina content. This repo would be entirely optional
for a Patina consumer.

### Performance Component Example

For example, the change described in [[Feature]: Support dynamic configuration of the patina_performance component](https://github.com/OpenDevicePartnership/patina/issues/578)
requires a new HOB GUID to be defined that can be used by platform HOB producer code to populate the HOB with the
Patina performance configuration from a platform-specific data source. Platform C code needs the HOB GUID and HOB
structure to be defined in a common location. Ideally, that would follow normal edk2 practices where a package clearly
owns declaration of the GUID and its type and platform code can clearly establish a dependency on that package. This
proposes that the package owner be `PatinaPkg` in the `patina-edk2` repo.

## Technology Background

This RFC does not impact any technology specifically. The [EDK II Package Declaration (DEC) File Format Specification](https://tianocore-docs.github.io/edk2-DecSpecification/release-1.27/)
may be useful as a general reference to understand how `PatinaPkg.dec` and the overall package would be constructed.

## Goals

1. Provide an obvious, single location for Patina content that is intended to be consumed by edk2 C code.

## Requirements

1. **Single Repository** - All Patina content that is intended to be consumed by edk2 C code must be in a single
   repository.
2. **Proper Scope** - The `patina-edk2` repository should only contain Patina definitions that are intended to be
   consumed by edk2 C code.
   - This includes Patina-specific HOBs, GUIDs, and other content that is not intended to be used by Rust code.
   - The repository should not contain any Rust code or Patina crates.
3. **Proper Classification** - The Patina repository should be rarely used within Patina work. Patina is not meant to
   establish a large set of C-specific content that is not already defined in specifications and made available
   through existing means. However, Patina will define some new content that is not defined in specifications and may
   be made available for use in edk2 C code. In that case, this repository is the proper place to put a single
   officially maintained copy of that content.
4. **Minimal Dependencies** - The repository is not allowed to depend on any content other than `MdePkg` and
   `MdeModulePkg` from upstream `edk2`. Any other dependency to an `edk2` package must be requested via RFC and
   Project Mu dependencies are completely disallowed.
5. **No Dependencies From Patina** - No other Patina repository can depend on content in this repository. No other
   Patina repository documentation should reference details in this repository. The repository should be able to be
   removed from the Patina project at any time without impacting any other Patina repository.

## Unresolved Questions

- Should we consider a strict generation process such as content being required to be generated from a tool like
  `cbindgen`? This would ensure that the content is always in sync with the Patina codebase and avoids manual
  maintenance. This proposal is avoided at this time to avoid the implication that Patina C generated content is
  so extensive that it requires a formal process and generation tool. That is certainly subjective though and this
  RFC simply does not propose this requirement at this time.

## Prior Art (Existing PI C Implementation)

- There is no existing location that is ideal for hosting Patina authored content that is intended to be consumed by
  edk2 C code.
