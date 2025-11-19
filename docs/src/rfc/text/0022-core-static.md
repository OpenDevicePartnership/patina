# RFC: `Static Resolution and usage in the Core`

This RFC proposes multiple architectural changes to our current usage of `static` throughout the codebase and with the
core including both legitimate `static` usage consolidation and static polymorphism where possible rather than dynamic
polymorphism.

## Change Log

- 2025-11-10: Initial RFC created.
- 2025-11-11: Clarified that this change reduces the initial stack frame size by sacrificing some binary size.
- 2025-11-11: Clarified that the performance gains are a side affect, not the intent of this change and that the
  performance differences would be incredibly small, at least for the impacted code today.
- 2025-11-11: Added test performance impact as tests can remove the global lock, and run in parallel.
- 2025-11-11: Remove associated constant from Platform trait to allow usage of `mockall::automock`
- 2025-11-11: Specify that not all statics will necessarily go in `UefiState` struct. Specify that the intent is to
  better organize the fields inside of the core, instead of having a core with 30 fields.
- 2025-11-13: Updated the Platform trait to sub-trait (type association) the component configuration for better
  organization. Also mentioned that we may move additional grouping of configs the same way in the future if deemed
  necessary. Provided example.
- 2025-11-13: Removed `UefiState` field from the title all together, and only used that as an example of where the
  dispatcher context will go. Further specified that other statics will ultimately go elsewhere
- 2025-11-13: Expanded on safety regarding the static instance. Mentioned the `Self::instance()` is only to be used in
  efiapi functions.
- 2025-11-13: Expanded on implementation example for converting the "efiapi" functions to using `Self::instance()` and
  described a method for making sure the efiapi functions cannot be called from within the core.

## Motivation

1. Reduction of stack size
2. Testing improvements for Patina maintainers
3. Improved code cleanliness for Patina maintaineres
4. Clear interface for Platform implementors on configuration options

## Technology Background

The technological background required for this RFC is entirely related to the rust programming language itself. Of
this, the prime knowledge area is rust traits, generics, and the static polymorphism provided by the traits.

It is suggested you read:

