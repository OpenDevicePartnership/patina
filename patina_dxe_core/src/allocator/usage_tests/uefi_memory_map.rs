//! Usage tests for the UEFI memory map.
//!
//! These tests validate that `get_memory_map()` returns the correct memory
//! descriptors based on various constructed memory scenarios.
//!
//! ## Overview
//!
//! Tests are built using the `MemoryMapTestScenario` to:
//!
//! 1. Configure HOB list with resource descriptors and memory allocations
//! 2. Initialize the memory manager with the test scenario
//! 3. Call `get_memory_map()` similar to an OS
//! 4. Validate that the returned descriptors match expected results
//!
//! The tests provide debug output to make it easy to see memory attributes returned in the
//! memory map in different scenarios. This can also be useful to quickly see the impact of
//! changes in patina code to the UEFI memory map.
//!
//! ## Logging
//!
//! The `memory_map_test` log target is used for all logging.
//!
//! To enable logging, set the `RUST_LOG` environment variable to the desired
//! log level (e.g., `debug`, `info`, `warn`, `error`) before running the tests.
//!
//! For example:
//!
//! ```sh
//! RUST_LOG=info cargo test --lib -p patina_dxe_core allocator::usage_tests::uefi_memory_map -- --nocapture
//! ```
//!
//! PowerShell example:
//!
//! ```powershell
//! $env:RUST_LOG="info"; cargo test --lib -p patina_dxe_core allocator::usage_tests::uefi_memory_map -- --nocapture
//! ```
//!
//! ## Considerations for Setting up Memory
//!
//! ### Free Memory Range
//!
//! The PHIT HOB's `free_memory_top` field defines the upper bound of system memory that
//! will be added to the GCD during initialization. This is important for correct test setup:
//!
//! - **System Memory Only**: `free_memory_top` should only cover system memory regions.
//!   Regions like MMIO need to be separate from system memory regions.
//! - **Automatic Free Memory Top Calculation**: The test framework automatically calculates
//!   `free_memory_top` as the end of the highest system memory resource descriptor.
//! - **GCD Initialization Order**: `init_gcd()` adds the free memory range as system memory
//!   before processing resource descriptor HOBs.
//!
//! ## Example: Using MemoryMapTestScenario
//!
//! ```rust
//! use patina::guids::ZERO;
//! use patina::pi::hob;
//! use r_efi::efi;
//!
//! let scenario = MemoryMapTestScenario::new("Test Name", 64 * 1024 * 1024) // 64MB
//!     .with_resource_descriptor(ResourceDescriptorConfig {
//!         resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
//!         resource_attribute: hob::TESTED_MEMORY_ATTRIBUTES as u32,
//!         physical_start: 0x100000,
//!         resource_length: 32 * 1024 * 1024,
//!         owner: ZERO,
//!     })
//!     .with_memory_allocation(MemoryAllocationConfig {
//!         memory_type: efi::BOOT_SERVICES_DATA,
//!         memory_base_address: 0x200000,
//!         memory_length: 4096,
//!         name: ZERO,
//!     })
//!     .with_validation(|descriptors| {
//!         if descriptors.is_empty() {
//!             return Err("No descriptors returned".to_string());
//!         }
//!         Ok(())
//!     });
//!
//! scenario.run_test();
//! ```
//!
//! ## Example: Using MemoryMapValidation
//!
//! The `MemoryMapValidation` helper provides structured validation for memory map tests:
//!
//! ```rust
//! use crate::allocator::memory_map_integration_tests::{MemoryMapTestScenario, MemoryMapValidation};
//! use r_efi::efi;
//!
//! // Set up test scenario
//! let scenario = MemoryMapTestScenario::new("Runtime Services Test", 256 * 1024 * 1024)
//!     .with_resource_descriptor(ResourceDescriptorConfig {
//!         resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
//!         resource_attribute: hob::TESTED_MEMORY_ATTRIBUTES as u32,
//!         physical_start: 0x100000,
//!         resource_length: 64 * 1024 * 1024,
//!         owner: ZERO,
//!     })
//!     .with_memory_allocation(MemoryAllocationConfig {
//!         memory_type: efi::RUNTIME_SERVICES_CODE,
//!         memory_base_address: 0x200000,
//!         memory_length: 1 * 1024 * 1024,
//!         name: ZERO,
//!     })
//!     .with_memory_allocation(MemoryAllocationConfig {
//!         memory_type: efi::RUNTIME_SERVICES_DATA,
//!         memory_base_address: 0x300000,
//!         memory_length: 2 * 1024 * 1024,
//!         name: ZERO,
//!     })
//!     .with_validation(|descriptors| {
//!         // Validate the memory map with structured expectations
//!         MemoryMapValidation::new()
//!             .expect_min_descriptors(5)
//!             .expect_memory_types(vec![
//!                 efi::CONVENTIONAL_MEMORY,
//!                 efi::RUNTIME_SERVICES_CODE,
//!                 efi::RUNTIME_SERVICES_DATA,
//!             ])
//!             .expect_runtime_memory_mb(3) // 1 MB code + 2 MB data
//!             .with_custom_validation(|descs| {
//!                 // Custom validation logic
//!                 if !descs.iter().any(|d| d.r#type == efi::RUNTIME_SERVICES_CODE) {
//!                     return Err("Must have runtime services code".to_string());
//!                 }
//!                 Ok(())
//!             })
//!             .validate(descriptors)
//!     });
//!
//! scenario.run_test();
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#[cfg(test)]
mod tests {
    use crate::{
        allocator::{get_memory_map, init_memory_support, reset_allocators},
        test_support,
    };
    use alloc::vec::Vec;
    use patina::{
        base::*,
        guids::ZERO,
        pi::{
            BootMode,
            hob::{self, HobList, PhaseHandoffInformationTable, ResourceDescriptor, header},
        },
    };
    use r_efi::efi;
    use serial_test::serial;
    use std::panic::RefUnwindSafe;

