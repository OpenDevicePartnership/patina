use mockall::automock;

#[cfg(test)]
#[automock]
struct MockGCD {
    maximum_address: usize,
    memory_blocks: Option<Rbt<'static, MemoryBlock>>,
}

#[cfg(test)]
impl MockGCD {
    #[cfg(test)]
    pub(crate) const fn new(processor_address_bits: u32) -> Self {
        todo!()
    }

    pub fn init(&mut self, processor_address_bits: u32) {
        todo!()
    }

    unsafe fn init_memory_blocks(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        base_address: usize,
        len: usize,
        capabilities: u64,
    ) -> Result<usize, Error> {
        todo!()
    }

    pub unsafe fn add_memory_space(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        base_address: usize,
        len: usize,
        mut capabilities: u64,
    ) -> Result<usize, Error> {
        todo!()
    }

    pub fn remove_memory_space(&mut self, base_address: usize, len: usize) -> Result<(), Error> {
        todo!()
    }

    pub fn allocate_memory_space(
        &mut self,
        allocate_type: AllocateType,
        memory_type: dxe_services::GcdMemoryType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
    ) -> Result<usize, Error> {
        todo!()
    }

    fn allocate_bottom_up(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        max_address: usize,
    ) -> Result<usize, Error> {
        todo!()
    }

    fn allocate_top_down(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        min_address: usize,
    ) -> Result<usize, Error> {
        todo!()
    }

    fn allocate_address(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        address: usize,
    ) -> Result<usize, Error> {
        todo!()
    }

    pub fn free_memory_space(&mut self, base_address: usize, len: usize) -> Result<(), Error> {
        todo!()
    }

    pub fn free_memory_space_preserving_ownership(&mut self, base_address: usize, len: usize) -> Result<(), Error> {
        todo!()
    }

    fn free_memory_space_worker(
        &mut self,
        base_address: usize,
        len: usize,
        transition: MemoryStateTransition,
    ) -> Result<(), Error> {
        todo!()
    }

    pub fn set_memory_space_attributes(
        &mut self,
        base_address: usize,
        len: usize,
        attributes: u64,
    ) -> Result<(), Error> {
        todo!()
    }

    pub fn set_memory_space_capabilities(
        &mut self,
        base_address: usize,
        len: usize,
        capabilities: u64,
    ) -> Result<(), Error> {
        todo!()
    }

    pub fn get_memory_descriptors(
        &mut self,
        buffer: &mut Vec<dxe_services::MemorySpaceDescriptor>,
    ) -> Result<(), Error> {
        todo!()
    }

    pub fn get_memory_descriptor_for_address(
        &mut self,
        address: efi::PhysicalAddress,
    ) -> Result<dxe_services::MemorySpaceDescriptor, Error> {
        todo!()
    }

    fn split_state_transition_at_idx(
        memory_blocks: &mut Rbt<MemoryBlock>,
        idx: usize,
        base_address: usize,
        len: usize,
        transition: MemoryStateTransition,
    ) -> Result<usize, InternalError> {
        todo!()
    }

    pub fn memory_descriptor_count(&self) -> usize {
        todo!()
    }
}
