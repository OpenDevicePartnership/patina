use alloc::{string::String, vec::Vec};
use patina_sdk::{boot_services::BootServices, component::service::IntoService};
use spin::RwLock;

use crate::{
    acpi::StandardAcpiProvider,
    acpi_table::Table,
    error::AmlError,
    service::{AcpiProvider, AmlParser, TableKey},
    signature::ACPI_CHECKSUM_OFFSET,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AmlSdtHandleInternal {
    modified: bool,
    table_key: TableKey,
    // SHERRY: the idea would be to use zerocopy to read opcodes and stuff from the AML Stream
    offset: usize,
    size: usize,
}

impl AmlSdtHandleInternal {
    fn new(table_key: TableKey, offset: usize, size: usize) -> Self {
        Self { modified: false, table_key, offset, size }
    }
}

// Sentinel
const ROOT_NODE: AmlSdtHandleInternal =
    AmlSdtHandleInternal { modified: false, table_key: TableKey(0), offset: 0, size: 0 };

pub type AmlHandle = AmlSdtHandleInternal;

pub enum AmlData {
    None,
    Opcode(u8),
    NameString(AmlNameStringPath),
    OpFn(AmlOp),
    UnsignedInt(u64),
    StringLiteral(String),
    Child(AmlHandle),
}

pub enum AmlOp {}

pub struct AmlNameStringPath {}

#[derive(IntoService)]
#[service(dyn AmlParser)]
struct StandardAmlParser<B: BootServices + 'static> {
    acpi_table_manager: StandardAcpiProvider<B>,
    open_handles: RwLock<Vec<AmlHandle>>,
    root_node: AmlSdtHandleInternal,
}

impl<B> StandardAmlParser<B>
where
    B: BootServices,
{
    pub fn new(acpi_table_manager: StandardAcpiProvider<B>) -> Self {
        Self { acpi_table_manager, open_handles: RwLock::new(Vec::new()), root_node: ROOT_NODE }
    }
}

impl<B> AmlParser for StandardAmlParser<B>
where
    B: BootServices,
{
    fn open_sdt(&self, table_key: crate::service::TableKey) -> Result<AmlHandle, crate::error::AmlError> {
        let size = 0; // SHERRY: there should be some function here that parses the size
        let handle = AmlHandle::new(table_key, 0, size);
        self.open_handles.write().push(handle);
        Ok(handle)
    }

    fn close_sdt(&self, handle: AmlHandle) -> Result<(), crate::error::AmlError> {
        let mut handles = self.open_handles.write();
        let handle_idx = handles.iter().position(|h| *h == handle);
        if let Some(index) = handle_idx {
            let handle_to_remove = handles.remove(index);
            let mut table_for_handle = self
                .acpi_table_manager
                .get_acpi_table(handle_to_remove.table_key)
                .map_err(|_| AmlError::InvalidHandle)?;
            table_for_handle.update_checksum(ACPI_CHECKSUM_OFFSET).map_err(|_| AmlError::CloseFailedChecksumUpdate)?;
            Ok(())
        } else {
            Err(crate::error::AmlError::InvalidHandle)
        }
    }

    fn iter_options(&self, handle: AmlHandle) -> Result<AmlData, crate::error::AmlError> {
        todo!()
    }

    fn get_child(&self, handle: AmlHandle) -> Result<AmlHandle, crate::error::AmlError> {
        todo!()
    }

    fn get_sibling(&self, handle: AmlHandle) -> Result<AmlHandle, crate::error::AmlError> {
        todo!()
    }
}
