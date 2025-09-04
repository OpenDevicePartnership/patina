//! UEFI Global Coherency Domain (GCD) Memory Block
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
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
pub enum MemoryBlock {
    Unallocated(dxe_services::MemorySpaceDescriptor),
    Allocated(dxe_services::MemorySpaceDescriptor),
}

pub enum StateTransition {
    Add(dxe_services::GcdMemoryType, u64, u64),
    Remove,
    Allocate(efi::Handle, Option<efi::Handle>),
    AllocateRespectingOwnership(efi::Handle, Option<efi::Handle>),
    Free,
    FreePreservingOwnership,
    SetAttributes(u64),
    SetCapabilities(u64),
}

impl Debug for StateTransition {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StateTransition::Add(memory_type, capabilities, attributes) => f
                .debug_struct("Add")
                .field("memory_type", memory_type)
                .field("capabilities", &format_args!("{capabilities:#X}"))
                .field("attributes", &format_args!("{attributes:#X}"))
                .finish(),
            StateTransition::Remove => f.debug_struct("Remove").finish(),
            StateTransition::Allocate(image_handle, device_handle) => f
                .debug_struct("Allocate")
                .field("image_handle", image_handle)
                .field("device_handle", device_handle)
                .finish(),
            StateTransition::AllocateRespectingOwnership(image_handle, device_handle) => f
                .debug_struct("AllocateRespectingOwnership")
                .field("image_handle", image_handle)
                .field("device_handle", device_handle)
                .finish(),
            StateTransition::Free => f.debug_struct("Free").finish(),
            StateTransition::FreePreservingOwnership => f.debug_struct("FreePreservingOwnership").finish(),
            StateTransition::SetAttributes(attributes) => {
                f.debug_struct("SetAttributes").field("attributes", &format_args!("{attributes:#X}")).finish()
            }
            StateTransition::SetCapabilities(capabilities) => {
                f.debug_struct("SetCapabilities").field("capabilities", &format_args!("{capabilities:#X}")).finish()
            }
        }
    }
}

