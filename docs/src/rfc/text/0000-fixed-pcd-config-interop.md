# RFC: EDK II Build System `Config<T>` Interop

This RFC proposes a translation mechanism to allow fixed-at-build EDK II build system configuration mechanisms--which will be referred to as "fixed EDK II configurations"--to be mapped to Patina `Config<T>` structure fields through a level of indirection that will prevent the concept of PCDs from being directly introduced into Patina source code.

For the purposes of this RFC, the following are considered fixed-at-build EDK II configurations: fixed PCDs (including FixedAtBuild, PatchableInModule, and FeatureFlags) and EDK II build variables. In the current version of this RFC, dynamic PCDs are considered out-of-scope, but may be introduced in a follow-up-RFC. A table of considered configurations to pass through with this RFC can be found in the "Considered EDK II Configurations" section of this document.

Note that while fixed PCDs and PCD feature flags in EDK II can be overwritten per-module, per-component PCD overrides are considered out-of-scope in this RFC, and, should this document be approved, will likely be addressed in a follow-up RFC.

While exact specifics are to-be-determined following a prototyping phase, the basic mechanism for configuration transfer is as follows:

1. Procedural macros are used to generate representations of configuration structures that are stable across versions of Rust (likely `#[repr(C)]` and `#[align(1)]`)
2. On the EDK II-side, Patina configuration `.inf` files (as denoted with `MODULE TYPE = PATINA_CONFIGURATION`) map fixed EDK II configurations to Patina configuration structure fields
3. Using a top-level `Cargo.lock` file, a Rust EDK II build system plugin locates all valid configuration structures within Patina-related source files and creates portable instances of their stable representations populated with the values of the mapped fixed EDK II configurations
4. The Rust EDK II build system plugin places these representations into either one combined or several split (TBD) HOBs through an auto-generated PEIM
5. A Patina `ExternalConfigurationExtractor` Component parses these HOB(s), converting them into their non-portable struct counterparts, checking whether they should be considered "enabled" and installing them as `Config<T>`s as appropriate.

Additional details on the above steps can be found in the sections below.

## Change Log

- 2025-09-08: Initial RFC created.
- 2025-09-09: Updated to second draft of RFC.

## Motivation

The ability to configure Components is vital to ensue that they can be reused across projects (or platforms within those projects) which may have different memory layouts, feature requirements, and expected behaviors. While Patina Components can already be configured in Rust, integration with outside build systems minimizes room for mismatches between values configured within and outside of Patina. **Put simply, platform configuration ought to be defined only once, and EDK II mechanisms are the common denominator**

While PCDs as a concept could be added to Rust themselves, this FRC opts for a level of indirection (in the form of a key-value store of configuration fields to fixed EDK II configuration values) to avoid tightly coupling Patina with a non-Cargo build system. While this RFC addresses EDK II specifically with an EDK II build plugin, other build systems could theoretically be supported by the same Patina-side changes with an equivalent plugin.

## Goals

The goal of this RFC is to create a translation layer that allows fixed EDK II configuration values to be applied to Patina configuration structures. This layer should:

1. Be automatic and minimize potential for mismatched or incomplete configurations by emitting build-time errors when possible
2. Be ergonomic and require minimal user interaction from both EDK II and Patina
3. Be EDK II-unaware from patina's perspective
4. Be efficient of memory use
5. Provide a mechanism for evaluating whether a given configuration is to be considered enabled (and subsequently installed as a `Config<T>`)

## Requirements

1. Configuration must not break compatibility with `cargo` and Cargo registries
2. Modifying configuration values must not require modifying or rebuilding Rust crates.
3. Modifying fixed EDK II configurations within the EDK II build system must be sufficient to change the corresponding value within the installed Rust `Config<T>`

## Software Components

### 1. EDK II Build System Plugin

The EDK II build system plugin, which will likely be written in combination of Rust and Python, will be responsible for gathering the mapping of fixed EDK II configuration values to Rust configurations, and arrange for a HOB to be published containing those values in the stable structure format.

Whether this plugin would best be unstreamed to Tianocore or modularly included from a Patina EDK II repo as needed depends on the degree of invasiveness into the EDK II build system required to implement it. This will be best determined after an initial prototyping phase.

#### Specifying Mappings

Mappings between fixed EDK II configurations and Patina configuration structures will be defined in a `[PatinaConfigurations]` section of any `.inf` included in a `.dsc` file of `MODULE_TYPE = PATINA_CONFIGURATION`. Different syntax, will be used to specify different types of configurations. The following is an example of such a `.inf`, but the exact format is still TBD.

