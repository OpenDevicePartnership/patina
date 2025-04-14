# RFC: `Guided Hob to Config<T>`

This is a request for comments for a design to allow a platform to register functionality with the core that will parse
a guided hob into a specific struct and register that struct instance with the Core to be accessible as a `Config<T>`
or `ConfigMut<T>`. This implementation will remove the need for a Component to parse the hoblist manually before
registering itself with the core, and instead moves the parsing to the core.

## Change Log

- 2025-04-09: Initial RFC created.
- 2025-04-10: Rename `IntoConfig` trait to `HobConfig`, `with_hob_parser` function to `with_hob_config`, `GUID` to
  `HOB_GUID`.
- 2025-04-10: Lock Config after registered.
- 2025-04-10: Add hob parsing implementation.
- 2025-04-14: Add function to allow core to register a default list of hob parsers

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
3. Have a default set of hob parsers added by the core.

## Requirements

1. One line registration with the core of expected guided HOBs and their generated struct
2. Should the Trait `parse` method return a result, an option, or just Self?

## Unresolved Questions

N/A

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

pub trait HobConfig: Default + 'static {
    const HOB_GUID: Guid::from_fields(...);

    fn register_config(bytes: &[u8], storage: &mut Storage) {
        storage.add_config(Self::parse(bytes));
        storage.lock_config::<Self>();
    }

    fn parse(bytes: &[u8]) -> Self;
}

/* ----- In lib.rs ------ */

struct Core {
    hob_list: HobList<'static>,
    hob_parsers: BTreeMap<Guid, fn(&[u8], &mut Storage)>
}

// Shortened impl for brevity - But this is for post init_memory()
impl Core {

    fn with_hob_config<T: HobConfig>(&mut self) {
        self.hob_parsers.insert(T::HOB_GUID, T::register_config)
    }

    fn add_default_hob_configs(&mut self)  {
        self.with_hob_config::<A>();
        self.with_hob_config::<B>();
        self.with_hob_config::<C>();
    }

    fn parse_hobs_to_config(&mut self) {
        for hob in self.hob_list.iter() {
            if let mu_pi::hob::Hob::GuidHob(guid, data) = hob {
                match self.hob_config_parsers.get(&guid.name) {
                    Some(parser) => {
                        parser(data, &mut self.storage);
                    }
                    None => {
                        log::warn!("No parser registered for HOB: {:?}", guid);
                    }
                }
            }
        }
    }

    fn start(mut self) -> Result<()> {
        self.add_default_hob_configs();
        self.parse_hobs_to_config();

        /* Continue */
    }
}
```

``` rust
    // Example usage from the platform perspective

    #[derive(Debug)]
    struct MyHobConfig;

    impl HobConfig for MyHobConfig {
        const HOB_GUID: Guid = /* guid */
        fn parse(&[u8]) -> Self {
            // Parse bytes into struct however you want
            MyHobConfig
        }
    }

    #[derive(HobConfig, Debug, Clone, Copy)]
    #[hob = "8be4df61-93ca-11d2-aa0d-00e098032b8c"]
    struct MyOtherHobConfig;

    // In entry point
    Core::default()
        .init_memory(physical_hobList)
        .with_hob_config::<MyHobConfig>()
        .with_hob_config::<MyOtherHobConfig>()
        .start()
        .unwrap()
```

## Guide-Level Explanation

1. `HobConfig` The trait responsible for converting a byte array into a specific struct
2. `Storage` Internal storage that contains all configuration (among other things)
3. `Core` The DxeCore which is a two phased system - pre_mem and post_mem