#[derive(Debug)]
pub enum MemoryBlockSplit<'a> {
    Same(&'a mut MemoryBlock),
    Before(&'a mut MemoryBlock, MemoryBlock),
    After(&'a mut MemoryBlock, MemoryBlock),
    Middle(&'a mut MemoryBlock, MemoryBlock, MemoryBlock),
}

impl MemoryBlock {
    pub fn merge(&mut self, other: &mut MemoryBlock) -> bool {
        if self.is_same_state(other) && self.end() == other.start() {
            self.as_mut().length += other.as_ref().length;
            other.as_mut().length = 0;
            true
        } else {
            false
        }
    }

    pub fn split(&mut self, base_address: usize, len: usize) -> Result<MemoryBlockSplit<'_>, Error> {
        let start = base_address;
        let end = base_address + len;

        if !(self.start() <= start && start < end && end <= self.end()) {
            return Err(Error::BlockOutsideRange);
        }

        if self.start() == start && end == self.end() {
            return Ok(MemoryBlockSplit::Same(self));
        }

        if self.start() == start && end < self.end() {
            let mut next = MemoryBlock::clone(self);

            self.as_mut().base_address = base_address as u64;
            self.as_mut().length = len as u64;
            next.as_mut().base_address = end as u64;
            next.as_mut().length -= len as u64;

            return Ok(MemoryBlockSplit::Before(self, next));
        }

        if self.start() < start && end == self.end() {
            let mut next = MemoryBlock::clone(self);

            self.as_mut().length -= len as u64;
            next.as_mut().base_address = base_address as u64;
            next.as_mut().length = len as u64;

            return Ok(MemoryBlockSplit::After(self, next));
        }

        if self.start() < start && end < self.end() {
            let mut next = MemoryBlock::clone(self);
            let mut last = MemoryBlock::clone(self);

            self.as_mut().length = (start - self.start()) as u64;
            next.as_mut().base_address = base_address as u64;
            next.as_mut().length = len as u64;
            last.as_mut().length = (last.end() - end) as u64;
            last.as_mut().base_address = end as u64;

            return Ok(MemoryBlockSplit::Middle(self, next, last));
        }

        unreachable!()
    }

    pub fn split_state_transition(
        &mut self,
        base_address: usize,
        len: usize,
        transition: StateTransition,
    ) -> Result<MemoryBlockSplit<'_>, Error> {
        let mut split = self.split(base_address, len)?;

        match &mut split {
            MemoryBlockSplit::Same(mb) => {
                mb.state_transition(transition)?;
            }
            MemoryBlockSplit::Before(mb, next) => {
                if let Err(e) = mb.state_transition(transition) {
                    mb.merge(next);
                    error!(e);
                }
            }
            MemoryBlockSplit::After(prev, mb) => {
                if let Err(e) = mb.state_transition(transition) {
                    prev.merge(mb);
                    error!(e)
                }
            }
            MemoryBlockSplit::Middle(prev, mb, next) => {
                if let Err(e) = mb.state_transition(transition) {
                    mb.merge(next);
                    prev.merge(mb);
                    error!(e)
                }
            }
        }

        Ok(split)
    }

    pub fn is_same_state(&self, other: &MemoryBlock) -> bool {
        matches!((self, other),
          (MemoryBlock::Unallocated(self_desc), MemoryBlock::Unallocated(other_desc)) |
          (MemoryBlock::Allocated(self_desc), MemoryBlock::Allocated(other_desc))
            if self_desc.memory_type == other_desc.memory_type
              && self_desc.attributes == other_desc.attributes
              && self_desc.capabilities == other_desc.capabilities
              && self_desc.device_handle == other_desc.device_handle
              && self_desc.image_handle == other_desc.image_handle
        )
    }

    pub fn state_transition(&mut self, transition: StateTransition) -> Result<(), Error> {
        match transition {
            StateTransition::Add(memory_type, capabilities, attributes) => {
                self.add_transition(memory_type, capabilities, attributes)
            }
            StateTransition::Remove => self.remove_transition(),
            StateTransition::Allocate(image_handle, device_handle) => {
                self.allocate_transition(image_handle, device_handle, false)
            }
            StateTransition::AllocateRespectingOwnership(image_handle, device_handle) => {
                self.allocate_transition(image_handle, device_handle, true)
            }
            StateTransition::Free => self.free_transition(false),
            StateTransition::FreePreservingOwnership => self.free_transition(true),
            StateTransition::SetAttributes(attributes) => self.attribute_transition(attributes),
            StateTransition::SetCapabilities(capabilities) => self.capabilities_transition(capabilities),
        }
    }

    pub fn add_transition(
        &mut self,
        memory_type: dxe_services::GcdMemoryType,
        capabilities: u64,
        attributes: u64,
    ) -> Result<(), Error> {
        match self {
            Self::Unallocated(md)
                if md.memory_type == dxe_services::GcdMemoryType::NonExistent
                    && memory_type != dxe_services::GcdMemoryType::NonExistent =>
            {
                md.memory_type = memory_type;
                md.capabilities = capabilities;
                md.attributes = attributes;
                Ok(())
            }
            _ => Err(Error::InvalidStateTransition),
        }
    }

    pub fn remove_transition(&mut self) -> Result<(), Error> {
        match self {
            Self::Unallocated(md) if md.memory_type != dxe_services::GcdMemoryType::NonExistent => {
                md.memory_type = dxe_services::GcdMemoryType::NonExistent;
                md.capabilities = 0;
                Ok(())
            }
            _ => Err(Error::InvalidStateTransition),
        }
    }

    pub fn allocate_transition(
        &mut self,
        image_handle: efi::Handle,
        device_handle: Option<efi::Handle>,
        respect_ownership: bool,
    ) -> Result<(), Error> {
        match self {
            Self::Unallocated(md)
                if !matches!(
                    md.memory_type,
                    dxe_services::GcdMemoryType::NonExistent | dxe_services::GcdMemoryType::Unaccepted
                ) =>
            {
                if respect_ownership && !(md.image_handle == 0 as efi::Handle || md.image_handle == image_handle) {
                    //block has an owner that isn't the requester.
                    Err(Error::InvalidStateTransition)?;
                }
                md.image_handle = image_handle;
                if let Some(device_handle) = device_handle {
                    md.device_handle = device_handle;
                }

                *self = Self::Allocated(*md);
                Ok(())
            }
            _ => Err(Error::InvalidStateTransition),
        }
    }

    pub fn free_transition(&mut self, preserve_ownership: bool) -> Result<(), Error> {
        match self {
            Self::Allocated(md) if md.memory_type != dxe_services::GcdMemoryType::NonExistent => {
                if !preserve_ownership {
                    md.image_handle = 0 as efi::Handle;
                }
                md.device_handle = 0 as efi::Handle;
                *self = Self::Unallocated(*md);
                Ok(())
            }
            _ => Err(Error::InvalidStateTransition),
        }
    }

    pub fn attribute_transition(&mut self, attributes: u64) -> Result<(), Error> {
        match self {
            Self::Allocated(md) | Self::Unallocated(md)
                if md.memory_type != dxe_services::GcdMemoryType::NonExistent =>
            {
                if (md.capabilities | attributes) != md.capabilities {
                    Err(Error::InvalidStateTransition)
                } else {
                    md.attributes = attributes;
                    Ok(())
                }
            }
            _ => Err(Error::InvalidStateTransition),
        }
    }

    pub fn capabilities_transition(&mut self, capabilities: u64) -> Result<(), Error> {
        match self {
            Self::Allocated(md) | Self::Unallocated(md)
                if md.memory_type != dxe_services::GcdMemoryType::NonExistent =>
            {
                if (capabilities & md.attributes) != md.attributes {
                    //
                    // Current attributes must still be supported with new capabilities
                    //
                    Err(Error::InvalidStateTransition)
                } else {
                    md.capabilities = capabilities;
                    Ok(())
                }
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

impl AsRef<dxe_services::MemorySpaceDescriptor> for MemoryBlock {
    fn as_ref(&self) -> &dxe_services::MemorySpaceDescriptor {
        match self {
            MemoryBlock::Unallocated(msd) | MemoryBlock::Allocated(msd) => msd,
        }
    }
}

impl AsMut<dxe_services::MemorySpaceDescriptor> for MemoryBlock {
    fn as_mut(&mut self) -> &mut dxe_services::MemorySpaceDescriptor {
        match self {
            MemoryBlock::Unallocated(msd) | MemoryBlock::Allocated(msd) => msd,
        }
    }
}

#[cfg(test)]
mod memory_block_tests {
    use super::*;
    use dxe_services::{GcdMemoryType, MemorySpaceDescriptor};

    #[test]
    fn test_transition_types() {
        let block = MemoryBlock::Unallocated(MemorySpaceDescriptor {
            base_address: 0,
            length: 0,
            memory_type: GcdMemoryType::NonExistent,
            attributes: 0,
            capabilities: 0,
            device_handle: 0 as efi::Handle,
            image_handle: 0 as efi::Handle,
        });

        // Test add_transition
        let mut b1 = block;
        b1.state_transition(StateTransition::Add(GcdMemoryType::MemoryMappedIo, 0, 0)).unwrap();
        assert_eq!(b1.as_ref().memory_type, GcdMemoryType::MemoryMappedIo);

        // test remove transition
        let mut b2 = b1;
        b2.state_transition(StateTransition::Remove).unwrap();
        assert_eq!(b2.as_ref().memory_type, GcdMemoryType::NonExistent);

        // test allocate transition
        let mut b3 = block;
        b3.as_mut().memory_type = GcdMemoryType::MemoryMappedIo;
        b3.state_transition(StateTransition::Allocate(0 as efi::Handle, None)).unwrap();
        match b3 {
            MemoryBlock::Allocated(md) => {
                assert_eq!(md.image_handle, 0 as efi::Handle);
                assert_eq!(md.device_handle, 0 as efi::Handle);
            }
            _ => panic!("Expected Allocated"),
        }

        // test free transition
        let mut b4 = b3;
        b4.state_transition(StateTransition::Free).unwrap();
        match b4 {
            MemoryBlock::Unallocated(md) => {
                assert_eq!(md.image_handle, 0 as efi::Handle);
                assert_eq!(md.device_handle, 0 as efi::Handle);
            }
            _ => panic!("Expected Unallocated"),
        }

        // test capabilities transition
        let mut b5 = block;
        b5.as_mut().memory_type = GcdMemoryType::MemoryMappedIo;
        b5.as_mut().attributes = 0b11;
        b5.as_mut().capabilities = 0b111;

        // Attributes before extending capabilities should fail
        assert!(b5.state_transition(StateTransition::SetAttributes(0b1111)).is_err());

        b5.state_transition(StateTransition::SetCapabilities(0b1111)).unwrap();
        assert_eq!(b5.as_ref().capabilities, 0b1111);

        // test attribute transition
        b5.as_mut().memory_type = GcdMemoryType::MemoryMappedIo;

        b5.state_transition(StateTransition::SetAttributes(0b1111)).unwrap();
        assert_eq!(b5.as_ref().attributes, 0b1111);

        // Reducing capabilities when attributes are more should fail
        assert!(b5.state_transition(StateTransition::SetCapabilities(0b1011)).is_err());

        // Memory type must not be NonExistent to set the attributes or capabilities
        let mut b7 = block;
        assert!(b7.state_transition(StateTransition::SetAttributes(0b1111)).is_err());
        assert!(b7.state_transition(StateTransition::SetCapabilities(0b1111)).is_err());
    }
}