    // Logger initialization
    fn init_logger() {
        use std::sync::OnceLock;
        static INIT: OnceLock<()> = OnceLock::new();

        INIT.get_or_init(|| {
            // Default to no logging unless RUST_LOG environment variable is set
            let mut builder = env_logger::Builder::from_default_env();

            // If RUST_LOG is not set, default to Off (no logging)
            if std::env::var("RUST_LOG").is_err() {
                builder.filter_level(log::LevelFilter::Off);
            }

            builder.init();
        });
    }

    /// HOB configuration structures to simplify building test scenarios.
    ///
    /// These simplified configuration structs provide a convenient test API without requiring
    /// manual HOB header construction. They are converted to full HOB structures (from
    /// `patina::pi::hob`) during HOB list building in `build_custom_hob_list()`, where the
    /// appropriate HOB headers are automatically added.
    mod hob_config {
        use r_efi::efi;

        /// Configuration for a resource descriptor HOB (becomes `patina::pi::hob::ResourceDescriptor`)
        #[derive(Clone, Debug)]
        pub struct ResourceDescriptorConfig {
            pub resource_type: u32,
            pub resource_attribute: u32,
            pub physical_start: u64,
            pub resource_length: u64,
            pub owner: efi::Guid,
        }

        /// Configuration for a memory allocation HOB (becomes `patina::pi::hob::MemoryAllocation`)
        #[derive(Clone, Debug)]
        pub struct MemoryAllocationConfig {
            pub memory_type: u32,
            pub memory_base_address: u64,
            pub memory_length: u64,
            pub name: efi::Guid,
        }
    }

    use hob_config::{MemoryAllocationConfig, ResourceDescriptorConfig};

    /// Type alias for memory map validation functions
    type ValidationFn = Box<dyn Fn(&[efi::MemoryDescriptor]) -> Result<(), String> + RefUnwindSafe>;

    /// Used to construct custom memory scenarios in tests
    pub struct MemoryMapTestScenario {
        name: String,
        memory_size: u64,
        resource_descriptors: Vec<ResourceDescriptorConfig>,
        memory_allocations: Vec<MemoryAllocationConfig>,
        validations: Vec<ValidationFn>,
    }

    /// Validation expectations that can be checked against the memory map returned by get_memory_map()
    pub struct MemoryMapValidation {
        pub total_memory_mb: Option<u64>,
        pub expected_types: Option<Vec<u32>>,
        pub min_descriptors: Option<usize>,
        pub max_descriptors: Option<usize>,
        pub conventional_memory_mb: Option<u64>,
        pub runtime_memory_mb: Option<u64>,
        custom_validations: Vec<ValidationFn>,
    }

