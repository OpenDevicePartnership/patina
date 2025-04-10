# RFC: `Guided Hob to Config<T>`

This is a request for comments for a design to allow a platform to register functionality with the core that will parse
a guided hob into a specific struct and register that struct instance with the Core to be accessible as a `Config<T>`
or `ConfigMut<T>`. This implementation will remove the need for a Component to parse the hoblist manually before
registering itself with the core, and instead moves the parsing to the core.

## Change Log

- 2025-04-09: Initial RFC created.

## Motivation

These are the main benefits to this RFC:

1. Allows the hoblist to only be parsed once instead of once per component
2. Separates and compartmentalizes the parsing logic
3. Moves all parsing prior to component execution, reducing confusion if parsing fails

## Technology Background

This proposal will use the existing `Storage` and `Config` logic from the `uefi_sdk` to allow for untyped storage of configuration.

## Goals

1. Enable a simple interface for component dependency injectable configuration to be produced via a guided hob in the
   hoblist
2. Create core / uefi-sdk `T`'s for standard spec-defined guided hobs that are available to component that wants it.

## Requirements

1. One line registration with the core of expected guided HOBs and their generated struct
2. Should the Trait `parse` method return a result, an option, or just Self?

## Unresolved Questions

- Should the name of the Trait be `FromGuidedHob` or `IntoConfig`

## Prior Art

Currently, each component that requires a HOB configuration parses the hoblist and configures itself prior to the
`Core` being initialized. The configured and initialized component is then registered with the core.

## Alternatives

N/A

## Rust Code Design

Current design is that a struct that can be generated from a guided hob will implement a single trait. This trait
allows the struct to specify the guid that should trigger this parse and provides one overridable method for
generating `Self` from the byte slice. As `Config<T>` requires that `T` implement `Default`, this trait has a
supertrait of `Default + 'static` to ensure that the generated `Self` meets the requirements to allow it to be
registered as a config with Storage.

```rust
// Current Design implementation

/* -------- in uefi_sdk ------- */

use uefi_sdk::component::Storage;
use refi::efi::Guid

pub trait IntoConfig: Default + 'static {
    const GUID: Guid;

    fn register_config(bytes: &[u8], storage: &mut Storage) {
        storage.add_config(Self::parse(bytes));
    }

    fn parse(bytes: &[u8]) -> Self;
}

/* ----- In lib.rs ------ */

struct Core {
    hob_parsers: BTreeMap<Guid, fn(&[u8], &mut Storage)>
}

// Shortened impl for brevity - But this is for post init_memory()
impl Core {

    fn with_hob_parser<T: IntoConfig>(&mut self) {
        self.hob_parsers.insert(T::GUID, T::register_config)
    }

    fn start(mut self) -> Result<()> {
        // Add Code to do a final parse of the Rust-y hoblist and call:
       if let Some(parser) = self.hob_parsers(guid) {
        (*parser)(hob_bytes, &mut self.storage)
       }
    }
}
```

``` rust
    // Example usage from the platform perspective

    #[derive(Debug)]
    struct MyHobConfig;

    impl IntoConfig for MyHobConfig {
        const HOB_GUID: Guid = /* guid */
        fn parse(&[u8]) -> Self {
            // Parse bytes into struct however you want
            MyHobConfig
        }
    }

    #[derive(IntoConfig, Debug, Clone, Copy)]
    #[hob = "8be4df61-93ca-11d2-aa0d-00e098032b8c"]
    struct MyOtherHobConfig;

    // In entry point
    Core::default()
        .init_memory(physical_hobList)
        .with_hob_parser::<MyHobConfig>()
        .with_hob_parser::<MyOtherHobConfig>()
        .start()
        .unwrap()
```

## Guide-Level Explanation

1. `IntoConfig` The trait responsible for converting a byte array into a specific struct
2. `Storage` Internal storage that contains all configuration (among other things)
3. `Core` The DxeCore which is a two phased system - pre_mem and post_mem
