# RFC: `Move CPU functionality into Patina Core`

The struct(s) for configuring CPU specific functionality are currently exposed external to the uefi-dxe-core via the
`.with_cpu_init` and `.with_interrupt_manager` methods in the `Core` object to support the ability to (1) replace
certain functionality based off of a platform's requirements and (2) replace cpu architecture specific functionality.
As the Patina Core has evolved, we have noted that platforms do not need to customize this functionality; all platforms
of a certain architecture will always use the same underlying architecture support code. Exposing this configuration to
the consumer only works to complicate the Patina Core initialization and has been deemed unnecessary.

This proposal is to remove the architecture specific customization from the public Patina Core interface, and
automatically use the appropriate logic for the given architecture. Configuration knobs can be provided to the Patina
Core to fine tune this logic for a given platform.

## Change Log

- 2025-04-10: Initial RFC created.
- 2025-04-25: General update after a commit that removed some of the generics

## Motivation

The main motivation of this RFC is to simplify the consumption of the Patina Core to improve ease of use and increase
adoption. By allowing the platform to pass in a struct that is always the same based off the CPU architecture, it
increases the chance of compilation errors due to crate version mismatches.

## Technology Background

The two traits, `Cpu` and `InterruptManager` are trait generics that provide an interface for initializing and
utilizing the low level cpu functionality. This functionality has been noted to always be the same for each cpu
architecture supported, but may have some different configuration knobs for different platforms. [uefi_cpu](https://github.com/OpenDevicePartnership/uefi-core/tree/main/uefi_cpu)
contains the functionality for all three of these trait interfaces and can be reviewed for specific functionality.

## Goals

1. Remove trait generic consumption from the `Core` interface and only expose config knobs where necessary
2. Allow configuration for cpu initialization

## Requirements

1. remove `.with_cpu_init` method and `EfiCpuInit` trait from the `Core`'s public interface.
2. remove `.with_interrupt_manager` method and `InterruptManager` trait from the `Core`'s public interface.
3. update `.with_interrupt_bases` method to consume gicd_base and gicr_base directly instead of through a trait
4. Expose `Cpu` trait as a service (`Service<dyn Cpu>`) which has the interface `flush_data_cache`, `init`,
   `get_timer_value`
5. Expose `InterruptManager` trait as a service (`Service<dyn InterruptManager>`) which has the interface
   `register_exception_handler` and `unregister_exception_handler`.
. Update cpu_arch protocol and hw_interrupt protocol to use Services instead of references to the trait object.

## Unresolved Questions

1. Do we want to update the `Cpu` or `InterruptManager` trait interfaces?
2. Do we want to move the `Cpu` or `InterruptManager` traits to another location (uefi-sdk)?

## Prior Art (Existing PI C Implementation)

In the current design, each of the three implementations must be registered with the Patina Core using the appropriate
`.with_*` method, which allows for the registration of a configured initializer

## Alternatives

Switch to a standardized struct instead of trait generics, for initialization.

## Rust Code Design

Before / After example

### Before Example

```rust
pub struct Core<CpuInit, SectionExtractor, InterruptManager, InterruptBases, MemoryState>
where
    CpuInit: uefi_cpu::cpu::EfiCpuInit + Default + 'static,
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
    InterruptManager: uefi_cpu::interrupts::InterruptManager + Default + Copy + 'static,
    InterruptBases: uefi_cpu::interrupts::InterruptBases + Default + Copy + 'static,
{
    cpu_init: CpuInit,
    section_extractor: SectionExtractor,
    interrupt_manager: InterruptManager,
    interrupt_bases: InterruptBases,
    components: Vec<Box<dyn Component>>,
    storage: Storage,
    _memory_state: core::marker::PhantomData<MemoryState>,
}

impl<CpuInit, SectionExtractor, InterruptManager, InterruptBases>
    Core<CpuInit, SectionExtractor, InterruptManager, InterruptBases, NoAlloc>
where
    CpuInit: uefi_cpu::cpu::EfiCpuInit + Default + 'static,
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
    InterruptManager: uefi_cpu::interrupts::InterruptManager + Default + Copy + 'static,
    InterruptBases: uefi_cpu::interrupts::InterruptBases + Default + Copy + 'static,
{
    /// Registers the CPU Init with it's own configuration.
    pub fn with_cpu_init(mut self, cpu_init: CpuInit) -> Self {
        self.cpu_init = cpu_init;
        self
    }

    /// Registers the Interrupt Manager with it's own configuration.
    pub fn with_interrupt_manager(mut self, interrupt_manager: InterruptManager) -> Self {
        self.interrupt_manager = interrupt_manager;
        self
    }

    /// Registers the section extractor with it's own configuration.
    pub fn with_section_extractor(mut self, section_extractor: SectionExtractor) -> Self {
        self.section_extractor = section_extractor;
        self
    }

    pub fn init_memory(
        mut self,
        physical_hob_list: *const c_void,
    ) -> Core<CpuInit, SectionExtractor, InterruptManager, InterruptBases, Alloc> {
        let _ = self.cpu_init.initialize();
        self.interrupt_manager.initialize().expect("Failed to initialize interrupt manager!");

        /* Continue as normal */

    }
}

// Platform integration step:
Core::default()
    .with_section_exctractor(...)
    .with_cpu_init(...)
    .with_interrupt_manager(...)
    .with_interrupt_bases(...)
    .init_memory(physical_hob_list)
    .start()
    .unwrap();
```

### After Example

```rust
// After
pub struct Core<SectionExtractor, MemoryState>
where
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static
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
    #[cfg(all(target_os = "uefi", target_arch = "aarch64"))]
    pub fn with_interrupt_bases(self, gicd_base: u64, gicr_base: u64) -> Self {
        self.interrupt_bases = (gicd_base, gicr_base)
        self
    }

    pub fn init_memory(
        mut self,
        physical_hob_list: *const c_void,
    ) -> Core<SectionExtractor, Alloc> {
        let mut cpu = Cpu::default();
        cpu.initialize().unwrap();
        let mut im = InteruptManager::default();
        im.initialize().unwrap();

        /* Continue as normal */

        storage.add_service(cpu);
        storage.add_service(im);
        let cpu: Service<dyn Cpu> = storage.get_service().unwrap();
        let im: Service<dyn InterruptManager> = storage.get_service.unwrap();

        cpu_arch_protocol::install_cpu_arch_protocol(cpu, im);

        #[cfg(all(target_os = "uefi", target_arch = "aarch64"))]
        hw_interrupt_protocol::install_hw_interrupt_protocol(im, &self.interrupt_bases);

        /* Continue as normal */
    }
}

// Platform integration step:
Core::default()
    .with_section_exctractor(...)
    .init_memory(physical_hob_list)
    .start()
    .unwrap();

```

## Guide-Level Explanation

N/A
