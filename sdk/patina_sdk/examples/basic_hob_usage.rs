//! This example demonstrates how to consume Guided HOB(s) (Hand-off Block) in a Patina based monolithic component.
//!
//! Specifically, this example demonstrates how to define a custom GUIDed HOB struct and it's parser, then consume it
//! in a component. Other standard GUIDed HOBs may already be available and only the consumption portion of this
//! example is relevant. It should be noted that only GUIDed HOBs are provided to components. Any non-GUIDed HOB types
//! are not available to components at this time.
//!
//! As an additional note, there is no way for a component to produce a HOB. HOBs (Hand-off Blocks) are specifically
//! designed as a way for pre-DXE firmware to pass information to the DXE Core. Raw HOBs are passed directly to the
//! core during initialization from the relevant pre-DXE phase firmware. From there, the core will parse any GUIDed
//! HOBs found in the HOB list that have registered parsers, and make them available to components.
//!
//! ## HOB parsing
//!
//! HOBs and their respective parsers are automatically gathered when a component is registered, and one step of Core
//! initialization is to parse the HOB list and use any registered parsers to parse a GUIDed HOB.
use patina_sdk::component::prelude::*;
use patina_sdk::component::{IntoComponent, Storage};

/// This struct represents a custom HOB that is a simple cast and does not require any special handling or parsing.
/// Due to this, The `FromHob` trait can be derived automatically. The `Copy` trait is required for this type so that
/// the core can copy it and not worry about the underlying bytes staying valid. If a guided HOB with the below GUID is
/// found in the HOB list, this parser will automatically run and parse the HOB into this struct.
#[derive(Debug, Clone, Copy, FromHob)]
#[repr(C)]
#[hob = "00000000-0000-0000-0000-000000000001"]
pub struct CustomHob1 {
    pub data1: u32,
    pub data2: u32,
    pub data3: u64,
    pub data4: bool,
    padding: [u8; 7],
}

/// This struct is not a simple cast and requires special handling, thus the `FromHob` trait must be implemented
/// manually. Since it is not a simple cast, we also do not need the `#[repr(C)]` attribute. If a guided HOB with the
/// below GUID is found in the HOB list, this parser will automatically run and parse the HOB into this struct.
#[derive(Debug, Clone)]
pub struct CustomHob2(String);

impl FromHob for CustomHob2 {
    const HOB_GUID: r_efi::efi::Guid =
        r_efi::efi::Guid::from_fields(0x0, 0x0, 0x0, 0x0, 0x0, &[0x00, 0x00, 0x00, 0x0, 0x0, 0x02]);

    fn parse(bytes: &[u8]) -> Self {
        let out = String::from_utf8(bytes.to_vec()).expect("Failed to parse string from bytes");
        CustomHob2(out)
    }
}

/// A simple configuration struct that can be used to store a boolean value.
///
/// Used in the `hob_to_config` function to demonstrate taking a value from a HOB and storing it in a Config.
#[derive(Debug, Default)]
pub struct BooleanConfig(pub bool);

/// This function component demonstrates how to consume a HOB.
///
/// It shows off three different consumption scenarios:
/// 1. Consuming a HOB that must exist for the component to run (hob1).
/// 2. Consuming a HOB that may or may not exist (hob2).
/// 3. Consuming a HOB that may be in the hob list multiple times (hob1 again).
pub fn consume_multiple_hobs(hob1: Hob<CustomHob1>, hob2: Option<Hob<CustomHob2>>) -> Result<()> {
    // (3) Show off that if we expect a HOB to exist multiple times, we can iterate over it.
    for hob in hob1.iter() {
        println!("  Hob1 data: {:?}", hob);
    }

    // (2) Show off that we can have optional HOBs
    match hob2 {
        // (1) Show off that if we only expect a single HOB, we can dereference it directly.
        Some(hob) => println!("  Hob2 exists with data: {:?}", *hob),
        None => println!("  Hob2 does not exist, continuing without it."),
    };

    Ok(())
}

/// This function component demonstrates how to consume a HOB and convert part of it's contents into a Config.
pub fn hob_to_config(hob: Hob<CustomHob1>, mut cfg: ConfigMut<BooleanConfig>) -> Result<()> {
    cfg.0 = hob.data4;
    println!("  Hob data converted to config. Config Value: {:?}", cfg.0);

    // Mark this configuration as final, so that it cannot be modified further. No other component that consumes this
    // mutably will run.
    cfg.lock();

    Ok(())
}

