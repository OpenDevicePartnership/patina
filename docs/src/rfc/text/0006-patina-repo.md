# RFC: Single Patina Repository

This RFC proposes a single Patina repository that contains all code and documentation for the Patina project.

## Change Log

- 2025-04-29: Initial draft of RFC.
- 2025-04-30: Added a proposal for existing public repositories to eventually be archived and moved into the Patina
  repository.

## Motivation

The UEFI rust work has undergone several iterations of design and implementation since it began. Most recently, the
overall effort has been consolidated under a single project called "Patina". This RFC proposes modifications to the
way Rust code is organized and maintained to improve developer efficiency and align code and docmentation more readily
under a cohesive Patina project umbrella.

## Technology Background

This RFC does not impact any technology specifically but general background on code organization within the project
is provided here for reference.

----

The initial focus for UEFI Rust work was to:

1. Create something meaningful that would add real world value to platforms by leveraging Rust's memory safety
   capabilities.
2. Create something that could be used in production with a reasonable time frame.
3. Create something that could be used by the larger firmware community with code patterns, documentation, libraries,
   methodologies, and tools that could be reused in future work.
4. Identify areas to improve language agnostic firmware design while maintaining a high degree of compatibility with
   existing PI and UEFI firmware.
5. Improve the overall developer experience for firmware developers by adopting modern development practices and tools.

The primary vehicle selected to achieve these goals was writing a DXE Core in Rust. This was considered high value as
it has a single entry point, orchestrates the entire DXE phase boot process, and provides critical services referenced
through binary interfaces. C drivers calling into those services would still be able to leverage functionality such as
Boot Services and DXE Services written in Pure Rust. During that time, many supporting code was created along
the way. In the end, a set of largely independent software entities are integrated to ultimately fulfill the
dependencies necessary for a functional DXE Core.

## Goals

1. Create a single Patina repository that contains *most*\* code and documentation for the Patina project.
2. Reduce the amount of overhead for integration between these crates.
3. Keep Patina issues, pull requests, and documentation in a single location to improve developer ergonomics and
   make it easier to couple documentation updates with code changes.
4. Ease platform integration and overrides of Patina crates by having them defined in a single local workspace.

\* An exception to (1) is code that is entirely independent of Patina firmware and can benefit a larger audience. For
example, the `paging` and `mtrr` crates have no coupling to Patina or firmware and are useful to share with a
broader set of developers to improve the overall quality of that code. These would be maintained by the Patina team but
considered independent repositories.

## Requirements

1. **Single Repository** - The Patina repository should contain all code and documentation for the Patina project.
2. **Patina Internal Crates** - A crate in the Patina repository may be marked for "internal" use only. This means
   that the crate is not intended to be consumed outside of the Patina repository and should not be a dependency in any
   crate outside the Patina repository. Any crate marked as "internal" must have a name that starts with
   `patina_internal_`.
3. **Versioning** - A single version of the Patina repository will be used for all Patina crates and tracked as the
   "Patina version".
   - The Patina version is only updated when a breaking change is made to "Patina public APIs".
   - "Patina public APIs" are publicly exposed APIs in Patina crates not marked as "internal" to Patina.
   - All Patina crates must be published on every Patina release.
4. **Documentation** - The Patina repository should contain documentation for all Patina crates and the Patina project.
   - Crate-specific documentation should generally be maintained in the crate itself.
   - Crates may also be described in common rep0-level documentation.

Note: At this time, some Rust code is maintained by the Patina team that is already public while most of the Patina
code is not public. This RFC proposes that code remains public and once Patina is public, those public repositories are
archived with their code moved into the Patina repository. This RFC does not track which repositories may be moved
to Patina specifically.

## Unresolved Questions

- **Repo Location** - Rename the existing DXE Core repository to `patina` and move all Patina crates into the
  repository.
  - This is suggested to keep issue and pull request history in the repository.
  - Source history from other repositories should be retained where possible.

- Exactly how to refactor existing crates into this new organization model. This is considered out of scope for this
  RFC which defines the new model not the logistics of moving existing code into it.

## Prior Art (Existing PI C Implementation)

This section describes the pre-existing code organization model in place at the time of this RFC.

### Prior Code Organization Model

To allow better separation of maintainership, documentation, release tracking to the GitHub repository, and higher
cohesion among repo contents, the code was organized as follows:

- DXE Core Specific Crates
  - Independent functionality exclusively used by the DXE Core.
  - Examples: DXE Core, Event infrastructure, GCD, memory allocator
- Core Specific Crates
  - Code that is specific to creating core environments, like the PEI, DXE, and MM  environments.
  - Examples: Protocol DB, Common PE/COFF (goblin) wrappers, common section extraction code
- Module Development (SDK) Crates
  - Functionality necessary to build UEFI modules.
    - Can be used by core or individual driver components.
  - Examples:
    - Boot Services & Runtime Services
    - Device Path services
    - Logging related (for modules to write to logs; not the adv logger protocol producer code for example)
    - GUID services
    - Performance services
    - TPL services
- Utility Crates
  - Code that is helpful to build UEFI modules but less common and/or has a strong need for independence.
  - Examples:
    - Code to draw to the screen
    - Crypto
- Feature Crates
  - Well-defined features with focused interfaces and documentation.
  - Examples:
    - DFCI
- Generic Crates
  - Code that is completely independent of UEFI and solves general problems that occur in other software.
  - Example:
    - HID report descriptor parsing, LZMA (bare metal), paging, MTRRs, red-black tree code

The matrix below shows allowed dependencies for each class of crate defined above.

|           | DXE Core | Core      | SDK       | Utility   | Feature   | Generic   |
|-----------|----------|-----------|-----------|-----------|-----------|-----------|
| DXE Core  | x        | Y         | Y         | N         | N         | Y         |
| Core      | N        | x         | Y         | N         | N         | Y         |
| SDK       | N        | N         | x         | N         | N         | Y         |
| Utility   | N        | N         | Y         | N         | N         | Y         |
| Feature   | N        | N         | Y         | N         | N         | Y         |
| Generic   | N        | N         | N         | N         | N         | Y         |

That provided a set of guidelines for organizing content into crates with the following strategies noted for crate
organization into repos:

There are a lot of strategies for assigning crates to repositories.

- Administrative – Largely non-technical. Artificial limits imposed on the number of repos or some organization cost
  associated with repos that influences their creation and constrains their number.
- Ergonomics – Developer efficiency. Not having to switch repositories often between crate dependencies.
- Logistical – Management and consistency. Coordinating work scales in complexity across the number of repos involved.
- Technical – Repos are created solely considering the role they serve in hosting the content placed there.

In particular, the repo defines focus for its content and purpose - the revision history, versioning, documentation,
crate-specific workflows, maintainers, policies, etc. This RFC proposes that Patina encompass more of the content into
a single location improving developer ergonomics and simplifying logistics of managing code withiin the project.

## Alternatives

- Stay on the current code organization model.
  - Not recommended as it does not meet the goals of this RFC.
