/// Initialization configuration for ACPI provider.
#[derive(Debug, Clone, Copy, Default)]
pub struct AcpiProviderInit {
    /// Platform vendor.
    pub oem_id: [u8; 6],
    /// Product variant for platform vendor.
    pub oem_table_id: [u8; 8],
    // Platform edition (OEM-defined). Not to be confused with ACPI revision.
    pub oem_revision: u32,
    /// ID of compiler used to generate the ACPI table.
    pub creator_id: u32,
    /// Version of the tool used to generate the ACPI table.
    pub creator_revision: u32,
}

impl AcpiProviderInit {
    pub fn new(
        should_reclaim_memory: bool,
        oem_id: [u8; 6],
        oem_table_id: [u8; 8],
        oem_revision: u32,
        creator_id: u32,
        creator_revision: u32,
    ) -> Self {
        Self { should_reclaim_memory, oem_id, oem_table_id, oem_revision, creator_id, creator_revision }
    }
}