fn main() {
    // Setup the storage and component for the example, similar to how it would be done in a real component.
    // This is not apart of the example, but is necessary to run the component in std.
    let mut storage = Storage::default();
    let components = vec![
        util::register_component(consume_multiple_hobs, &mut storage),
        util::register_component(hob_to_config, &mut storage),
    ];

    util::setup_storage(
        &mut storage,
        vec![
            util::Custom::Hob1(CustomHob1 { data1: 42, data2: 100, data3: 50, data4: true, padding: [0; 7] }),
            util::Custom::Hob1(CustomHob1 { data1: 43, data2: 101, data3: 10, data4: false, padding: [0; 7] }),
            util::Custom::Hob2(CustomHob2("Hello".to_string())),
        ],
    );

    // This is the core of the example. When this executes, it will consume the HOBs that were inserted above, printing
    // the data contained within them.
    for mut component in components {
        println!("Running component: {:?}", component.metadata().name());
        component.run(&mut storage).expect("Component execution failed");
    }
}

// Users reviewing this example can skip the following module, as it is not relevant to the example itself.
mod util {
    use mu_pi::hob::GuidHob;
    use patina_sdk::component::Component;

    use super::{CustomHob1, CustomHob2, FromHob, IntoComponent, Storage};

    pub enum Custom {
        Hob1(CustomHob1),
        Hob2(CustomHob2),
    }

    impl Custom {
        fn insert(self, hob_list: &mut mu_pi::hob::HobList) {
            match self {
                Custom::Hob1(hob) => insert_custom_hob1(hob_list, hob),
                Custom::Hob2(hob) => insert_custom_hob2(hob_list, hob),
            }
        }
    }

    pub fn register_component<H>(component: impl IntoComponent<H>, storage: &mut Storage) -> Box<dyn Component> {
        let mut component = component.into_component();
        component.initialize(storage);
        component
    }

    pub fn setup_storage(storage: &mut Storage, hobs: Vec<Custom>) {
        let mut hob_list = mu_pi::hob::HobList::new();

        for hob in hobs {
            hob.insert(&mut hob_list);
        }

        // Parse HOBs, which is done automatically by the component system.
        for hob in hob_list.iter() {
            match hob {
                mu_pi::hob::Hob::GuidHob(hob, data) => {
                    for parser in storage.get_hob_parsers(&hob.name) {
                        parser(data, storage);
                    }
                }
                _ => continue, // Skip other types of HOBs
            }
        }
    }

    /// A helper function to insert a custom HOB into the HOB list.
    fn insert_custom_hob1(hob_list: &mut mu_pi::hob::HobList, hob: CustomHob1) {
        let mut data = Vec::new();
        data.extend_from_slice(&hob.data1.to_le_bytes());
        data.extend_from_slice(&hob.data2.to_le_bytes());
        data.extend_from_slice(&hob.data3.to_le_bytes());
        data.push(hob.data4 as u8);
        data.extend_from_slice(&hob.padding);

        let as_slice = Box::leak(data.into_boxed_slice());

        let hob = Box::leak(Box::new(GuidHob {
            header: mu_pi::hob::header::Hob {
                r#type: mu_pi::hob::GUID_EXTENSION,
                length: std::mem::size_of::<CustomHob1>() as u16,
                reserved: 0,
            },
            name: r_efi::efi::Guid::from_fields(0x0, 0x0, 0x0, 0x0, 0x0, &[0x00, 0x00, 0x00, 0x0, 0x0, 0x01]),
        }));
        hob_list.push(mu_pi::hob::Hob::GuidHob(hob, as_slice));
    }

    /// A helper function to insert a custom HOB into the HOB list.
    fn insert_custom_hob2(hob_list: &mut mu_pi::hob::HobList, hob: CustomHob2) {
        let mut data = Vec::new();
        data.extend_from_slice(hob.0.as_bytes());

        let as_slice = Box::leak(data.into_boxed_slice());

        let hob = Box::leak(Box::new(GuidHob {
            header: mu_pi::hob::header::Hob {
                r#type: mu_pi::hob::GUID_EXTENSION,
                length: std::mem::size_of::<CustomHob2>() as u16,
                reserved: 0,
            },
            name: CustomHob2::HOB_GUID,
        }));
        hob_list.push(mu_pi::hob::Hob::GuidHob(hob, as_slice));
    }
}
