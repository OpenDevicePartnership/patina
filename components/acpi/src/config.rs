#[derive(Debug, Clone, Copy, Default)]
pub struct AcpiProviderInit {
    pub version: u32,
    pub should_reclaim_memory: bool,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub creator_id: u32,
    pub creator_revision: u32,
}

impl AcpiProviderInit {
    pub fn new(
        version: u32,
        should_reclaim_memory: bool,
        oem_id: [u8; 6],
        oem_table_id: [u8; 8],
        creator_id: u32,
        creator_revision: u32,
    ) -> Self {
        Self { version, should_reclaim_memory, oem_id, oem_table_id, creator_id, creator_revision }
    }
}
