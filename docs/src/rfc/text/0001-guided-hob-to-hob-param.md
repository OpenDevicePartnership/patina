# RFC: `Guided Hob to Hob<T>`

This is a request for comments for a design to allow a platform to register functionality with the core that will parse
a guided HOB (Hand-Off Block) into a specific struct and register that struct instance with the Core to be accessible
as a `Hob<T>` struct. This implementation will remove the need for a Component to parse the HOB list manually before
registering itself with the core, and instead moves the parsing to the core.

## Change Log

- 2025-04-09: Initial RFC created.
- 2025-04-10: Rename `IntoConfig` trait to `HobConfig`, `with_hob_parser` function to `with_hob_config`, `GUID` to
  `HOB_GUID`.
- 2025-04-10: Lock Config after registered.
- 2025-04-10: Add hob parsing implementation.
- 2025-04-14: Add function to allow core to register a default list of hob parsers
- 2025-04-14: Move from conversion from `Config<T>` to a new Param `Hob<T>` to support multiple instances of the same
  guided HOB, and to be able to remove the need to register HOBs with the core. Update `HobConfig` to `FromHob`.
  Remove `with_hob_config`.
- 2025-04-15: Move `parse_hobs` logic into core, so components with access to `Storage` from using `parse_hobs`
- 2025-05-08 - Amendment: Remove references to the now deprecated `uefi-sdk` repo

## Motivation

These are the main benefits to this RFC:

1. Allows the HOB list to only be parsed once instead of once per component
2. Separates and compartmentalizes the parsing logic
3. Moves all parsing prior to component execution, reducing confusion if parsing fails

## Technology Background

This proposal will use the existing `Storage` logic from the `patina_sdk` to store the new `Hob<T>` datums and parsers.

## Goals

1. Enable a simple interface for component dependency injectable configuration to be produced via a guided HOB in the
   HOB list
2. Create `T`'s for standard spec-defined guided HOBs that are available to component that wants it.

## Requirements

1. Dependency injectable Hob datums for component
2. Automatic parsing of guided HOBs to be used by components.

## Unresolved Questions

N/A

## Prior Art

Currently, each component that requires a HOB configuration parses the HOB list and configures itself prior to the
`Core` being initialized. The configured and initialized component is then registered with the core.

## Alternatives

N/A

## Rust Code Design

Current design is that a struct that can be generated from a guided HOB will implement a single trait. This trait
allows the struct to specify the guid that should trigger this parse and provides one overridable method for
generating `Self` from the byte slice. This `Self` is added to the storage. These values will be accessable to
components via the `Hob<T>` struct, which is a dependency injectable param. The `Hob<T>` holds `1..N` instances of
the guided HOB value, depending on how many were passed via the HOB list. Users can access the first value by
dereferencing the provided instance or they can iterate through all instances using the `IntoIterator` trait
implementation. `Hob<T>` parser implementations are registered automatically with `Storage` when a component that has
a `Hob<T>` in it's param list is registered, so there is no need for users to manually register any HOB parsers.

```rust
// Current Design implementation

/* -------- in patina_sdk ------- */

use patina_sdk::component::Storage;
use refi::efi::Guid

pub trait FromHob: Sized + 'static {
    const HOB_GUID: Guid::from_fields(...);

    fn register(bytes: &[u8], storage: &mut Storage) {
        storage.add_hob(Self::parse(bytes))
    }

    fn parse(bytes: &[u8]) -> Self;
}

pub struct Hob<'h, T: FromHob + 'static> {
    value: &'h [Box<dyn Any>]
    _marker: core::marker::PhantomData<T>
}

impl<'h, H: FromHob + 'static> Hob<'h, T> {
    pub fn mock(value: Vec<T>) -> Self {}
    pub fn iter(&self) => HobIter<'h, T> {}
}

impl<'h, H: FromHob + 'static> From<&'h [Box<dyn Any>]> {
    fn from(value: &'h [Box<dyn Any>]) -> Self { }
}

// Access the first entry
impl<'h H: FromHob + 'static> Deref for Hob<'h, H> {
    type Target = H;

    fn deref(&self) -> &Self::Target {}
}

impl <'h, H: FromHob + 'static> IntoIterator for &Hob<'h, H> {
    type Item = &'h T;
    type IntoIter = HobIter<'h, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct HobIter<'h, H> {
    inner: core::slice::Iter<'h, Box<dyn Any>>,
    _marker: core::marker::PhantomData<H>,
}

impl<'h, H: FromHob + 'static> Iterator for HobIter<'h, H> {
    type Item = &'h H;

    fn next(&mut self) -> Option<Self::Item> {}
}

struct Storage {
    hob_parsers: BTreeMap<Guid, fn(&[u8], &mut Storage)>,
    hobs: SparseVec<Vec<Box<dyn Any>>>,
    hob_indices: BTreeMap<TypeId, usize>,
}

impl Storage {
    pub(crate) fn add_hob_parser<H: FromHob>(&mut self) {}
    pub(crate) fn register_hob<H: FromHob>(&mut self) {}
    pub(crate) fn get_or_register_hob(&mut self, id: TypeId) -> usize {}
    pub(crate) fn add_hob<H: FromHob>(&mut self, hob: H) {}
    pub(crate) fn get_raw_hob(&self, id: usize) -> &[Box<dyn Any>] {}
    pub fn get_hob<'a, T: FromHob>(&self) -> Option<Hob<'a, T>> {}
    pub fn get_hob_parser(&self, guid: &Guid) -> Option<fn(&[u8], &mut Storage)>
}

/* ----- In lib.rs ------ */

// Shortened impl for brevity - But this is for post init_memory()
impl Core {

    pub fn parse_hobs(&mut self) -> Result<()> {}

    pub fn start(mut self) -> Result<()> {
        self.parse_hobs(self.hob_list)

        /* Continue */
    }
}
```

``` rust
    // Example usage from the platform perspective

    #[derive(Debug)]
    struct MyHobConfig;

    impl FromHob for MyHobConfig {
        const HOB_GUID: Guid = /* guid */
        fn parse(&[u8]) -> Self {
            // Parse bytes into struct however you want
            MyHobConfig
        }
    }

    #[derive(FromHob, Debug, Clone, Copy)]
    #[hob = "8be4df61-93ca-11d2-aa0d-00e098032b8c"]
    struct MyOtherHobConfig;

    fn my_component(hob: Hob<MyOtherHobConfig>) -> Result<()> {

    }

    // In entry point
    Core::default()
        .init_memory(physical_hob_list)
        .with_component(my_component) // This automatically registers the hob parser for `MyOtherHobConfig`
        .start()
        .unwrap()
```

## Guide-Level Explanation

1. `FromHob` The trait responsible for converting a byte array into a specific struct
2. `Storage` Internal storage that contains all configuration (among other things)
3. `Core` The DxeCore which is a two phased system - pre_mem and post_mem
