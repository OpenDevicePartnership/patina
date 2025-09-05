//! UEFI Global Coherency Domain (GCD) I/O Block
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::fmt::Debug;

use mu_pi::dxe_services;
use r_efi::efi;

use crate::error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidStateTransition,
    BlockOutsideRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoBlock {
    Unallocated(dxe_services::IoSpaceDescriptor),
    Allocated(dxe_services::IoSpaceDescriptor),
}

#[derive(Debug)]
pub enum StateTransition {
    Add(dxe_services::GcdIoType),
    Remove,
    Allocate(efi::Handle, Option<efi::Handle>),
    Free,
}

#[derive(Debug, PartialEq)]
pub enum IoBlockSplit<'a> {
    Same(&'a mut IoBlock),
    Before(&'a mut IoBlock, IoBlock),
    After(&'a mut IoBlock, IoBlock),
    Middle(&'a mut IoBlock, IoBlock, IoBlock),
}

impl IoBlock {
    pub fn merge(&mut self, other: &mut IoBlock) -> bool {
        if self.is_same_state(other) && self.end() == other.start() {
            self.as_mut().length += other.as_ref().length;
            other.as_mut().length = 0;
            true
        } else {
            false
        }
    }

    pub fn split(&mut self, base_address: usize, len: usize) -> Result<IoBlockSplit<'_>, Error> {
        let start = base_address;
        let end = base_address + len;

        if !(self.start() <= start && start < end && end <= self.end()) {
            return Err(Error::BlockOutsideRange);
        }

        if self.start() == start && end == self.end() {
            return Ok(IoBlockSplit::Same(self));
        }

        if self.start() == start && end < self.end() {
            let mut next = IoBlock::clone(self);

            self.as_mut().base_address = base_address as u64;
            self.as_mut().length = len as u64;
            next.as_mut().base_address = end as u64;
            next.as_mut().length -= len as u64;

            return Ok(IoBlockSplit::Before(self, next));
        }

        if self.start() < start && end == self.end() {
            let mut next = IoBlock::clone(self);

            self.as_mut().length -= len as u64;
            next.as_mut().base_address = base_address as u64;
            next.as_mut().length = len as u64;

            return Ok(IoBlockSplit::After(self, next));
        }

        if self.start() < start && end < self.end() {
            let mut next = IoBlock::clone(self);
            let mut last = IoBlock::clone(self);

            self.as_mut().length = (start - self.start()) as u64;
            next.as_mut().base_address = base_address as u64;
            next.as_mut().length = len as u64;
            last.as_mut().length = (last.end() - end) as u64;
            last.as_mut().base_address = end as u64;

            return Ok(IoBlockSplit::Middle(self, next, last));
        }

        unreachable!()
    }

    pub fn split_state_transition(
        &mut self,
        base_address: usize,
        len: usize,
        transition: StateTransition,
    ) -> Result<IoBlockSplit<'_>, Error> {
        let mut split = self.split(base_address, len)?;

        match &mut split {
            IoBlockSplit::Same(mb) => {
                mb.state_transition(transition)?;
            }
            IoBlockSplit::Before(mb, next) => {
                if let Err(e) = mb.state_transition(transition) {
                    mb.merge(next);
                    error!(e);
                }
            }
            IoBlockSplit::After(prev, mb) => {
                if let Err(e) = mb.state_transition(transition) {
                    prev.merge(mb);
                    error!(e)
                }
            }
            IoBlockSplit::Middle(prev, mb, next) => {
                if let Err(e) = mb.state_transition(transition) {
                    mb.merge(next);
                    prev.merge(mb);
                    error!(e)
                }
            }
        }

        Ok(split)
    }

    pub fn is_same_state(&self, other: &IoBlock) -> bool {
        matches!((self, other),
          (IoBlock::Unallocated(self_desc), IoBlock::Unallocated(other_desc)) |
          (IoBlock::Allocated(self_desc), IoBlock::Allocated(other_desc))
            if self_desc.io_type == other_desc.io_type
               && self_desc.device_handle == other_desc.device_handle
               && self_desc.image_handle == other_desc.image_handle
        )
    }

    pub fn state_transition(&mut self, transition: StateTransition) -> Result<(), Error> {
        match transition {
            StateTransition::Add(io_type) => self.add_transition(io_type),
            StateTransition::Remove => self.remove_transition(),
            StateTransition::Allocate(image_handle, device_handle) => {
                self.allocate_transition(image_handle, device_handle)
            }
            StateTransition::Free => self.free_transition(),
        }
    }

    pub fn add_transition(&mut self, io_type: dxe_services::GcdIoType) -> Result<(), Error> {
        match self {
            Self::Unallocated(id)
                if id.io_type == dxe_services::GcdIoType::NonExistent
                    && io_type != dxe_services::GcdIoType::NonExistent =>
            {
                id.io_type = io_type;
                Ok(())
            }
            _ => Err(Error::InvalidStateTransition),
        }
    }

    pub fn remove_transition(&mut self) -> Result<(), Error> {
        match self {
            Self::Unallocated(id) if id.io_type != dxe_services::GcdIoType::NonExistent => {
                id.io_type = dxe_services::GcdIoType::NonExistent;
                Ok(())
            }
            _ => Err(Error::InvalidStateTransition),
        }
    }

    pub fn allocate_transition(
        &mut self,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
    ) -> Result<(), Error> {
        match self {
            Self::Unallocated(id) if id.io_type != dxe_services::GcdIoType::NonExistent => {
                id.image_handle = image_handle;
                if let Some(device_handle) = device_handle {
                    id.device_handle = device_handle;
                }
                *self = Self::Allocated(*id);
                Ok(())
            }
            _ => Err(Error::InvalidStateTransition),
        }
    }

    pub fn free_transition(&mut self) -> Result<(), Error> {
        match self {
            Self::Allocated(id) if id.io_type != dxe_services::GcdIoType::NonExistent => {
                id.image_handle = 0 as efi::Handle;
                id.device_handle = 0 as efi::Handle;
                *self = Self::Unallocated(*id);
                Ok(())
            }
            _ => Err(Error::InvalidStateTransition),
        }
    }

    pub fn start(&self) -> usize {
        self.as_ref().base_address as usize
    }

    pub fn end(&self) -> usize {
        (self.as_ref().base_address + self.as_ref().length) as usize
    }

    pub fn len(&self) -> usize {
        self.as_ref().length as usize
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl AsRef<dxe_services::IoSpaceDescriptor> for IoBlock {
    fn as_ref(&self) -> &dxe_services::IoSpaceDescriptor {
        match self {
            IoBlock::Unallocated(msd) | IoBlock::Allocated(msd) => msd,
        }
    }
}

impl AsMut<dxe_services::IoSpaceDescriptor> for IoBlock {
    fn as_mut(&mut self) -> &mut dxe_services::IoSpaceDescriptor {
        match self {
            IoBlock::Unallocated(msd) | IoBlock::Allocated(msd) => msd,
        }
    }
}

#[cfg(test)]
mod io_block_tests {
    use core::panic;

    use super::*;
    use dxe_services::{GcdIoType, IoSpaceDescriptor};

    #[test]
    fn test_blocks_can_merge() {
        let mut block1 = IoBlock::Allocated(IoSpaceDescriptor {
            base_address: 0,
            length: 10,
            io_type: GcdIoType::NonExistent,
            device_handle: core::ptr::null_mut(),
            image_handle: core::ptr::null_mut(),
        });
        let mut block2 = IoBlock::Allocated(IoSpaceDescriptor {
            base_address: 10,
            length: 10,
            io_type: GcdIoType::NonExistent,
            device_handle: core::ptr::null_mut(),
            image_handle: core::ptr::null_mut(),
        });

        // Check we can correctly merge two blocks
        assert!(block1.merge(&mut block2));
        assert_eq!(block1.as_ref().length, 20);
        assert_eq!(block2.as_ref().length, 0);

        let mut block3 = IoBlock::Unallocated(IoSpaceDescriptor {
            base_address: 20,
            length: 10,
            io_type: GcdIoType::NonExistent,
            device_handle: core::ptr::null_mut(),
            image_handle: core::ptr::null_mut(),
        });

        // Check we can't merge two blocks that are different allocation
        // states, even if the addresses line up
        assert!(!block1.merge(&mut block3));
        assert_eq!(block1.len(), 20);
        assert_eq!(block3.len(), 10);
    }

    #[test]
    fn test_blocks_can_split() {
        let mut block = IoBlock::Allocated(IoSpaceDescriptor {
            base_address: 10,
            length: 10,
            io_type: GcdIoType::NonExistent,
            device_handle: core::ptr::null_mut(),
            image_handle: core::ptr::null_mut(),
        });

        // Check cannot split if the range is outside the block
        assert_eq!(Err(Error::BlockOutsideRange), block.split(5, 10));
        assert_eq!(Err(Error::BlockOutsideRange), block.split(15, 10));

        // Check we can split the block with the same start and end
        match block.clone().split(10, 10).unwrap() {
            IoBlockSplit::Same(_) => {}
            _ => panic!("Expected Same"),
        }

        // Check we can split the block in half (before)
        match block.clone().split(10, 5).unwrap() {
            IoBlockSplit::Before(before, after) => {
                assert_eq!(before.len(), 5);
                assert_eq!(after.len(), 5);
            }
            _ => panic!("Expected Before"),
        }

        // Check we can split in the middle
        match block.clone().split(12, 5).unwrap() {
            IoBlockSplit::Middle(before, middle, after) => {
                assert_eq!(before.len(), 2);
                assert_eq!(middle.len(), 5);
                assert_eq!(after.len(), 3);
            }
            _ => panic!("Expected Middle"),
        }

        //  // Check we can split the block in half (after)
        match block.clone().split(15, 5).unwrap() {
            IoBlockSplit::After(before, after) => {
                assert_eq!(before.len(), 5);
                assert_eq!(after.len(), 5);
            }
            _ => panic!("Expected After"),
        }
    }

    #[test]
    fn test_abort_split_transition() {
        let mut block = IoBlock::Allocated(IoSpaceDescriptor {
            base_address: 10,
            length: 10,
            io_type: GcdIoType::NonExistent,
            device_handle: core::ptr::null_mut(),
            image_handle: core::ptr::null_mut(),
        });
        let block_check = block;

        // Test recover from failed transition `Same`
        let status = block.split_state_transition(10, 10, StateTransition::Add(GcdIoType::Io));
        assert_eq!(status, Err(Error::InvalidStateTransition));
        assert_eq!(block, block_check);

        // Test recover from failed transition `Before`
        let status = block.split_state_transition(10, 5, StateTransition::Free);
        assert_eq!(status, Err(Error::InvalidStateTransition));
        assert_eq!(block, block_check);

        // Test recover from failed transition `After`
        let status = block.split_state_transition(15, 5, StateTransition::Allocate(0 as efi::Handle, None));
        assert_eq!(status, Err(Error::InvalidStateTransition));
        assert_eq!(block, block_check);

        // Test recover from failed transition `Middle`
        let status = block.split_state_transition(12, 5, StateTransition::Remove);
        assert_eq!(status, Err(Error::InvalidStateTransition));
        assert_eq!(block, block_check);
    }

    #[test]
    fn test_transition_types() {
        let block = IoBlock::Unallocated(IoSpaceDescriptor {
            base_address: 50,
            length: 50,
            io_type: GcdIoType::NonExistent,
            device_handle: core::ptr::null_mut(),
            image_handle: core::ptr::null_mut(),
        });

        // Test add transition
        if let Ok(IoBlockSplit::Before(b1, b2)) =
            block.clone().split_state_transition(50, 25, StateTransition::Add(GcdIoType::Io))
        {
            assert_eq!(b1.as_ref().io_type, GcdIoType::Io);
            assert_eq!(b2.as_ref().io_type, GcdIoType::NonExistent);
        } else {
            panic!("Expected Ok. Test add transition failed.");
        }

        // Test remove transition
        let mut b1 = block;
        b1.as_mut().io_type = GcdIoType::Io;
        if let Ok(IoBlockSplit::Before(b1, b2)) = b1.split_state_transition(50, 25, StateTransition::Remove) {
            assert_eq!(b1.as_ref().io_type, GcdIoType::NonExistent);
            assert_eq!(b2.as_ref().io_type, GcdIoType::Io);
        } else {
            panic!("Expected Ok. Test remove transition failed.");
        }

        // Test allocate transition
        let mut b1 = block;
        b1.as_mut().io_type = GcdIoType::Io;
        if let Ok(IoBlockSplit::Before(b1, b2)) =
            b1.split_state_transition(50, 25, StateTransition::Allocate(0 as efi::Handle, Some(1 as efi::Handle)))
        {
            match (b1, b2) {
                (
                    IoBlock::Allocated(IoSpaceDescriptor { base_address: 50, length: 25, .. }),
                    IoBlock::Unallocated(IoSpaceDescriptor { base_address: 75, length: 25, .. }),
                ) => {}
                _ => panic!("Expected Allocated, Unallocated"),
            }
        } else {
            panic!("Expected Ok. Test allocate transition failed.");
        }

        // Test free transition
        let mut b1 = IoBlock::Allocated(*block.clone().as_ref());
        b1.as_mut().io_type = GcdIoType::Io;
        if let Ok(IoBlockSplit::Before(b1, b2)) = b1.split_state_transition(50, 25, StateTransition::Free) {
            match (b1, b2) {
                (
                    IoBlock::Unallocated(IoSpaceDescriptor { base_address: 50, length: 25, .. }),
                    IoBlock::Allocated(IoSpaceDescriptor { base_address: 75, length: 25, .. }),
                ) => {}
                _ => panic!("Expected Unallocated, Allocated"),
            }
        } else {
            panic!("Expected Ok. Test free transition failed.");
        }
    }
}