    impl MemoryMapTestScenario {
        fn call_get_memory_map(&self) -> Result<Vec<efi::MemoryDescriptor>, String> {
            use core::{mem, ptr};

            let mut memory_map_size: usize = 0;
            let mut map_key: usize = 0;
            let mut descriptor_size: usize = 0;
            let mut descriptor_version: u32 = 0;

            let status = get_memory_map(
                ptr::from_mut(&mut memory_map_size),
                ptr::null_mut(),
                ptr::from_mut(&mut map_key),
                ptr::from_mut(&mut descriptor_size),
                ptr::from_mut(&mut descriptor_version),
            );

            if status != efi::Status::BUFFER_TOO_SMALL {
                return Err(format!("Expected BUFFER_TOO_SMALL, got {:?}", status));
            }

            let descriptor_count = memory_map_size / mem::size_of::<efi::MemoryDescriptor>();
            let mut descriptors: Vec<mem::MaybeUninit<efi::MemoryDescriptor>> = Vec::with_capacity(descriptor_count);

            // SAFETY: Capacity was reserved for `descriptor_count` elements and the length below matches that.
            unsafe { descriptors.set_len(descriptor_count) };

            let status = get_memory_map(
                ptr::from_mut(&mut memory_map_size),
                descriptors.as_mut_ptr().cast(),
                ptr::from_mut(&mut map_key),
                ptr::from_mut(&mut descriptor_size),
                ptr::from_mut(&mut descriptor_version),
            );

            if status != efi::Status::SUCCESS {
                return Err(format!("get_memory_map() failed: {:?}", status));
            }

            if descriptor_size != mem::size_of::<efi::MemoryDescriptor>() {
                return Err(format!("Unexpected descriptor size: {}", descriptor_size));
            }
            if descriptor_version != efi::MEMORY_DESCRIPTOR_VERSION {
                return Err(format!("Unexpected descriptor version: {}", descriptor_version));
            }

            let actual_descriptor_count = memory_map_size / descriptor_size;
            descriptors.truncate(actual_descriptor_count);

            // SAFETY: get_memory_map() has successfully initialized all descriptors up to
            // `actual_descriptor_count`. The transmute from Vec<MaybeUninit<T>> to Vec<T> is used
            // because MaybeUninit<T> and T have the same layout, and all elements are now initialized.
            let descriptors = unsafe {
                mem::transmute::<Vec<mem::MaybeUninit<efi::MemoryDescriptor>>, Vec<efi::MemoryDescriptor>>(descriptors)
            };

            Ok(descriptors)
        }

        /// Create a new test scenario with the given name
        pub fn new(name: &str, memory_size: u64) -> Self {
            Self {
                name: name.to_string(),
                memory_size,
                resource_descriptors: Vec::new(),
                memory_allocations: Vec::new(),
                validations: Vec::new(),
            }
        }

        /// Add a resource descriptor to the scenario
        pub fn with_resource_descriptor(mut self, config: ResourceDescriptorConfig) -> Self {
            self.resource_descriptors.push(config);
            self
        }

        /// Add a memory allocation to the scenario
        pub fn with_memory_allocation(mut self, config: MemoryAllocationConfig) -> Self {
            self.memory_allocations.push(config);
            self
        }

        /// Add a validation function to check the resulting memory map
        pub fn with_validation<F>(mut self, validation: F) -> Self
        where
            F: Fn(&[efi::MemoryDescriptor]) -> Result<(), String> + RefUnwindSafe + 'static,
        {
            self.validations.push(Box::new(validation));
            self
        }

        /// Execute the test scenario and validate results
        pub fn run_test(&self) {
            test_support::with_global_lock(|| {
                let hob_list_ptr = self.build_custom_hob_list();

                // SAFETY: This is a test environment where the initialization order is controlled per
                // the overall test framework. The hob_list_ptr points to valid memory allocated by build_custom_hob_list().
                // Resetting global state (protocol DB, GCD, allocators) is considered safe in this isolated test context.
                unsafe {
                    test_support::init_test_protocol_db();
                    crate::GCD.reset();
                    crate::gcd::init_gcd(hob_list_ptr);
                    reset_allocators();
                }

                let mut hob_list = HobList::default();
                hob_list.discover_hobs(hob_list_ptr);
                init_memory_support(&hob_list);

                let descriptors = self.call_get_memory_map().expect("Failed to get the UEFI memory map");

                log::info!(target: "memory_map_test", "Found {} descriptors for scenario '{}':", descriptors.len(), self.name);
                for (i, desc) in descriptors.iter().enumerate() {
                    log::info!(
                        target: "memory_map_test",
                        "  [{}] Type: {}, Start: {:#x}, Pages: {}, Attr: {:#x}",
                        i, desc.r#type, desc.physical_start, desc.number_of_pages, desc.attribute
                    );
                }

                for validation in &self.validations {
                    if let Err(e) = validation(&descriptors) {
                        panic!("Validation failed for scenario '{}': {}", self.name, e);
                    }
                }

                log::info!(target: "memory_map_test", "✅ Test scenario '{}' passed with {} descriptors", self.name, descriptors.len());
            })
            .unwrap_or_else(|_| panic!("Test '{}' panicked during execution", self.name));
        }