1. [rust-lang: Traits](https://doc.rust-lang.org/reference/items/traits.html)
2. [nomicon: repr-rust](https://doc.rust-lang.org/nomicon/repr-rust.html)

## Goals

1. Reduction of stack size
   1. Static definition of the Core reduces initial stack frame size
   2. Deferred instantiation of components reduces initial stack frame size

2. Testing improvements for Patina maintainers
   1. Removal of statics reduces complexity writing tests that are properly reset
   2. Generics for Core dependencies allows for easier mocking
   3. Improved testing performance

3. Improved code cleanliness for Patina maintaineres
   1. Never calling "efiapi" functions
   2. Making "efiapi" functions a light wrapper around pure-rust implementations

4. Clear Platform Interface and Configuration
   1. Clearly describes all configurations available to the platform.
   2. Clear platform notification when platform configuration options change.

## Requirements

1. Stack size / performance improvements*
    1. Platforms will declare the `Core` as a static in their binary file
    2. Platforms will declare their `Core` generics via the `Platform` trait (described in (4))
    3. Component, Service, and Config instantiation is moved to a `Platform` trait method, and thus does not take
       up space on the initial stack frame.
    4. Current usage of dynamic polymorphism in statics be switched to static polymorphism (e.g. section_extractor in
       the dispatcher context)*

2. Testing improvements for Patina maintainers
    1. All statics are moved into the `UefiState` field of the `Core`. e.g. `SpinLockedGcd`, `DispatcherContext`, etc.
    2. Possibility to add generics at additional levels, to make mocking for tests simple
    3. Ignore code coverage for all "efiapi" functions
    4. Removal of global lock, allowing for more parallel testing.

3. Improved code cleanliness for Patina maintaineres
    1. All pure-rust methods directly access `UefiState` field in the `Core`. e.g. `self.uefi_state.<thing>`.
    2. A `instance` function in the core provides an abstracted interface for accessing the `Core` statically in
       "efiapi" functions
    3. Only "efiapi" functions will use the `instance` functions
    4. "efiapi" functions are only wrappers around a static-less `Core` function accessed via `instance` function.
    5. "efiapi" functions are only for usage in the system tables or event notify callbacks. They are not to be called
       directly by other rust code.

4. Clear Platform Interface and Configuration
    1. A `Platform` trait is defined, which uses trait `type` functionality to clearly specify generics
       e.g. `type Extractor: SectionExtractor`
    2. The `Platform` trait uses static trait functions to specify platform configurations that is not "const init-able"
       such as something that requires an allocation
    3. `const` values for setting configurations that can be created at compile-time
    4. The Platform trait itself will remain stateless.

\* While performance improvements are mentioned here, they are only a side affect of the change and not the intent of
   of the change. By switching to static trait resolution at compile time (static polymorphism) rather then dynamic
   polymorphism at runtime, we save some performance on vtable function pointer indirection. Unless the code is
   incredibly "hot", the performance improvements will **not** be noticeable. As it stands, none of the dyn trait
   objects impacted by the change are "hot" enough code for performance to be impacted, though that does not mean
   future additions won't be.

## Unresolved Questions

- How to handle components like the advanced logger that need the hob list. Maybe provide the hob list in the
  `components` function?

## Prior Art (Existing PI C Implementation)

Previous to this RFC, static state is spread across the entire repository, and platforms currently registered configuration
for the core via dynamic polymorphism.

## Alternatives

N/A, keep the same.

## Rust Code Design

### Addition of the `Platform` and `ComponentInfo` traits

The first design change is that we will introduce two new traits for fully configuring / describing the platform at
compile time. This includes both function implementations for configuration, and type specifications for generic trait
resolution. If you remember previously, we did this in the core itself, and it lead to a laundry list of traits with
complex types specified. This interface is cleaner, and procides less complexities in the `Core`.

The `Platform` trait will start with a limited number of configuration options as shown below, but it is expected that
more will be specified as needed. If necessary, groupings of configurations will be placed in a sub-trait, and type
associated, similar to how the `ComponentInfo` trait is type associated to the `Platform` trait as seen below.

The `ComponentInfo` trait is the new location to register components, configs, and services. It breaks the registration
out into 3 distinct callback methods to enforce better organization. This trait replaces the `with_component`,
`with_service` and `with_config` methods that were directly implemented on the core.

**Note:** It is elected **not** to use associated consts (e.g. const fields) in the Platform trait as `automock` does not
mocking traits that use const fields without defaults. This can be re-evaluated at a different time if need be.

```rust
/// A trait to be implemented by the platform to register additional components, configurations, and services.
///
/// Allocations are available when these callbacks are invoked.
trait ComponentInfo {
    /// An optional platform callback to register components with the core.
    #[inline(always)]
    fn components<'a>(_add: &mut Add<'a, Component>) { }

    /// An optional platform callback to register configurations with the core.
    #[inline(always)]
    fn configs<'a>(_add: &mut Add<'a, Config>) { }

    /// An optional platform callback to register services with the core.
    #[inline(always)]
    fn services<'a>(_add: &mut Add<'a, Service>) { }
}

/// A trait to be implemented by the platform to provide configuration values and types to be used directly by the
/// Patina DXE Core.
trait Platform {
     /// The Platform component information.
    type ComponentInfo: ComponentInfo;

    /// The platform's section extractor type, used when extracting sections from firmware volumes.
    type Extractor: SectionExtractor;

    /// If true, the platform prioritizes allocating 32-bit memory when not otherwise specified.
    #[inline(always)]
    fn prioritize_32_bit_memory() -> bool { false }

    /// Specifies the GIC base addresses for AARCH64 systems.
    #[cfg(target_arch = "aarch64")]
    fn gic_bases() -> GicBases;
}
```

In this, platform owners will be encouraged (via documentation) to place `#[inline(always)]` on the trait
implementations where applicable to help the compiler with optimizations.

This will introduce changes to the core and how it consumes the trait. An example is shown below

```rust
pub struct Core<P: Platform> {
    ...
    uefi_state: UefiState<P: Platform>, // `UefiState` to be discussed next.
}

impl <P: Platform> Core<P> {
    /// This was originally `init_memory`. It is now `start` and the only public function.
    pub fn entry_point(&'static self, physical_hob_list: *const c_void) -> ! {
        self.init_memory(physical_hob_list)

        P::ComponentInfo::components(Add {
            storage: &mut self.storage,
            components: &mut self.components,
            _limiter: core::marker::PhantomData,
        });

        P::ComponentInfo::configs(Add {
            storage: &mut self.storage,
            components: &mut self.components,
            _limiter: core::marker::PhantomData,
        });

        P::ComponentInfo::services(Add {
            storage: &mut self.storage,
            components: &mut self.components,
            _limiter: core::marker::PhantomData
        });

        ...
    }

    fn init_memory(&self) {
        ...

        GCD.prioritize_32_bit_memory(P::prioritize_32_bit_memory());

        ...
    }
}
```

### Moving statics into the Core

Since we will be moving all of the static state into the Core, we need a clean way to store all of this state. The idea
is to group statics as seems appropriate, but is left as an implementation detail. For the sake of this RFC, we will
start with the idea of a new struct called `UefiState` which will store some of the pure UEFI related statics. This
includes the dispatcher context, and parsed FV data. Other statics, such as the ones more kernel-like in nature will
ultimately be moved elsewhere, but again, that is an implementation detail for the sake of this RFC.
statics sit in the top-level Core struct as fields.

Please note the application of this RFC will take place over multiple PRs as the complexity introduced by moving these
statics is quite vast. For this RFC, we will continue to show examples based off the dispatcher_context, as no other
statics depend directly on it, and it's move into the core is simple compared to the others. Additionally, it allows
demonstaration of moving a struct that is currently a dynamic trait object (dynamic polymorphism) to static
polymorphism decided by platform.

```rust
// in lib.rs
struct UefiState<P: Platform> {
    dispatcher_context: TplMutex<DispatcherContext<P::Extractor>>,
    ...
}

struct Core<P: Platform> {
    ...
    uefi_state: UefiState<P>,
}
```

For the most part, the move of a static member into the core is that simple. There is some work to be done to handle
the `efiapi` methods that previously directly access that static, which will be discussed in the next section.

The main difficulty of this move is that we need to perform some analysis to determine the dependency tree between all
statics in the core. We can only transition the "leaf node" statics* (e.g. the ones that do not have any other statics
accessing them). If that is impossible in some cases (some type of circular dependency), then the whole circular
dependency must be moved at once

\* The reason we can only move the leave nodes comes down to an implementation detail that will be discussed in the
   next section. However the main limitation is that the "efiapi" method must be an associated function of the Core to
   be able to access the core statically. The reasoning will be discussed below.

### Core UEFI & efiapi function support

The biggest limitation of the Core is that we are compatible with "efiapi" functions, however these functions cross
the FFI boundary and thus we cannot capture state in lambdas, like is the normal process for getting Sync/Send safe
data into stateless callbacks. To fix this, we create a static, type-erased pointer pointing at our Core. We can then
access that pointer into our "efiapi" functions. It must be a type-erased pointer because the Core is generic over
`P` ("Platform"), which ultimately means the layout in memory of the Core is not known by our crate, only by the binary
crate using the core.

This is only able to be done because of the general context in which our crate is used. There is only ever going to be
one `Core<P>` initialized in a given binary, so we can guarantee that the `Core` in the pointer is our core. To
continue down the safety conversation, there are also a few extra percautions we take. The first is that the only way
to set the instance is through the `entry_point` method, which requires that `Self` have a lifetime of `'static`. This
guarantees the pointer will always be valid.

The next has to do with mutable aliasing. It is impossible. Since the platform must instantiate the `Core` as a static,
it must always be immutable (.e.g. not `static mut`). In addition to this, the `instance()` function (which gets the
static referenced core in the "efiapi" functions), also only returns a reference. What this ultimately means is that all
fields in the Core must now have their own interior mutability. For the previously static types, this is no problem.
They already had that. But it does result in us needing to add some wrapper types around our existing fields. That is
an implementation detail that will not be discussed here.

Lets take a look at our interface for being able to access the `Core` statically.

**NOTE: The `instance()` method is only to be used by "efiapi" functions registered with the efi system table.**

```rust
/// in lib.rs
static __SELF: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());

impl <P: Platform> Core {
    fn set_instance(&self) {
        let ptr = NonNull::from(self).cast::<u8>().as_ptr();
        __SELF.store(ptr, core::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn instance<'a>() -> &'a Self {
        let ptr = __SELF.load(core::sync::atomic::Ordering::SeqCst);
        unsafe {
            NonNull::new(ptr).expect("Core instance is already initialize")
                .cast::<Self>()
                .as_ref()
        }
    }

    pub fn entry_point(&'static self, physical_hob_list: *const c_void) -> ! {
        self.set_instance();

        ...
    }
}
```

Next, we will talk about the layout of the "efiapi" functions. From here on out, the "efiapi" functions will be simple
wrappers around a first-class pure-rust implementation. The "efiapi" function will always do the following:

1. Get the static instance of the core with `Self::instance()`.
2. Convert any C / UEFI types to rust types (mainly pointers to references, including null checks)
3. Call the pure rust method.

The pure rust methods will be implemented on the appropriate field of the `Core` (not the `Core` itself) as seems
appropriate during the implementation, but the "efiapi" method must be implemented directly on the `Core` so that it
can call `Self::instance()`. Due to the rust module system, we can implement these "efiapi" methods on the `Core` in a
different module then the `Core` was defined. If these functions are not marked as `pub`, then they will not be
callable outside of that module, which will be good for preventing users from calling the "efiapi" methods directly.

Here is an example of converting a single "efiapi" function that the dispatcher context owns:

```rust
/// In dispatcher.rs
impl <E: SectionExtractor> Dispatcher<E> {
    pub fn trust(&mut self, handle: efi::Handle, file: &efi::Guid) -> Result<(), EfiError> {
        for driver in self.pending_drivers.iter_mut() {
            if driver.firmware_volume_handle == handle && OrdGuid(driver.file_name) == OrdGuid(*file) {
                driver.security_status = efi::Status::SUCCESS;
                return Ok(())
            }
        }
        Err(EfiError::NotFound)
    }
}

/// In another module, such as systemtables.rs
impl <P: Platform> Core<P> {
    #[coverage(off)]
    extern "efiapi" fn trust_efiapi(firmware_volume_handle: efi::Handle, file_name: *const efi::Guid) -> efi::Status {
        if file_name.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // Safety: caller must ensure that file_name is a valid pointer. It is null-checked above.
        let file_name = unsafe { file_name.read_unaligned() };

        match Self::instance().uefi_state.dispatcher_context.trust(firmware_volume_handle, &file_name) {
            Err(status) => status.into(),
            Ok(_) => efi::Status::SUCCESS,
        }
    }
}
```

### Platform Interface changes

We've now seen all of the core changes. What does the the change look for the platform? The platform now manually
implements the `Platform` trait. The actual name of the struct doing the implementing is an implementation detail of
the platform implementor. We will just use Q35 as an example.

```rust
struct Q35;

impl Platform for Q35 {
    type ComponentInfo = Self;
    type Extractor = CompositeSectionExtractor;
}

impl ComponentInfo for Q35 {
    fn components(mut add: Add<Component>) {
        add.component(AdvancedLoggerComponent::<Uart16550>::new(&LOGGER));
        add.component(q35_services::mm_config_provider::MmConfigurationProvider);
        add.component(q35_services::mm_control::QemuQ35PlatformMmControl::new());
        add.component(patina_mm::component::sw_mmi_manager::SwMmiManager::new());
    }

    fn configs(mut add: Add<Config>) {
        add.config(patina_mm::config::MmCommunicationConfiguration {
                acpi_base: patina_mm::config::AcpiBase::Mmio(0x0), // Actual ACPI base address will be set during boot
                cmd_port: patina_mm::config::MmiPort::Smi(0xB2),
                data_port: patina_mm::config::MmiPort::Smi(0xB3),
                comm_buffers: vec![],
            });
    }
}

static CORE: Core<Q35> = Core::new(CompositeSectionExtractor::new());

#[cfg_attr(target_os = "uefi", unsafe(export_name = "efi_main"))]
pub extern "efiapi" fn _start(physical_hob_list: *const c_void) -> ! {
    ...

    CORE.entry_point(physical_hob_list)
}
```

## Guide-Level Explanation

In this RFC, we are attempting to clean up and consolidate the static usage in the `patina_dxe_core` crate, which has
many benefits to both patina developers and platform owners. For Patina developers, by consolidating all static usage
into the `Core` struct, it allows us to much more easily test our code because we do not have to worry about some
static state being affected by other tests. This also allows us to easily specify generics in more locations throughout
the codebase, which enables easier testing because we can mock parts that we are not actually testing, but must exist
to compile.

For platform owners, static polymorphism allows them to fully define all of their platform configurations and
dependencies at compile time, which will result in compilation errors when initially configuring the core for their
platform, or as configurations evolve with Patina.

Generically, the combination of both a static core and delayed instantiation of components should reduce the size of
the initial stack frame. While the size of the initial stack frame is not currently a concern, as platforms begin to
use more patina components, it will be. This change will increase the binary size (size of the `Core` object) to
reduce the initial stack frame size (size of `Core` + `Config`'s + `Component`'s).

In a STD compilation, it was seen that the initial stack frame was reduced from 568 to 488 with this change (with no
registered components, configs, or services). Adding components, configs, or services increased the initial stack size
of the pre-rfc implementation by the size of the object + minimal overhead where as with this implementation, almost
no additional stack frame size was seen.

Additionally, I will note that there will be a very (and I mean very) slight performance increase as we certain
functionalities (such as the section extractor) move away from dyn trait objects and vtable indirection. I want to
reiterate that this is incredibly small performance gain. Almost worth not mentioning, but I do find it important to at
least mention in writing.
