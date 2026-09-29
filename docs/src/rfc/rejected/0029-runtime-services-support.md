# RFC: Runtime Services Support

This RFC proposes a roadmap for Runtime Services and runtime module support for the
Patina project. Runtime modules and services must exist in separate memory regions
from the monolithic Patina core and so present problems for using the existing core
and infrastructure to support. This RFC proposes that a new runtime module be spun
up from the existing Rust code and to defer other future support. This RFC leaves open
the possibility that this module be extended in the future.

## Change Log

- 10-14-25: Initial RFC created.
- 6-23-26: Moving to rejected.

## Motivation

Runtime services present a unique challenge for the monolithic binary model that
Patina uses. Runtime services must exist after boot services have been unloaded,
and so it is not possible for the runtime services to exist as part of the
monolithic binary, at least without very specialized handling. Runtime services
must have a focused roadmap to align with the goals and development models of
Patina to be clear about expected investments.

## Technology Background

Currently the EDKII implementation dispatches runtime modules as part of DXE phase,
most notably RuntimeDxe which provides core services such as `SetVirtualAddressMap`
and other modules to implement more platform specific runtime services. The original
implementation on Patina provided an implementation of `SetVirtualAddressMap` within
the monolithic core, but this results in some problems on Windows boot as we will
leverage boot services memory in `SetVirtualAddressMap` which is a runtime service
and executes after ExitBootServices. Generally, this makes assumptions about OS
behavior which may not be sustainable. These were resolved in the following PRs.

- [dxe_core: Move runtime event handling into runtime module](https://github.com/OpenDevicePartnership/patina/pull/738)
- [patina_dxe_core: Switch to use external RuntimeDxe implementation](https://github.com/OpenDevicePartnership/patina/pull/769)

The approach that has been taken in the interim is to remove the Patina
implementation for now and to ask platforms to go back to the EDKII RuntimeDxe.

## Goals

1. Set short term support plan for runtime services/modules.
2. Enumerate expansion plans for runtime services in the future.

## Requirements

1. Runtime services must continue to operate as expected for short term.
2. Runtime modules must exist purely in runtime service memory.
3. Long term goals should remove dependence on EDKII runtime code.

## Unresolved Questions

- Is there a suitable type of dynamic linking that may make other options suitable
  without excessive one-off infrastructure?

## Prior Art (Existing PI C Implementation)

The current C implementation consists of one primary runtime module, usually
RuntimeDxe and a variable amount of additional runtime modules. These modules are
dispatched by DXE, but will be loaded in runtime memory. The primary runtime module
should produce the runtime architecture protocol, defined in the PI spec section
12.8. This protocol allows for the core to fill in critical information for the
runtime module to implement SetVirtualAddressMap. This includes the image list of
runtime modules, events registered for virtualizing memory, and memory map
information that is not currently used. When `SetVirtualAddressMap` is called, the
runtime DXE module will use the information from the protocol it published and the
core filled to re-relocate the images, virtualizing their execution, and call all
events registered to allow drivers/libraries to virtualize any stored pointers.

## Alternatives

### Create new interface for runtime architectural information

Instead of using runtime architectural protocol for the chosen driver model, a new
protocol or interface could be invented that would allow safer parsing. For example,
for the event and image lists, these could be instead maintained in a vector and
the slices provided through a protocol. This would prevent the need for unsafe logic
in walking a raw pointer based linked list. However, this would be creating a new
set of rust-to-rust interfaces that do not exist within the monolithically compiled
binaries. This both complicates the interoperability story with existing code, and
will only be applicable to this one off scenario for the time being.

### Dynamic Module Linking

An alternative to leveraging the protocol communication would be to allow dynamic
linking and direct invocation of the runtime module. This would prevent the need
for secondary structures and callbacks. However, this raises many questions and
potential toolchain problems such as support between GCC and MSVC tool chains for
dynamic linking. This would also invent additional infrastructure needed only for
runtime services.

### Runtime Service Removal

The long term goal of runtime services, instead of replacing them, should be to
remove them. Most runtime services are superfluous on modern machines. The only
runtime service that is typically invoked at runtime that cannot be replaced with
other existing mechanisms is variable services. Even so, variable services are nearly
always a simple wrapper around calls to the implementation executing in either SMM
or TrustZone. An alternative to re-implementing runtime services for Patina, would
be to simply not support runtime services at runtime, but instead provide a
specification defined interface to access variable services directly through SMM
and TrustZone.

However, this is not feasible in the short term. Any solution here would require
updates both to the UEFI specifications to support them as well as specification
of the new SMM/TrustZone interfaces, not to mention OS support. While removal of
runtime services is a desirable long term goal it does not satisfy the immediate
goals of this RFC.

### Defer Runtime Module Support

Another solution is to defer implementation of any Rust runtime module until DXE
and MM are solved and a more clear interfacing model appears, and continue to use
the EDKII implementation of RuntimeDxe in the interim. The EDKII implementation
would either be replaced by a more robust implementation of a Patina runtime or
removed in the future if runtime services are no longer required in the future.
However, as explained above a more robust solution in this space is filled with
complications and is unlikely to occur in the near future. Additionally, runtime
service removal will be a long process of standardization and OS support. Deferring
until a more complete solution may not be feasible.

## Rust Code Design

Patina should continue to use separate runtime modules which interface using the
UEFI and PI defined interfaces. This includes the system architectural protocol for
resources from the core as well as boot services for allocations and other DXE
phase calls.

However, as a stepping stone to replacement RuntimeDxe should be implementation of a
Rust driver leveraging the original code removed in the above PRs. This driver would
be implemented in the Rust driver infrastructure developed for EDKII and located in
the PatinaPkg. This module would use all existing UEFI/PI interfaces to access boot
services and communicate runtime image and event data. The responsibilities of each
module in this model are detailed below.

### Patina DXE Core

The Patina DXE core will continue to listen for the registration of the runtime
architectural protocol, and register event and image data to the protocol. This
will be updated whenever a new runtime image is loaded or a new `SetVirtualAddressMap`
callback is registered.

### Patina Runtime (new)

A new module compiled as a Rust driver in an EDKII environment, provides a
compatible implementation as RuntimeDxe to implement core functions, i.e.
`SetVirtualAddressMap` & `ConvertPointer`. This driver will install the runtime
architectural protocol. On `SetVirtualAddressMap` the module will be responsible for
re-relocating images and invoking all appropriate events filled in the
architectural protocol.

### Refactoring Relocation Logic (optional)

The logic necessary to relocate the PE is currently split across multiple modules
in patina_dxe_core. To reduce code duplication, it may be desirable to refactor the
PE loader logic into a standalone crate, e.g. `patina_loader`. Without this,
relocation and PE parsing logic may need to be duplicated.

## Guide-Level Explanation

Does not change any consumable rust APIs.

## Rejection Summary

This RFC has been moved to the rejected state as Patina is likely not the proper place to sort out the real solutions
here. The underlying option of removing runtime services as a concept in favor of direct variable access must be addressed
in the specification community. This work is not at a point of clarity where a Patina specific proposal is appropriate.