        /// Build a custom HOB list based on the scenario configuration
        fn build_custom_hob_list(&self) -> *const core::ffi::c_void {
            // SAFETY: get_memory() allocates test memory that remains valid for the duration of the test.
            // The returned slice should be properly aligned and sized to the memory_size argument given.
            let mem = unsafe { test_support::get_memory(self.memory_size as usize) };
            let mem_base = mem.as_mut_ptr() as u64;

            // Calculate the total HOB list size
            let hob_size = core::mem::size_of::<PhaseHandoffInformationTable>()
                + core::mem::size_of::<hob::Cpu>()
                + core::mem::size_of::<hob::MemoryAllocation>()
                + self.resource_descriptors.len() * core::mem::size_of::<ResourceDescriptor>()
                + self.memory_allocations.len() * core::mem::size_of::<hob::MemoryAllocation>()
                + core::mem::size_of::<header::Hob>();

            // Calculate free_memory_top as the end of the highest system memory region
            let free_memory_top = self
                .resource_descriptors
                .iter()
                .filter(|rd| rd.resource_type == hob::EFI_RESOURCE_SYSTEM_MEMORY)
                .map(|rd| rd.physical_start + rd.resource_length)
                .max()
                .unwrap_or(self.memory_size);

            let phit = PhaseHandoffInformationTable {
                header: header::Hob {
                    r#type: hob::HANDOFF,
                    length: core::mem::size_of::<PhaseHandoffInformationTable>() as u16,
                    reserved: 0,
                },
                version: 0x0009,
                boot_mode: BootMode::BootAssumingNoConfigurationChanges,
                memory_top: mem_base + self.memory_size,
                memory_bottom: mem_base,
                free_memory_top: mem_base + free_memory_top,
                free_memory_bottom: mem_base + hob_size as u64,
                end_of_hob_list: mem_base + hob_size as u64 - core::mem::size_of::<header::Hob>() as u64,
            };

            let cpu = hob::Cpu {
                header: header::Hob { r#type: hob::CPU, length: core::mem::size_of::<hob::Cpu>() as u16, reserved: 0 },
                size_of_memory_space: 48,
                size_of_io_space: 16,
                reserved: Default::default(),
            };

            let mut offset = 0;
            // SAFETY: Memory was successfully allocated and the HOB list size calculated
            // in this function. Other structures are allocated locally in this function.
            // copy_nonoverlapping is considered safe because the source and destination do not overlap.
            // In non-test code, this would be broken up to have more granular safety comments.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    core::ptr::addr_of!(phit).cast::<u8>(),
                    mem.as_mut_ptr().add(offset),
                    core::mem::size_of::<PhaseHandoffInformationTable>(),
                );
                offset += core::mem::size_of::<PhaseHandoffInformationTable>();

                core::ptr::copy_nonoverlapping(
                    core::ptr::addr_of!(cpu).cast::<u8>(),
                    mem.as_mut_ptr().add(offset),
                    core::mem::size_of::<hob::Cpu>(),
                );
                offset += core::mem::size_of::<hob::Cpu>();

                let stack_size = SIZE_64KB as u64;
                let stack_base = mem_base + SIZE_1MB as u64;
                let stack_hob = hob::MemoryAllocation {
                    header: header::Hob {
                        r#type: hob::MEMORY_ALLOCATION,
                        length: core::mem::size_of::<hob::MemoryAllocation>() as u16,
                        reserved: 0,
                    },
                    alloc_descriptor: hob::header::MemoryAllocation {
                        name: patina::guids::HOB_MEMORY_ALLOC_STACK,
                        memory_base_address: stack_base,
                        memory_length: stack_size,
                        memory_type: r_efi::efi::BOOT_SERVICES_DATA,
                        reserved: Default::default(),
                    },
                };
                core::ptr::copy_nonoverlapping(
                    core::ptr::addr_of!(stack_hob).cast::<u8>(),
                    mem.as_mut_ptr().add(offset),
                    core::mem::size_of::<hob::MemoryAllocation>(),
                );
                offset += core::mem::size_of::<hob::MemoryAllocation>();

                for resource in &self.resource_descriptors {
                    let resource_hob = ResourceDescriptor {
                        header: header::Hob {
                            r#type: hob::RESOURCE_DESCRIPTOR,
                            length: core::mem::size_of::<ResourceDescriptor>() as u16,
                            reserved: 0,
                        },
                        owner: resource.owner,
                        resource_type: resource.resource_type,
                        resource_attribute: resource.resource_attribute,
                        physical_start: mem_base + resource.physical_start,
                        resource_length: resource.resource_length,
                    };
                    core::ptr::copy_nonoverlapping(
                        core::ptr::addr_of!(resource_hob).cast::<u8>(),
                        mem.as_mut_ptr().add(offset),
                        core::mem::size_of::<ResourceDescriptor>(),
                    );
                    offset += core::mem::size_of::<ResourceDescriptor>();
                }

                for allocation in &self.memory_allocations {
                    let alloc_hob = hob::MemoryAllocation {
                        header: header::Hob {
                            r#type: hob::MEMORY_ALLOCATION,
                            length: core::mem::size_of::<hob::MemoryAllocation>() as u16,
                            reserved: 0,
                        },
                        alloc_descriptor: hob::header::MemoryAllocation {
                            name: allocation.name,
                            memory_base_address: mem_base + allocation.memory_base_address,
                            memory_length: allocation.memory_length,
                            memory_type: allocation.memory_type,
                            reserved: Default::default(),
                        },
                    };
                    core::ptr::copy_nonoverlapping(
                        core::ptr::addr_of!(alloc_hob).cast::<u8>(),
                        mem.as_mut_ptr().add(offset),
                        core::mem::size_of::<hob::MemoryAllocation>(),
                    );
                    offset += core::mem::size_of::<hob::MemoryAllocation>();
                }

                let end_hob = header::Hob {
                    r#type: hob::END_OF_HOB_LIST,
                    length: core::mem::size_of::<header::Hob>() as u16,
                    reserved: 0,
                };
                core::ptr::copy_nonoverlapping(
                    core::ptr::addr_of!(end_hob).cast::<u8>(),
                    mem.as_mut_ptr().add(offset),
                    core::mem::size_of::<header::Hob>(),
                );
            }

