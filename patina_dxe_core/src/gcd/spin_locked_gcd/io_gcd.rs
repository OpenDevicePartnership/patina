//! IO Global Coherency Domain
//!
//! This module contains the IO GCD specific handling logic.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use super::{
    AllocateType, Box, Display, EfiError, INVALID_HANDLE, InternalError, IoBlock, IoBlockError, IoBlockSplit,
    IoStateTransition, Rbt, SliceError, SliceKey, SpinLockedGcd, Vec, dxe_services, efi, ensure, error, function,
    io_block, node_size, ptr, vec, writelncrlf,
};

pub(super) const IO_BLOCK_SLICE_LEN: usize = 4096;
const IO_BLOCK_SLICE_SIZE: usize = IO_BLOCK_SLICE_LEN * node_size::<IoBlock>();

#[derive(Debug)]
///The I/O Global Coherency Domain (GCD) Services are used to manage the I/O resources visible to the boot processor.
pub struct IoGCD {
    pub(super) maximum_address: usize,
    pub(super) io_blocks: Rbt<'static, IoBlock>,
}

impl IoGCD {
    // Create an instance of the Global Coherency Domain (GCD) for testing.
    #[cfg(test)]
    pub(crate) const fn _new(io_address_bits: u32) -> Self {
        assert!(io_address_bits > 0);
        Self { io_blocks: Rbt::new(), maximum_address: 1 << io_address_bits }
    }

    pub fn init(&mut self, io_address_bits: u32) {
        self.maximum_address = 1 << io_address_bits;
    }

