use patina_sdk::component::hob::FromHob;

/// Hob that contains information about previously installed ACPI tables.
#[derive(Copy, Clone, FromHob)]
#[hob = "9f9a9506-5597-4515-bab6-8bcde784ba87"]
pub struct AcpiMemoryHob {
    /// The address of the previous RSDP, which holds information about installed ACPI tables.
    pub rsdp_address: u64,
}
