# Code Organization

The Rust DXE Core is a complex system with many constituent parts. This document describes the organization of the
overall codebase, including the key dependencies that are shared between the Rust DXE Core and other components. The
goal is to provide a high-level overview of these relationships.

This is meant to be a living document, and as the code base evolves, this document should be updated to reflect the
current state.

## General Principles

As we build the elements necessary for a functional DXE Core, many supporting systems must necessarily be created along
the way. In the end, a set of largely independent software entities are integrated to ultimately fulfill the
dependencies necessary for a functional DXE Core. The fact code was conceived to support the DXE Core does not mean it
is intrinsically coupled with DXE Core.

The principles described here are not meant to be more detailed or complex than necessary. Their goal is to support
the organization of Rust code developed in this project in a consistent manner. It is not a goal to describe anything
beyond what is necessary to organize the code per the guidelines given.

### Code Cohesion and Coupling

Cohesion is a popular software concept described in many places including
[Wikipedia](https://en.wikipedia.org/wiki/Cohesion_(computer_science)).

When speaking of cohesion generally, it is a degree to which the elements inside a software container belong together.
In different languages and environments, what is a “container” might vary. But we know that containers with high
cohesion are easier to understand, use, and maintain.

Coupling is another concept commonly covered including its own page on
[Wikipedia](https://en.wikipedia.org/wiki/Coupling_(computer_programming)). This definition is taken directly from
there:

> "In software engineering, coupling is the degree of interdependence between software modules, a measure of how closely
> connected two routines or modules are, and the strength of the relationships between modules. Coupling is not binary
> but multi-dimensional."

Understanding coupling is easy. Minimizing coupling in practice is hard. Coupling is a key driver of technical debt in
code. Having unrelated subsystems tightly coupled will only worsen over time. Developers will use the smallest amount
of coupling as a precedent for further coupling, creating a self-perpetuating cycle. This is a major factor in poor
software design over time. Tight coupling results in:

- **Systems that are difficult to understand and maintain** – If you want to understand how one thing works that is
  coupled, you must now understand how everything else coupled to it works. *They are coupled into one system.*
- **Systems that are difficult to refactor** – To refactor coupled systems, you must refactor all coupled systems.
- **Systems that are difficult to version** – If something is changed in a multi-coupled system, the whole system’s
  version is revised and published. That does not reflect the actual degree of change in the individual elements of
  the system.
- **Systems that are more difficult to test** – A coupled system requires testing layers, dependencies, and interfaces
  irrelevant the interface initially being tested.

This essentially forms “spaghetti code”. Spaghetti code is relatively easy to identify in existing code. Spaghetti
code often begins because of a slip in coupling in one part of the system that starts the cycle. In the end, no one is
quite sure who is responsible for the spaghetti code and how it got that way, but it did. Now it’s a huge mess to clean
and many parts of the system must be impacted to do so. Depending on the complexity of the system, now tests,
documentation, repo organization, and public APIs all must change. This is the “ripple” effect coupling has where the
ripple grows larger based on the degree of coupling and size of the system.

Code in the DXE Core should strive to achieve high cohesion and low coupling in the various layers of “containers”.
This results in higher quality software.

### SOLID

Certain [SOLID](https://en.wikipedia.org/wiki/SOLID) principles apply more broadly outside of pure object-oriented
design than others.

#### Single Responsibility Principle

The single responsibility principle applies in many situations. When designing a set of code, we should ask, “What is
the responsibility of this code?  Does it make sense that someone here for one responsibility cares about the other
responsibilities?”

Given that we are thinking about responsibility more broadly than individual classes, we will take on multiple
responsibilities at a certain level. For example, a trait might focus on a single responsibility for its interface
but a module that contains that trait might not. It is not so important to literally apply a single responsibility
to each layer of code when thinking about organization, but it is helpful to consider responsibilities and how they
relate to the overall cohesion and coupling of what is being defined.

#### Interface Segregation

Another SOLID principle that has utility outside designing classes is the interface segregation principle, which states
that “no code should be forced to depend on methods it does not use”. That exact definition applies more precisely to
granular interfaces like traits, but the idea is useful to consider in the larger composition of software as well as
it affects the cohesion and coupling of components. We should try to reduce the extraneous detail and functionality in
code, when possible, to make the code more portable, testable, and maintainable.

## Organizational Elements: Crates and Modules

A package is a bundle of one or more crates. A crate is the smallest amount of code the Rust compiler considers at a
time. Code is organized in crates with modules. All of these serve a purpose and must be considered.

For example, modules allow similar code to be grouped, control the visibility of code, and the path of items in th
module hierarchy. Crates support code reuse across projects – ours and others. Crates can be independently versioned.
Crates are published as standalone entities to crates.io. Crates allow us to clearly see external dependencies for
the code in the crate. Packages can be used to build multiple crates in a repo where that makes sense like a library
crate that is available outside the project but also used to build a binary in the package.

When we think about code organization at a high-level, we generally think about crates because those are the units of
reusability across projects. That’s the level where we can clearly see what functionality is being produced and
consumed by a specific set of code. Modules can fall into place within crates as needed.

Therefore, it is recommended to start thinking about organization at the crate level.

- What is the cohesion of the code within this reusable unit of software?
- If a project depends upon this crate for one interface it exposes, is it likely that project will need the other
  interfaces?
- Are the external dependencies (i.e. crate dependencies, feature dependencies) of this crate appropriate for its
  purpose?
- Is the crate easy to understand? Are the interfaces and their purpose well documented? If someone wants to understand
  the core purpose of this crate, is that easy? Is there unrelated content in the way?

### Repo and Packages

There are a lot of strategies for assigning crates to repositories.

- Administrative – Largely non-technical. Artificial limits imposed on the number of repos or some organization cost
  associated with repos that influences their creation and constrains their number.
- Ergonomics – Developer efficiency. Not having to switch repositories often between crate dependencies.
- Logistical – Management and consistency. Coordinating work scales in complexity across the number of repos involved.
- Technical – Repos are created solely considering the role they serve in hosting the content placed there.

The administrative category is largely illusory and caters more to GitHub administrators rather than developers.
Logistics are a concern, particularly early in code development. Fortunately, in Rust, crate development can occur
mostly independent of repository assignment. Therefore, all of the software development principles previously discussed
can be applied to crate and module separation and then repo creation fall into place when the code is ready to be
shared with a wider audience. As a public project, the technical reasons will ultimately serve the long-term interests
of the project best. Allowing code to be managed at the proper scope for all its contributors and users.

Also consider that the repo defines focus for its content and purpose - the revision history, versioning, documentation,
crate-specific workflows, maintainers, policies, etc. In some cases, it might make sense for a crate to share a repo
with other crates. That might be the case if what binds the two crates being in a single repo is beneficial to their
management – a single revision history, a single repo version, a single repo issue list, and so on.

Note that this section is not meant to be prescriptive about how to manage repos. The guidance is to consider the
various factors for repo assignment and then make the best decision for the long term interests of the project.

## Code Organization Guidelines

These guidelines consider the placement and organization of code to support long-term maintenance and usability. In
addition, the goal is to employ the software principles described in the previous section to publish crates to the
wider community.

Note: These categories are an initial proposal based on code trends that have developed over time in our work and
subject to change based on review.

- DXE Core Specific Crate (`uefi-dxe-core` repository)
  - Independent functionality exclusively used by the DXE Core.
  - Examples: DXE Core, Event infrastructure, GCD, memory allocator
- Core Specific Crate (`uefi-core` repository)
  - Code that is specific to creating core environments, like the PEI, DXE, and MM  environments.
  - Examples: Protocol DB, Common PE/COFF (goblin) wrappers, common section extraction code
- Module Development (SDK) Crate (`uefi-sdk` repository)
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

This document does not intend to define exact mappings of current code to crates, that is out of scope. Its goal is to
define the guidelines for managing crates.

### Crate Dependencies

The matrix below shows allowed dependencies for each class of crate defined in the previous section.

|           | DXE Core | Core      | SDK       | Utility   | Feature   | Generic   |
|-----------|----------|-----------|-----------|-----------|-----------|-----------|
| DXE Core  | x        | Y         | Y         | N         | N         | Y         |
| Core      | N        | x         | Y         | N         | N         | Y         |
| SDK       | N        | N         | x         | N         | N         | Y         |
| Utility   | N        | N         | Y         | N         | N         | Y         |
| Feature   | N        | N         | Y         | N         | N         | Y         |
| Generic   | N        | N         | N         | N         | N         | Y         |

Separating out generic code is beneficial because it allows the code to be reused in the greatest number of places
including outside the UEFI environment in host unit tests.