            mem.as_ptr() as *const core::ffi::c_void
        }
    }

    impl MemoryMapValidation {
        /// Create a new validation helper
        pub fn new() -> Self {
            Self {
                total_memory_mb: None,
                expected_types: None,
                min_descriptors: None,
                max_descriptors: None,
                conventional_memory_mb: None,
                runtime_memory_mb: None,
                custom_validations: Vec::new(),
            }
        }

        /// Expect a specific total amount of memory in MB
        pub fn expect_total_memory_mb(mut self, mb: u64) -> Self {
            self.total_memory_mb = Some(mb);
            self
        }

        /// Expect specific memory types to be present
        pub fn expect_memory_types(mut self, types: Vec<u32>) -> Self {
            self.expected_types = Some(types);
            self
        }

        /// Expect a minimum number of descriptors
        pub fn expect_min_descriptors(mut self, count: usize) -> Self {
            self.min_descriptors = Some(count);
            self
        }

        /// Expect a maximum number of descriptors
        #[allow(dead_code)]
        pub fn expect_max_descriptors(mut self, count: usize) -> Self {
            self.max_descriptors = Some(count);
            self
        }

        /// Expect a specific amount of runtime memory in MB
        pub fn expect_runtime_memory_mb(mut self, mb: u64) -> Self {
            self.runtime_memory_mb = Some(mb);
            self
        }

        /// Add a custom validation function
        pub fn with_custom_validation<F>(mut self, validation: F) -> Self
        where
            F: Fn(&[efi::MemoryDescriptor]) -> Result<(), String> + RefUnwindSafe + 'static,
        {
            self.custom_validations.push(Box::new(validation));
            self
        }

        /// Validate a memory map against all configured expectations
        pub fn validate(&self, descriptors: &[efi::MemoryDescriptor]) -> Result<(), String> {
            if let Some(min) = self.min_descriptors
                && descriptors.len() < min
            {
                return Err(format!("Expected at least {} descriptors, got {}", min, descriptors.len()));
            }

            if let Some(max) = self.max_descriptors
                && descriptors.len() > max
            {
                return Err(format!("Expected at most {} descriptors, got {}", max, descriptors.len()));
            }

            let total_memory_bytes: u64 = descriptors.iter().map(|d| d.number_of_pages * UEFI_PAGE_SIZE as u64).sum();
            let total_memory_mb = total_memory_bytes / SIZE_1MB as u64;

            let conventional_memory_bytes: u64 = descriptors
                .iter()
                .filter(|d| d.r#type == efi::CONVENTIONAL_MEMORY)
                .map(|d| d.number_of_pages * UEFI_PAGE_SIZE as u64)
                .sum();
            let conventional_memory_mb = conventional_memory_bytes / SIZE_1MB as u64;

            let runtime_memory_bytes: u64 = descriptors
                .iter()
                .filter(|d| (d.attribute & efi::MEMORY_RUNTIME) != 0)
                .map(|d| d.number_of_pages * UEFI_PAGE_SIZE as u64)
                .sum();
            let runtime_memory_mb = runtime_memory_bytes / SIZE_1MB as u64;

            if let Some(expected) = self.total_memory_mb
                && total_memory_mb != expected
            {
                return Err(format!("Expected {} MB total memory, got {} MB", expected, total_memory_mb));
            }

            if let Some(expected) = self.conventional_memory_mb
                && conventional_memory_mb != expected
            {
                return Err(format!("Expected {} MB conventional memory, got {} MB", expected, conventional_memory_mb));
            }

            if let Some(expected) = self.runtime_memory_mb
                && runtime_memory_mb != expected
            {
                return Err(format!("Expected {} MB runtime memory, got {} MB", expected, runtime_memory_mb));
            }

            if let Some(ref expected_types) = self.expected_types {
                let present_types: std::collections::BTreeSet<u32> = descriptors.iter().map(|d| d.r#type).collect();
                for &expected_type in expected_types {
                    if !present_types.contains(&expected_type) {
                        return Err(format!("Expected memory type 0x{:x} not found in memory map", expected_type));
                    }
                }
            }

            for validation in &self.custom_validations {
                validation(descriptors)?;
            }

            Ok(())
        }
    }

    /// Helper function to create a more complex scenario
    fn create_complex_hob_scenario() -> MemoryMapTestScenario {
        MemoryMapTestScenario::new("Complex HOB Scenario", SIZE_128MB as u64)
            // 64MB System Memory region (Tested).
            .with_resource_descriptor(ResourceDescriptorConfig {
                resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
                resource_attribute: hob::TESTED_MEMORY_ATTRIBUTES,
                physical_start: SIZE_1MB as u64,
                resource_length: SIZE_64MB as u64,
                owner: ZERO,
            })
            // Memory Allocation HOBs
            .with_memory_allocation(MemoryAllocationConfig {
                memory_type: efi::BOOT_SERVICES_DATA,
                memory_base_address: SIZE_2MB as u64,
                memory_length: SIZE_512KB as u64,
                name: efi::Guid::from_fields(
                    0x4ED4BF27,
                    0x4092,
                    0x42E9,
                    0x80,
                    0x7D,
                    &[0x52, 0x7B, 0x1D, 0x00, 0xC9, 0xBD],
                ),
            })
            .with_memory_allocation(MemoryAllocationConfig {
                memory_type: efi::BOOT_SERVICES_CODE,
                memory_base_address: (SIZE_1MB * 3) as u64,
                memory_length: SIZE_4KB as u64,
                name: ZERO,
            })
            .with_memory_allocation(MemoryAllocationConfig {
                memory_type: efi::RUNTIME_SERVICES_DATA,
                memory_base_address: SIZE_4MB as u64,
                memory_length: SIZE_4KB as u64,
                name: ZERO,
            })
    }

    #[test]
    #[serial]
    fn test_simple_memory_scenario() {
        init_logger();
        let scenario = MemoryMapTestScenario::new("Simple Memory Test", SIZE_64MB as u64)
            .with_resource_descriptor(ResourceDescriptorConfig {
                resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
                resource_attribute: hob::TESTED_MEMORY_ATTRIBUTES,
                physical_start: SIZE_1MB as u64,
                resource_length: SIZE_32MB as u64,
                owner: ZERO,
            })
            .with_validation(|descriptors| {
                MemoryMapValidation::new()
                    .expect_memory_types(vec![efi::CONVENTIONAL_MEMORY])
                    .expect_total_memory_mb(32)
                    .validate(descriptors)
            });

        scenario.run_test();
    }

    #[test]
    #[serial]
    fn test_mmio_and_memory_scenario() {
        init_logger();
        let scenario = MemoryMapTestScenario::new("MMIO and Memory Test", SIZE_128MB as u64) // 128MB
            .with_resource_descriptor(ResourceDescriptorConfig {
                resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
                resource_attribute: hob::TESTED_MEMORY_ATTRIBUTES,
                physical_start: SIZE_1MB as u64,
                resource_length: SIZE_64MB as u64,
                owner: ZERO,
            })
            .with_resource_descriptor(ResourceDescriptorConfig {
                resource_type: hob::EFI_RESOURCE_MEMORY_MAPPED_IO,
                resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT | hob::EFI_RESOURCE_ATTRIBUTE_INITIALIZED,
                physical_start: (SIZE_1MB * 80) as u64,
                resource_length: SIZE_16MB as u64,
                owner: ZERO,
            })
            .with_memory_allocation(MemoryAllocationConfig {
                memory_type: efi::BOOT_SERVICES_DATA,
                memory_base_address: SIZE_2MB as u64,
                memory_length: SIZE_1MB as u64,
                name: ZERO,
            })
            .with_validation(|descriptors| {
                memory_map::print_details(descriptors);

                MemoryMapValidation::new()
                    .expect_memory_types(vec![efi::BOOT_SERVICES_DATA])
                    .with_custom_validation(|descriptors| {
                        // Verify MMIO does NOT appear (it did not have the runtime attribute set)
                        let mmio_count = descriptors.iter().filter(|d| d.r#type == efi::MEMORY_MAPPED_IO).count();
                        if mmio_count > 0 {
                            return Err("Non-runtime MMIO should not appear in the UEFI memory map".to_string());
                        }

                        log::info!(target: "memory_map_test", "✅ Test passed: Non-runtime MMIO correctly filtered out");
                        Ok(())
                    })
                    .validate(descriptors)
            });

        scenario.run_test();
    }

    #[test]
    #[serial]
    fn test_runtime_services_memory() {
        init_logger();
        let scenario = MemoryMapTestScenario::new("Runtime Services Test", SIZE_256MB as u64)
            .with_resource_descriptor(ResourceDescriptorConfig {
                resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
                resource_attribute: hob::TESTED_MEMORY_ATTRIBUTES,
                physical_start: SIZE_1MB as u64,
                resource_length: SIZE_128MB as u64,
                owner: ZERO,
            })
            .with_memory_allocation(MemoryAllocationConfig {
                memory_type: efi::RUNTIME_SERVICES_DATA,
                memory_base_address: SIZE_2MB as u64,
                memory_length: SIZE_2MB as u64,
                name: ZERO,
            })
            .with_memory_allocation(MemoryAllocationConfig {
                memory_type: efi::RUNTIME_SERVICES_CODE,
                memory_base_address: SIZE_4MB as u64,
                memory_length: SIZE_1MB as u64,
                name: ZERO,
            })
            .with_validation(|descriptors| {
                memory_map::print_details(descriptors);

                MemoryMapValidation::new()
                    .expect_memory_types(vec![efi::RUNTIME_SERVICES_DATA, efi::RUNTIME_SERVICES_CODE])
                    .expect_runtime_memory_mb(3)
                    .with_custom_validation(|_| {
                        log::info!(target: "memory_map_test", "✅ Test passed: RT memory matched expectations");
                        Ok(())
                    })
                    .validate(descriptors)
            });

        scenario.run_test();
    }

    #[test]
    #[serial]
    fn test_complex_real_world_hob_scenario() {
        init_logger();
        let scenario = create_complex_hob_scenario().with_validation(|descriptors| {
            MemoryMapValidation::new()
                .expect_min_descriptors(5)
                .expect_memory_types(vec![efi::CONVENTIONAL_MEMORY, efi::BOOT_SERVICES_DATA])
                .with_custom_validation(|descriptors| {
                    let has_runtime = descriptors.iter().any(|d| (d.attribute & efi::MEMORY_RUNTIME) != 0);
                    if !has_runtime {
                        return Err("No runtime memory found".to_string());
                    }
                    Ok(())
                })
                .validate(descriptors)
        });

        scenario.run_test();
    }

    #[test]
    #[serial]
    fn test_memory_map_descriptor_merging() {
        init_logger();
        let scenario = MemoryMapTestScenario::new("Descriptor Merging Test", SIZE_128MB as u64)
            .with_resource_descriptor(ResourceDescriptorConfig {
                resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
                resource_attribute: hob::TESTED_MEMORY_ATTRIBUTES,
                physical_start: SIZE_1MB as u64,
                resource_length: SIZE_64MB as u64,
                owner: ZERO,
            })
            // Two adjacent allocations of the same type should be merged
            .with_memory_allocation(MemoryAllocationConfig {
                memory_type: efi::BOOT_SERVICES_DATA,
                memory_base_address: SIZE_2MB as u64,
                memory_length: SIZE_4KB as u64,
                name: ZERO,
            })
            .with_memory_allocation(MemoryAllocationConfig {
                memory_type: efi::BOOT_SERVICES_DATA,
                memory_base_address: (SIZE_2MB + SIZE_4KB) as u64,
                memory_length: SIZE_4KB as u64,
                name: ZERO,
            })
            .with_validation(|descriptors| {
                // Verify that the two adjacent 4KB allocations were merged into a single descriptor
                let boot_services_descriptors: Vec<_> =
                    descriptors.iter().filter(|d| d.r#type == efi::BOOT_SERVICES_DATA).collect();

                // Should have merged into a single 8KB descriptor (2 pages)
                // Look for a descriptor with exactly 2 pages (the merged allocation)
                let found_merged = boot_services_descriptors.iter().any(|d| d.number_of_pages == 2);

                if !found_merged {
                    return Err(format!(
                        "Expected adjacent allocations to be merged into 2 pages. Found Boot Services descriptors: {:?}",
                        boot_services_descriptors
                            .iter()
                            .map(|d| (d.physical_start, d.number_of_pages))
                            .collect::<Vec<_>>()
                    ));
                }

                log::info!(target: "memory_map_test", "✅ Test passed: Adjacent allocations successfully merged into 2-page descriptor");
                Ok(())
            });

        scenario.run_test();
    }

    #[test]
    #[serial]
    fn test_zero_length_allocation() {
        init_logger();
        let scenario = MemoryMapTestScenario::new("Zero Length Allocation Test", SIZE_64MB as u64)
            .with_resource_descriptor(ResourceDescriptorConfig {
                resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
                resource_attribute: hob::TESTED_MEMORY_ATTRIBUTES,
                physical_start: SIZE_16MB as u64,
                resource_length: SIZE_32MB as u64,
                owner: ZERO,
            })
            .with_memory_allocation(MemoryAllocationConfig {
                memory_type: efi::BOOT_SERVICES_DATA,
                memory_base_address: SIZE_32MB as u64,
                memory_length: 0,
                name: ZERO,
            })
            .with_validation(|descriptors| {
                // Verify the zero-length allocation was ignored
                let zero_length_found = descriptors.iter().any(|d| d.number_of_pages == 0);
                if zero_length_found {
                    return Err("Zero-length allocation should not appear in memory map".to_string());
                }

                // Verify that at least one conventional memory descriptor is present
                let has_conventional = descriptors.iter().any(|d| d.r#type == efi::CONVENTIONAL_MEMORY);
                if !has_conventional {
                    return Err("Expected at least some conventional memory".to_string());
                }

                Ok(())
            });

        scenario.run_test();
    }

    #[test]
    #[serial]
    fn test_unaligned_memory_allocation() {
        init_logger();
        let scenario = MemoryMapTestScenario::new("Unaligned Allocation Test", SIZE_64MB as u64)
            .with_resource_descriptor(ResourceDescriptorConfig {
                resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
                resource_attribute: hob::TESTED_MEMORY_ATTRIBUTES,
                physical_start: SIZE_16MB as u64,
                resource_length: SIZE_32MB as u64,
                owner: ZERO,
            })
            .with_memory_allocation(MemoryAllocationConfig {
                memory_type: efi::BOOT_SERVICES_DATA,
                memory_base_address: SIZE_32MB as u64 + 1,
                memory_length: SIZE_4KB as u64,
                name: ZERO,
            })
            .with_validation(|descriptors| {
                // Verify that unaligned memory allocations are ignored by the allocator
                let boot_services_descriptors: Vec<_> =
                    descriptors.iter().filter(|d| d.r#type == efi::BOOT_SERVICES_DATA).collect();

                // Check that a descriptor is not contained within or starts near the unaligned address
                // The unaligned address was SIZE_32MB + 1 + mem_base
                // The alignment check should have prevented this allocation from being processed
                let unaligned_relative = SIZE_32MB as u64 + 1;
                let mut found_unaligned_region = false;
                for desc in descriptors.iter() {
                    let desc_start = desc.physical_start;
                    let desc_end = desc_start + (desc.number_of_pages * UEFI_PAGE_SIZE as u64);

                    // Check if the relative unaligned address would fall within any descriptor
                    if desc_start <= unaligned_relative && unaligned_relative < desc_end {
                        // This would mean the unaligned allocation was processed
                        found_unaligned_region = true;
                        break;
                    }

                    // Also check if any descriptor starts exactly at the unaligned offset
                    if (desc_start & 0xFFFFFF) == unaligned_relative {
                        found_unaligned_region = true;
                        break;
                    }
                }

                if found_unaligned_region {
                    return Err("Found memory descriptor that suggests unaligned allocation was processed".to_string());
                }

                // At least the stack HOB should create a boot services descriptor
                let boot_services_count = boot_services_descriptors.len();
                if boot_services_count == 0 {
                    return Err("Expected at least 1 boot services descriptor for stack HOB".to_string());
                }

                Ok(())
            });

        scenario.run_test();
    }

    #[test]
    #[serial]
    fn test_mmio_wo_rt_attribute_is_not_reported() {
        init_logger();
        let scenario = MemoryMapTestScenario::new("MMIO Without RT Attribute Test", SIZE_64MB as u64)
            .with_resource_descriptor(ResourceDescriptorConfig {
                resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
                resource_attribute: hob::TESTED_MEMORY_ATTRIBUTES,
                physical_start: SIZE_1MB as u64,
                resource_length: SIZE_32MB as u64,
                owner: ZERO,
            })
            .with_resource_descriptor(ResourceDescriptorConfig {
                resource_type: hob::EFI_RESOURCE_MEMORY_MAPPED_IO,
                resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT | hob::EFI_RESOURCE_ATTRIBUTE_INITIALIZED,
                physical_start: (SIZE_1MB * 34) as u64,
                resource_length: SIZE_8MB as u64,
                owner: ZERO,
            })
            .with_memory_allocation(MemoryAllocationConfig {
                memory_type: efi::MEMORY_MAPPED_IO,
                memory_base_address: (SIZE_1MB * 34) as u64,
                memory_length: SIZE_2MB as u64,
                name: ZERO,
            })
            .with_memory_allocation(MemoryAllocationConfig {
                memory_type: efi::RUNTIME_SERVICES_DATA,
                memory_base_address: SIZE_2MB as u64,
                memory_length: SIZE_2MB as u64,
                name: ZERO,
            })
            .with_memory_allocation(MemoryAllocationConfig {
                memory_type: efi::RUNTIME_SERVICES_CODE,
                memory_base_address: SIZE_4MB as u64,
                memory_length: SIZE_1MB as u64,
                name: ZERO,
            })
            .with_validation(|descriptors| {
                memory_map::print_details(descriptors);

                MemoryMapValidation::new()
                    .expect_memory_types(vec![efi::RUNTIME_SERVICES_DATA, efi::RUNTIME_SERVICES_CODE])
                    .expect_runtime_memory_mb(3)
                    .with_custom_validation(|descriptors| {
                        // Verify MMIO does NOT appear (no runtime attribute, so should be filtered)
                        let mmio_count = descriptors.iter().filter(|d| d.r#type == efi::MEMORY_MAPPED_IO).count();
                        if mmio_count > 0 {
                            return Err(format!(
                                "Non-runtime MMIO should not appear in UEFI memory map, but found {} MMIO descriptor(s)",
                                mmio_count
                            ));
                        }

                        log::info!(target: "memory_map_test", "✅ Test passed: Non-runtime MMIO correctly filtered out");
                        Ok(())
                    })
                    .validate(descriptors)
            });

        scenario.run_test();
    }
}
