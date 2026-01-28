# RFC: `Move CPU functionality into Patina Core`

// TODO: SHERRY make copilot write this

## Change Log

- 2026-01-27: Initial RFC created.

## Motivation

The current Patina implementation of the Device Path protocol is disorganized. Issues include:
- Definitions of device path types across multiple crates (`fv.rs`, `measurement.rs`, etc.)
- Parsing functionality split across multiple crates (e.g. `patina_internal_device_path`, `uefi_protocol`)
- The usage of crates labeled `internal` by external crates (e.g. `patina_performance` relies on `patina_internal_device_path`)

This RFC presents a strategy for integrating all Device-Path-related functionality into a single crate as part of the Patina SDK.

## Technology Background

For basic information on the Device Path Protocol, see the [UEFI Spec Vol 10](https://uefi.org/specs/UEFI/2.10/10_Protocols_Device_Path_Protocol.html). This RFC focuses more on its organization within Patina than specifics of Deivce Path functionality.

The Patina SDK provides shared primitives used in the rest of the Patina core. For more information, see the [README](/sdk/patina/README.md). As many core implementations across several crates depend on Device Path functionality, `sdk` is the best consolidated location to place these struct definitions and parsing functions.

## Goals

1. Consolidate all Device Path functionality into the Patina SDK crate
2. Clearly delineate internal vs. external functionality within the SDK


## Requirements

1. Organize all Device Path struct definitions into a `device_path` module inside `sdk`
2. Organize all Device Path parsing functions into a `device_path` module inside `sdk`
3. Preserve internal `core` Device Path functionality, ensuring it is not used outside `core`
4. Move externally used Device Path structs and functions into the `device_path` module into `sdk`

## Unresolved Questions

// TODO_SHERRY

## Prior Art (Existing PI C Implementation)

Currently, Device Path functionality and structs are split across multiple crates. 

The following crates contain Device Path struct definitions:
- `patina_dxe_core`: `fv.rs`
- `sdk/patina_performance`: `measurement.rs`
- `patina_internal_device_path`

The following crates contain Device Path parsing functionality:
- `patina_internal_device_path`
- `sdk/uefi_protocol`
- `patina_dxe_core/image.rs`

## Alternatives

Switch to a standardized struct instead of trait generics, for initialization.

## Rust Code Design

Before / After example

### Before Example

```rust
pub struct Core<SectionExtractor, MemoryState>
where
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
{
    cpu_init: EfiCpu,
    section_extractor: SectionExtractor,
    interrupt_manager: InterruptManager,
    interrupt_bases: Interrupts,
    components: Vec<Box<dyn Component>>,
    storage: Storage,
    _memory_state: core::marker::PhantomData<MemoryState>,
}

impl<SectionExtractor> Core<SectionExtractor, NoAlloc>
where
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
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

pub struct GicBases(u64, u64);

impl GicBases {
    pub fn new(gicd_base: u64, gicr_base) -> Self {
        GicBases(gicd_base, gicr_base)
    }
}

impl Default for GicBases {
    fn default() -> Self {
        panic!("GicBases `Config` must be provided directly to the core with `.with_config(...)`.")
    }
}

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

        /* immediately before `systemtables::init_system_table, return from init_memory */
        Core { ... }
    }
}

impl<SectionExtractor> Core<SectionExtractor, Alloc>
where
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static
{
    fn initialize_system_table(&self) -> Result<()> {

        let cpu: Service<dyn Cpu> = storage.get_service().unwrap();
        let im: Service<dyn InterruptManager> = storage.get_service.unwrap();

        /* Continue from `systemtables::init_system_table();` */

        cpu_arch_protocol::install_cpu_arch_protocol(cpu, im);

        #[cfg(all(target_os = "uefi", target_arch = "aarch64"))]
        hw_interrupt_protocol::install_hw_interrupt_protocol(im, self.storage.get_config().unwrap());

        /* Continue */
        Ok(())
    }

    fn start(mut self) -> Result<()> {
        log::info!("Initiliazing System Table");
        self.initialize_system_table()?;
        log::info!("System Table Initialized");
    }
}

// Platform integration step:
Core::default()
    .with_section_exctractor(...)
    .init_memory(physical_hob_list)
    .with_config(GicBases::new(0x40060000, 0x40080000))
    .start()
    .unwrap();

```

## Guide-Level Explanation

N/A
