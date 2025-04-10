# RFC: `Move CPU functionality into Core`

Currently, traits generics are used in the Patina Core's struct defintion to support the ability to (1) replace certain functionality based off of a platform's requirements and (2) replace cpu architecture specific functionality. As time has continued and the core has evolved, we have noted that platforms do not need to customize this functionality; all platforms of a certain architecture will always use the same underlying architecture support code. Exposing this to the consumer only works to complicate the core initialization and has been deemed unecessary.

This proposal is to remove the architecture specific customization from the public core interface, and automatically use the appropriate logic for the given architecture. Configuration knobs can be provided to the core to fine tune this logic for a given platform.

## Change Log

- 2025-04-10: Initial RFC created.

## Motivation

The main motivation of this RFC is to simplify the consumption of the Patina Core to improve ease of use and increase
adoption.

## Technology Background

The three traits, `EfiCpuInit`, `InterruptManager`, and `InterruptBases` are trait generics that provide an interface for initializing many of the low level cpu functionality. This functionality will be the same for each cpu architecture supported, but may have some different configuration knobs for different platforms. [uefi_cpu](https://github.com/OpenDevicePartnership/uefi-core/tree/main/uefi_cpu)
contains the core functionality for all three of these trait interfaces and can be reviewed for specific functionality.

## Goals

1. Remove as many trait generics from the core as possible
2. Allow configuration for cpu initialization


## Requirements

1. remove `.with_cpu_init` method and `EfiCpuInit` trait from the `Core`'s public interface.
2. remove `.with_interrupt_manager` method and `InterruptManager` trait from the `Core`'s public interface.
3. remove `.with_interrupt_bases` method and `InterruptBases` trait from the `Core`'s public interface.
4. Automatically select and use the given `EfiCpuInit`, `InterruptManager`, and `InterruptBases` code based off the
   compilation target architecture.
5. Provide generic configuration knob support for platforms to fine-tune these initializations where necessary.

## Unresolved Questions

- A proper way to add config knob support for the initialization of these
- Do we want to expand the interface to provide a generic pre-mem initialization routine that anyone can call, but we
  have a few hardcoded initializers for cpu initialization?

## Prior Art (Existing PI C Implementation)

In the current design, each of the three implementations must be registered with the core using the appropriate
`.with_*` method, which allows for the registration of a configured initializer

## Alternatives

Switch to a standardized struct instead of trait generics, for initialization.

## Rust Code Design

Update the Patina Core interface to:

```rust

pub struct Core<SectionExtractor, MemoryState>
where
    SectionExtractor: fw_fs::SectionExtrator + Default + Copy + 'static
{
    section_extractor: SectionExtractor,
    components: Vec<Box<dyn Component>>,
    storage: Storage,
    _memory_state: core::marker::PhantomData<MemoryState>    
}

impl<SectionExtractor> Core<SectionExtractor, NoAlloc>
where
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static
{
    #[cfg(all(arch = "x64", target_os = "uefi"))]
    fn cpu_init(&self) -> Result<(), EfiError> {
        EfiCpuInitX64::default().initialize()?;
        InterruptManagerX64::default().initialize()?;
    }

    #[cfg(all(arch = "aarch64", target_os = "uefi"))]
    fn cpu_init(&self) -> Result<(), EfiError> {
        // These `default` implementation is where we probably need to add configuration support
        EfiCpuInitAArch64::default().initialize()?;
        InterruptManagerAarch64::default().initialize()?;
        InterruptBasesAArch64::default().initialize()?;
    }

    pub fn init_memory(
        mut self,
        physical_hob_list: *const c_void,
    ) -> Core<SectionExtractor, Alloc> {
        #[cfg(target_os = "uefi")]
        self.cpu_init()

        /* Continue as normal */

        #[cfg(all(target_os = "uefi", target_arch = "aarch64"))]
        hw_interrupt_protocol::install_hw_interrupt_protocol(&mut InterruptManagerAarch64::default(), &self.interrupt_bases);

        /* Continue as normal */
    }
}

```

## Guide-Level Explanation

N/A