```yaml
[Defines]
 INF_VERSION                    = 0x00010005
 MODULE_TYPE                    = PATINA_COMPONENT

[PatinaConfigurations]
 MyConfigStruct.Field1 = PcdNameSpace.PcdName             # PCD (FixedAtBuild, PatchableInModule)
 MyConfigStruct.Field2 = PcdNameSpace.PcdFeatureFlagName  # PCD (FeatureFlag)
 MyConfigStruct.Field3 = $(BUILD_SYSTEM_VAR_NAME)         # Build system variable
```

For simplicity, nested configuration structs are out-of-scope for this RFC.

#### Determining Patina Portable Struct Layout

The EDK II build plugin will use a form of cross-crate reflection to inspect the portable layout of referenced structs in the crates in which they are defined. These portable layouts are automatically generated by the procedural macros in the `ExternalConfigurationExtractor` crate, as described in its section below. To ensure the correct version of the crate is used, the top-level `Cargo.lock` file of the Crate containing the invocation of the Patina core will need to be referenced by the plugin. From there, the build plugin will download and inspect the source code as appropriate using either `syn` or the JSON output mode of `cargo rustdoc`. The exact mechanism is TBD pending the prototyping phase of this RFC.

#### Verifying Mapping Compatibility

Once the structure of the portable-version of the targeted structs are known, the EDK build plugin will verify three things:

1. All configuration structure and field names exist in valid Rust configuration structures
2. If any field in a configuration structure is mapped, that all fields in that configuration structure is mapped
3. Compatibility between the data types of the fixed EDK II configurations and Patina configuration struct fields

The strictness of the the third verification (type-checking) is an open question.

Fixed-at-build PCDs come in `u8`, `u16`, `u32`, `u64`, `bool`, and byte-array (`VOID*`) variants. PCD feature flags come in type `bool` whereas EDK II build system variables are ASCII strings.

While is is a reasonable assumption that, when it comes to integer types, that any unsigned integer type can be converted into the same type or a wider type, a few of the questions that remain are:

- Should you be able to convert a wider integer into a narrower integer if the value fits in the narrower type?
- Should it be allowed that unsigned integer types be converted into signed types? If so, should it be a build error if that value would then be negative?
- Should byte arrays be allowed to be converted into `zerocopy::FromBytes` slices? Should non-byte-array types be able to?

#### Constructing Instances of Portable Structs

After the mappings have been verified to be compatible, instances of the portable structs will be generated and placed into either one or multiple HOBs (TBD). While the exact mechanism for this is TBD, likely candidates are compiling a Rust portion of the EDK II build plugin with `syn`, or generating byte-compatible (but not named) representations of the structs.

#### Orchestrating the HOB(s) Installations

Once HOB(s) have been generated, the EDK II build plugin will ensure the HOBs are installed by auto-generating and including a minimal PEIM which serves that purpose.

### 2. Patina `ExternalConfigurationExtractor` Component

The `ExternalConfigurationExtractor` component will have four primary responsibilities:

1. Unpacking the portable configuration structs from the HOB(s)
2. Converting those portable configuration structs to their unmodified originals
3. Determining whether that configuration structure is "enabled" and ought to be installed
4. Installing "enabled" configuration structures as `Config<T>`s

To aid in the above steps, procedural macros defined within the crate will automatically generate the following for each compatible configuration structure:

- A representation of the structure that is stable across versions of Rust (likely `#[repr(C)]` and `#[align(1)]`)
- A routine to convert the stable structure to the original form, to check whether it is "enabled", and to install it as a `Cargo<T>` from a HOB

Whether or not it is possible to achieve this without a manually specified `#[derive]` on all configuration structs or top-level build support is to-be-determined following the prototyping phase.

If it is not possible for a portable representation of a configuration structure to be automatically generated (such as if the configuration struct contains a `Box<T>`), a manually defined portable representation and conversion routine will need to be provided. The exact mechanism for this is an open question.

#### Unpacking The Portable Configuration Structs

The implementation of unpacking the portable configuration structs is dependent on whether a single HOB is used, or whether individual configuration structure HOBs are used.

In the single-HOB option, the portable configuration structures are packed together in one contiguous structure, with each struct prepended by a the name of the structure as an ASCII string.

In the multiple-HOB option, `HOB<T>`s would be generated procedurally per configuration struct (with their HOB GUIDs generated procedurally as well). In this scenario the portable structs would be converted to `HOB<PortableConfigurationStruct>` before further processing.