    fn init_io_blocks(&mut self) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);

        self.io_blocks.expand(
            // SAFETY: the boxed slice is leaked to back the tree storage for its lifetime.
            unsafe {
                Box::into_raw(vec![0_u8; IO_BLOCK_SLICE_SIZE].into_boxed_slice())
                    .as_mut()
                    .expect("RBT given null pointer in initialization.")
            },
        );

        self.io_blocks
            .add(IoBlock::Unallocated(dxe_services::IoSpaceDescriptor {
                io_type: dxe_services::GcdIoType::NonExistent,
                base_address: 0,
                length: self.maximum_address as u64,
                ..Default::default()
            }))
            .map_err(|_| EfiError::OutOfResources)?;

        Ok(())
    }

    /// This service adds reserved I/O, or system I/O resources to the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.9
    pub fn add_io_space(
        &mut self,
        io_type: dxe_services::GcdIoType,
        base_address: usize,
        len: usize,
    ) -> Result<usize, EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0, EfiError::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, EfiError::Unsupported);

        log::trace!(target: "allocations", "[{}] Adding IO space at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   IO Type: {:?}\n", function!(), io_type);

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }

        let io_blocks = &mut self.io_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = io_blocks.get_closest_idx(&(base_address as u64)).ok_or(EfiError::NotFound)?;
        let block = io_blocks.get_with_idx(idx).ok_or(EfiError::NotFound)?;

        ensure!(block.as_ref().io_type == dxe_services::GcdIoType::NonExistent, EfiError::AccessDenied);

        match Self::split_state_transition_at_idx(io_blocks, idx, base_address, len, IoStateTransition::Add(io_type)) {
            Ok(idx) => Ok(idx),
            Err(InternalError::IoBlock(IoBlockError::BlockOutsideRange)) => error!(EfiError::AccessDenied),
            Err(InternalError::IoBlock(IoBlockError::InvalidStateTransition)) => error!(EfiError::InvalidParameter),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service removes reserved I/O, or system I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.12
    pub fn remove_io_space(&mut self, base_address: usize, len: usize) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0, EfiError::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, EfiError::Unsupported);

        log::trace!(target: "allocations", "[{}] Removing IO space at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}\n", function!(), len);

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }

        let io_blocks = &mut self.io_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = io_blocks.get_closest_idx(&(base_address as u64)).ok_or(EfiError::NotFound)?;
        let block = *io_blocks.get_with_idx(idx).expect("Idx valid from get_closest_idx");

        match Self::split_state_transition_at_idx(io_blocks, idx, base_address, len, IoStateTransition::Remove) {
            Ok(_) => Ok(()),
            Err(InternalError::IoBlock(IoBlockError::BlockOutsideRange)) => error!(EfiError::NotFound),
            Err(InternalError::IoBlock(IoBlockError::InvalidStateTransition)) => match block {
                IoBlock::Unallocated(_) => error!(EfiError::NotFound),
                IoBlock::Allocated(_) => error!(EfiError::AccessDenied),
            },
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service allocates reserved I/O, or system I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.10
    pub fn allocate_io_space(
        &mut self,
        allocate_type: AllocateType,
        io_type: dxe_services::GcdIoType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
    ) -> Result<usize, EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0 && image_handle > ptr::null_mut(), EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Allocating IO space: {:x?}", function!(), allocate_type);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   IO Type: {:?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or_default());

        match allocate_type {
            AllocateType::BottomUp(max_address) => self.allocate_bottom_up(
                io_type,
                alignment,
                len,
                image_handle,
                device_handle,
                max_address.unwrap_or(usize::MAX),
            ),
            AllocateType::TopDown(max_address) => self.allocate_top_down(
                io_type,
                alignment,
                len,
                image_handle,
                device_handle,
                max_address.unwrap_or(usize::MAX),
            ),
            AllocateType::Address(address) => {
                ensure!(address + len <= self.maximum_address, EfiError::Unsupported);
                self.allocate_address(io_type, alignment, len, image_handle, device_handle, address)
            }
        }
    }

    pub(super) fn allocate_bottom_up(
        &mut self,
        io_type: dxe_services::GcdIoType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        max_address: usize,
    ) -> Result<usize, EfiError> {
        ensure!(len > 0, EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Bottom up IO allocation: {:#?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Max Address: {:#x}", function!(), max_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or_default());

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }

        let io_blocks = &mut self.io_blocks;

        log::trace!(target: "gcd_measure", "search");
        let mut current = io_blocks.first_idx();
        while let Some(idx) = current {
            let ib = io_blocks.get_with_idx(idx).expect("idx is valid from next_idx");
            if ib.len() < len {
                current = io_blocks.next_idx(idx);
                continue;
            }
            let address = ib.start();
            let mut addr = address & (usize::MAX << alignment);
            if addr < address {
                addr += 1 << alignment;
            }
            ensure!(addr + len <= max_address, EfiError::NotFound);
            if ib.as_ref().io_type != io_type {
                current = io_blocks.next_idx(idx);
                continue;
            }

            match Self::split_state_transition_at_idx(
                io_blocks,
                idx,
                addr,
                len,
                IoStateTransition::Allocate(image_handle, device_handle),
            ) {
                Ok(_) => return Ok(addr),
                Err(InternalError::IoBlock(_)) => {
                    current = io_blocks.next_idx(idx);
                    continue;
                }
                Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
                Err(e) => panic!("{e:?}"),
            }
        }
        Err(EfiError::NotFound)
    }

    pub(super) fn allocate_top_down(
        &mut self,
        io_type: dxe_services::GcdIoType,
        align_shift: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        max_address: usize,
    ) -> Result<usize, EfiError> {
        ensure!(len > 0, EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Top down IO allocation: {:#?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Max Address: {:#x}", function!(), max_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Align Shift: {:#x}", function!(), align_shift);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or_default());

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }

        let io_blocks = &mut self.io_blocks;

        log::trace!(target: "gcd_measure", "search");
        let mut current = io_blocks.get_closest_idx(&(max_address as u64));
        while let Some(idx) = current {
            let ib = io_blocks.get_with_idx(idx).expect("idx is valid from prev_idx");

            // Account for if the block is truncated by the max_address. Max address
            // is inclusive, but end() is exclusive so subtract 1 from end.
            let usable_len = if ib.end() - 1 > max_address { max_address - ib.start() + 1 } else { ib.len() };
            if usable_len < len {
                current = io_blocks.prev_idx(idx);
                continue;
            }

            // Find the last suitable aligned range in the IO block.
            let addr = (ib.start() + usable_len - len) & (usize::MAX << align_shift);
            if addr < ib.start() {
                current = io_blocks.prev_idx(idx);
                continue;
            }

            if ib.as_ref().io_type != io_type {
                current = io_blocks.prev_idx(idx);
                continue;
            }

            match Self::split_state_transition_at_idx(
                io_blocks,
                idx,
                addr,
                len,
                IoStateTransition::Allocate(image_handle, device_handle),
            ) {
                Ok(_) => return Ok(addr),
                Err(InternalError::IoBlock(_)) => {
                    current = io_blocks.prev_idx(idx);
                    continue;
                }
                Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
                Err(e) => panic!("{e:?}"),
            }
        }
        Err(EfiError::NotFound)
    }

    pub(super) fn allocate_address(
        &mut self,
        io_type: dxe_services::GcdIoType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        address: usize,
    ) -> Result<usize, EfiError> {
        ensure!(len > 0, EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Exact address IO allocation: {:#?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Address: {:#x}", function!(), address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   IO Type: {:?}", function!(), io_type);
        log::trace!(target: "allocations", "[{}]   Alignment: {:#x}", function!(), alignment);
        log::trace!(target: "allocations", "[{}]   Image Handle: {:#x?}", function!(), image_handle);
        log::trace!(target: "allocations", "[{}]   Device Handle: {:#x?}\n", function!(), device_handle.unwrap_or_default());

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }
        let io_blocks = &mut self.io_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = io_blocks.get_closest_idx(&(address as u64)).ok_or(EfiError::NotFound)?;
        let block = io_blocks.get_with_idx(idx).ok_or(EfiError::NotFound)?;

        ensure!(
            block.as_ref().io_type == io_type && address == address & (usize::MAX << alignment),
            EfiError::NotFound
        );

        match Self::split_state_transition_at_idx(
            io_blocks,
            idx,
            address,
            len,
            IoStateTransition::Allocate(image_handle, device_handle),
        ) {
            Ok(_) => Ok(address),
            Err(InternalError::IoBlock(_)) => error!(EfiError::NotFound),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service frees reserved I/O, or system I/O resources from the global coherency domain of the processor.
    ///
    /// # Documentation
    /// UEFI Platform Initialization Specification, Release 1.8, Section II-7.2.4.11
    pub fn free_io_space(&mut self, base_address: usize, len: usize) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(len > 0, EfiError::InvalidParameter);
        ensure!(base_address + len <= self.maximum_address, EfiError::Unsupported);

        log::trace!(target: "allocations", "[{}] Free IO space at {:#?}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Length: {:#x}\n", function!(), len);

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }

        let io_blocks = &mut self.io_blocks;

        log::trace!(target: "gcd_measure", "search");
        let idx = io_blocks.get_closest_idx(&(base_address as u64)).ok_or(EfiError::NotFound)?;

        match Self::split_state_transition_at_idx(io_blocks, idx, base_address, len, IoStateTransition::Free) {
            Ok(_) => Ok(()),
            Err(InternalError::IoBlock(_)) => error!(EfiError::NotFound),
            Err(InternalError::Slice(SliceError::OutOfSpace)) => error!(EfiError::OutOfResources),
            Err(e) => panic!("{e:?}"),
        }
    }

    /// This service returns a copy of the current set of memory blocks in the GCD.
    /// Since GCD is used to service heap expansion requests and thus should avoid allocations,
    /// Caller is required to initialize a vector of sufficient capacity to hold the descriptors
    /// and provide a mutable reference to it.
    pub fn get_io_descriptors(&mut self, buffer: &mut Vec<dxe_services::IoSpaceDescriptor>) -> Result<(), EfiError> {
        ensure!(self.maximum_address != 0, EfiError::NotReady);
        ensure!(buffer.capacity() >= self.io_descriptor_count(), EfiError::InvalidParameter);
        ensure!(buffer.is_empty(), EfiError::InvalidParameter);

        log::trace!(target: "allocations", "[{}] Enter\n", function!());

        if self.io_blocks.capacity() == 0 {
            self.init_io_blocks()?;
        }

        let blocks = &self.io_blocks;
        let mut current = blocks.first_idx();
        while let Some(idx) = current {
            let ib = blocks.get_with_idx(idx).expect("Index comes from dfs and should be valid");
            match ib {
                IoBlock::Allocated(descriptor) | IoBlock::Unallocated(descriptor) => buffer.push(*descriptor),
            }
            current = blocks.next_idx(idx);
        }
        Ok(())
    }

    fn split_state_transition_at_idx(
        io_blocks: &mut Rbt<IoBlock>,
        idx: usize,
        base_address: usize,
        len: usize,
        transition: IoStateTransition,
    ) -> Result<usize, InternalError> {
        let ib_before_split = *io_blocks.get_with_idx(idx).expect("Caller should ensure idx is valid.");

        log::trace!(target: "allocations", "[{}] Splitting IO block at {:#x}", function!(), base_address);
        log::trace!(target: "allocations", "[{}]   Total IO Blocks Right Now: {:#}", function!(), io_blocks.len());
        log::trace!(target: "allocations", "[{}]   Length: {:#x}", function!(), len);
        log::trace!(target: "allocations", "[{}]   Block Index: {:#x}", function!(), idx);
        log::trace!(target: "allocations", "[{}]   Transition: {:?}\n", function!(), transition);

        // SAFETY: split_state_transition does not update the key for this block.
        let new_idx = unsafe {
            match io_blocks.get_with_idx_mut(idx).expect("idx valid above").split_state_transition(
                base_address,
                len,
                transition,
            )? {
                IoBlockSplit::Same(_) => Ok(idx),
                IoBlockSplit::After(_, next) => {
                    log::trace!(target: "gcd_measure", "add");
                    log::trace!(target: "allocations", "[{}] IoBlockSplit (After) -> Next: {:#x?}\n", function!(), next);
                    io_blocks.add(next)
                }
                IoBlockSplit::Before(_, next) => {
                    log::trace!(target: "gcd_measure", "add");
                    log::trace!(target: "allocations", "[{}] IoBlockSplit (Before) -> Next: {:#x?}\n", function!(), next);
                    io_blocks.add(next).map(|_| idx)
                }
                IoBlockSplit::Middle(_, next, next2) => {
                    log::trace!(target: "gcd_measure", "add");
                    log::trace!(target: "gcd_measure", "add");
                    log::trace!(target: "allocations", "[{}] IoBlockSplit (Middle) -> Next: {:#x?}. Next2: {:#x?}\n", function!(), next, next2);
                    io_blocks.add_many([next2, next])
                }
            }
        };

        // If the split failed, restore the memory block to its previous state.
        let idx = match new_idx {
            Ok(idx) => idx,
            Err(e) => {
                log::error!("[{}] IO block split failed! -> Error: {:#?}", function!(), e);
                // SAFETY: restoring the prior block state does not change the base_address key.
                unsafe {
                    *io_blocks.get_with_idx_mut(idx).expect("idx valid above") = ib_before_split;
                }
                error!(e);
            }
        };

        // Lets see if we can merge the block with the next block
        if let Some(next_idx) = io_blocks.next_idx(idx) {
            let mut next = *io_blocks.get_with_idx(next_idx).expect("idx valid from insert");
            // SAFETY: merge does not update the base_address key for this block.
            unsafe {
                if io_blocks.get_with_idx_mut(idx).expect("idx valid from insert").merge(&mut next) {
                    io_blocks.delete_with_idx(next_idx).expect("Index already verified.");
                }
            }
        }

        // Lets see if we can merge the block with the previous block
        if let Some(prev_idx) = io_blocks.prev_idx(idx) {
            let mut block = *io_blocks.get_with_idx(idx).expect("idx valid from insert");
            // SAFETY: merge does not update the base_address key for this block.
            unsafe {
                if io_blocks.get_with_idx_mut(prev_idx).expect("idx valid from insert").merge(&mut block) {
                    io_blocks.delete_with_idx(idx).expect("Index already verified.");
                    return Ok(prev_idx);
                }
            }
        }

        Ok(idx)
    }

    /// returns the current count of blocks in the list.
    pub fn io_descriptor_count(&self) -> usize {
        self.io_blocks.len()
    }

    const GCD_IO_TYPE_NAMES: [&'static str; 4] = [
        "NonExist", // EfiGcdIoTypeNonExistent
        "Reserved", // EfiGcdIoTypeReserved
        "I/O     ", // EfiGcdIoTypeIo
        "Unknown ", // EfiGcdIoTypeMaximum
    ];
}

impl Display for IoGCD {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writelncrlf!(f, "GCDIoType  Range                            ")?;
        writelncrlf!(f, "========== =================================")?;

        let blocks = &self.io_blocks;
        let mut current = blocks.first_idx();
        while let Some(idx) = current {
            let ib = blocks.get_with_idx(idx).expect("idx is valid from next_idx");
            match ib {
                IoBlock::Allocated(descriptor) | IoBlock::Unallocated(descriptor) => {
                    let io_type_str_idx = usize::min(descriptor.io_type as usize, Self::GCD_IO_TYPE_NAMES.len() - 1);
                    writelncrlf!(
                        f,
                        "{}  {:016x?}-{:016x?}{}",
                        IoGCD::GCD_IO_TYPE_NAMES.get(io_type_str_idx).expect("io_type_str_idx bounded by min()"),
                        descriptor.base_address,
                        descriptor.base_address + descriptor.length - 1,
                        { if descriptor.image_handle == INVALID_HANDLE { "" } else { "*" } }
                    )?;
                }
            }
            current = blocks.next_idx(idx);
        }
        Ok(())
    }
}

impl SliceKey for IoBlock {
    type Key = u64;
    fn key(&self) -> &Self::Key {
        &self.as_ref().base_address
    }
}

impl From<io_block::Error> for InternalError {
    fn from(value: io_block::Error) -> Self {
        InternalError::IoBlock(value)
    }
}

impl SpinLockedGcd {
    /// Acquires lock and delegates to [`IoGCD::add_io_space`]
    pub fn add_io_space(
        &self,
        io_type: dxe_services::GcdIoType,
        base_address: usize,
        len: usize,
    ) -> Result<usize, EfiError> {
        self.io.lock().add_io_space(io_type, base_address, len)
    }

    /// Acquires lock and delegates to [`IoGCD::remove_io_space`]
    pub fn remove_io_space(&self, base_address: usize, len: usize) -> Result<(), EfiError> {
        self.io.lock().remove_io_space(base_address, len)
    }

    /// Acquires lock and delegates to [`IoGCD::allocate_io_space`]
    pub fn allocate_io_space(
        &self,
        allocate_type: AllocateType,
        io_type: dxe_services::GcdIoType,
        alignment: usize,
        len: usize,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
    ) -> Result<usize, EfiError> {
        self.io.lock().allocate_io_space(allocate_type, io_type, alignment, len, image_handle, device_handle)
    }

    /// Acquires lock and delegates to [`IoGCD::free_io_space`]
    pub fn free_io_space(&self, base_address: usize, len: usize) -> Result<(), EfiError> {
        self.io.lock().free_io_space(base_address, len)
    }

    /// Acquires lock and delegates to [`IoGCD::get_io_descriptors`]
    pub fn get_io_descriptors(&self, buffer: &mut Vec<dxe_services::IoSpaceDescriptor>) -> Result<(), EfiError> {
        self.io.lock().get_io_descriptors(buffer)
    }

    /// Acquires lock and delegates to [`IoGCD::io_descriptor_count`]
    pub fn io_descriptor_count(&self) -> usize {
        self.io.lock().io_descriptor_count()
    }
}