Which of these methods should be used is an option question. The single-HOB option has the benefit of simplicity and room for future metadata which would facilitate extensions such as Component-specific `Config<T>` instances, whereas the multi-HOB option is more in keeping with how existing HOB-based configurations are converted into `Config<T>`s.

#### Converting Portable Configuration Structs into their Original Counterparts

The routines to convert the portable configuration structs into their original (likely non-Portable) counterparts will be generated via the procedural macro described above.

#### Determining Whether a Configuration is "Enabled"

Before installing the configurations structures as `Config<T>`s, the component will check to determine whether a given configuration is considered "enabled". The exact mechanism for this is TBD, but a candidate would be to have some `ConditionallyEnabled` trait that can be derived or implemented for configuration structures that defines the function `fn is_enabled(&self) -> bool`.

#### Installing "Enabled" Configuration Structures

After verifying a configuration structure is "enabled", the Component will install the structure as a `Config<T>`, making it available for use by other components.

## Unresolved Questions

1. How strict should type checking be between fixed EDK II configurations and Patina configuration struct fields? (See: "Verifying Mapping Compatibility")
2. Should the portable configuration structures be stored in a single HOB, or in multiple HOBs (one per configuration structure)? (See: "Unpacking The Portable Configuration Structs")
3. How should custom portable structure definitions and conversion routines be provided for non-`#[repr(C)]`-safe configuration structs? (See: "Patina `ExternalConfigurationExtractor` Component" section)
4. How should the Patina `ExternalConfigurationExtractor` component determine whether a configuration structure is "enabled" and should be installed as a `Config<T>`? (See: "Determining Whether a Configuration is "Enabled"")

## Alternatives

### Alternative 1: Manual Definition in Code Containing Invocation of Patina

- PCD configuration values can be manually copied from the EDK II build system to Rust code containing the platform's invocation of Patina DXE Core
- Issues
    - Requires maintaining parity between EDK II and Rust code manually, which is extremely error prone

### Alternative 2: Single PEIM Installing a HOB

- Instead of using a build system tool, a single PEIM can install the HOB which it populates from PCDs it requires through its own `.inf`
- Issues
    - The potential for struct-layout mismatches would require sharing a set of header files which cover all Patina configuration structures
    - All PCDs would be in the PEIM scope
    - There is no good way to provide per-component PCD overrides
    - Layout errors and struct field mismatches wouldn't be easily caught

### Alternative 3: Single + Per-Component (or Per-`Config<T>`) PEIMs

- Instead of using a build system tool, one central PEIM and per-component PEIMs which would install split HOBs
- Issues
    - The potential for struct-layout mismatches would require sharing a set of header files which cover all Patina configuration structures
    - All PCDs would still be in the PEIM scope
    - Layout errors and struct field mismatches wouldn't be easily caught
    - Unergonomic and hard to keep track of

### Alternative 4: Consolidate All Required PCDs into a "PCD Store" HOB

- Rather than trying to generate Rust-compatible structs from the EDK II build system, publish all required PCDs in a consolidated HOB which a Patina-side component would split apart into `Config<T>`s
- Fields could be populated with PCDs gathered through a PCD macro: `PCD!(PcdNamespace, PcdName)`
- Issues
    - Deserialization to Rust-compatible types (and the errors which may arise) would be runtime, not build-time
    - Patina components would need to be aware of the EDK II build system, and would need to keep track of PCD names and namespaces

## Considered EDK II Configurations

The following EDK II configurations have been considered for initial inclusion in this RFC. The final list of allowed configurations is open to debate.

| EDK-II-Related Configuration | Included | Reason                                                                                                                                                                                                  |
| ---------------------------- | -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| FixedAtBuild PCDs            | Yes      | Fixed at build time                                                                                                                                                                                     |
| FeatureFlag PCDs             | Yes      | Fixed at built time                                                                                                                                                                                     |
| PatchableInModule PCDs       | Yes      | Fixed at build time                                                                                                                                                                                     |
| EDK II Build System Variable | Yes      | Fixed at build time                                                                                                                                                                                     |
| Dynamic PCDs                 | No       | Currently out of scope as their handling on the EDK II build-extension side would be very different than fixed-at-build values. This may be added to the current document, or added in a follow-up RFC. |
| C Compiler Flags             | No       | Lack of clear use case when EDK II Build System Variable support exists.                                                                                                                                |
| UEFI Variables               | No       | Outside scope and covered by Variable Services.                                                                                                                                                         |
