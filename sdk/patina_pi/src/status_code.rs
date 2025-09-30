#![cfg_attr(rustfmt, rustfmt_skip)]
//! StatusCode related definitions in PI.
//!
//! These status codes are defined in UEFI Platform Initialization Specification 1.2,
//! Volume 3: Shared Architectural Elements.
//!
//! See <https://uefi.org/specs/PI/1.8A/V3_Status_Codes.html#code-definitions>.
//!
//! ## License
//!
//! Copyright (c) 2009 - 2018, Intel Corporation. All rights reserved.
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::protocols::status_code::{EfiStatusCodeType, EfiStatusCodeValue};
// Required for IA32, X64, IPF, ARM and EBC defines for CPU exception types
use r_efi::efi::protocols::debug_support;

// A Status Code Type is made up of the code type and severity.
// All values masked by EFI_STATUS_CODE_RESERVED_MASK are
// reserved for use by this specification.
//
pub const EFI_STATUS_CODE_TYPE_MASK:      EfiStatusCodeType = 0x000000FF;
pub const EFI_STATUS_CODE_SEVERITY_MASK:  EfiStatusCodeType = 0xFF000000;
pub const EFI_STATUS_CODE_RESERVED_MASK:  EfiStatusCodeType = 0x00FFFF00;

// Definition of code types. All other values masked by
// EFI_STATUS_CODE_TYPE_MASK are reserved for use by
// this specification.
//
pub const EFI_PROGRESS_CODE:  EfiStatusCodeType = 0x00000001;
pub const EFI_ERROR_CODE:     EfiStatusCodeType = 0x00000002;
pub const EFI_DEBUG_CODE:     EfiStatusCodeType = 0x00000003;

// Definitions of severities, all other values masked by
// EFI_STATUS_CODE_SEVERITY_MASK are reserved for use by
// this specification.
// Uncontained errors are major errors that could not contained
// to the specific component that is reporting the error.
// For example, if a memory error was not detected early enough,
// the bad data could be consumed by other drivers.
//
pub const EFI_ERROR_MINOR:        EfiStatusCodeType = 0x40000000;
pub const EFI_ERROR_MAJOR:        EfiStatusCodeType = 0x80000000;
pub const EFI_ERROR_UNRECOVERED:  EfiStatusCodeType = 0x90000000;
pub const EFI_ERROR_UNCONTAINED:  EfiStatusCodeType = 0xa0000000;

// A Status Code Value is made up of the class, subclass, and
// an operation.
//
pub const EFI_STATUS_CODE_CLASS_MASK:      EfiStatusCodeValue = 0xFF000000;
pub const EFI_STATUS_CODE_SUBCLASS_MASK:   EfiStatusCodeValue = 0x00FF0000;
pub const EFI_STATUS_CODE_OPERATION_MASK:  EfiStatusCodeValue = 0x0000FFFF;

// General partitioning scheme for Progress and Error Codes are:
//   - 0x0000-0x0FFF    Shared by all sub-classes in a given class.
//   - 0x1000-0x7FFF    Subclass Specific.
//   - 0x8000-0xFFFF    OEM specific.
//
pub const EFI_SUBCLASS_SPECIFIC:  EfiStatusCodeValue = 0x1000;
pub const EFI_OEM_SPECIFIC:       EfiStatusCodeValue = 0x8000;

// Debug Code definitions for all classes and subclass.
// Only one debug code is defined at this point and should
// be used for anything that is sent to the debug stream.
//
pub const EFI_DC_UNSPECIFIED:  EfiStatusCodeValue = 0x0;

// Class definitions.
// Values of 4-127 are reserved for future use by this specification.
// Values in the range 127-255 are reserved for OEM use.
//
pub const EFI_COMPUTING_UNIT:  EfiStatusCodeValue = 0x00000000;
pub const EFI_PERIPHERAL:      EfiStatusCodeValue = 0x01000000;
pub const EFI_IO_BUS:          EfiStatusCodeValue = 0x02000000;
pub const EFI_SOFTWARE:        EfiStatusCodeValue = 0x03000000;

// Computing Unit Subclass definitions.
// Values of 8-127 are reserved for future use by this specification.
// Values of 128-255 are reserved for OEM use.
//
pub const EFI_COMPUTING_UNIT_UNSPECIFIED:         EfiStatusCodeValue = EFI_COMPUTING_UNIT;
pub const EFI_COMPUTING_UNIT_HOST_PROCESSOR:      EfiStatusCodeValue = EFI_COMPUTING_UNIT | 0x00010000;
pub const EFI_COMPUTING_UNIT_FIRMWARE_PROCESSOR:  EfiStatusCodeValue = EFI_COMPUTING_UNIT | 0x00020000;
pub const EFI_COMPUTING_UNIT_IO_PROCESSOR:        EfiStatusCodeValue = EFI_COMPUTING_UNIT | 0x00030000;
pub const EFI_COMPUTING_UNIT_CACHE:               EfiStatusCodeValue = EFI_COMPUTING_UNIT | 0x00040000;
pub const EFI_COMPUTING_UNIT_MEMORY:              EfiStatusCodeValue = EFI_COMPUTING_UNIT | 0x00050000;
pub const EFI_COMPUTING_UNIT_CHIPSET:             EfiStatusCodeValue = EFI_COMPUTING_UNIT | 0x00060000;

// Computing Unit Class Progress Code definitions.
// These are shared by all subclasses.
//
pub const EFI_CU_PC_INIT_BEGIN:  EfiStatusCodeValue = 0x00000000;
pub const EFI_CU_PC_INIT_END:    EfiStatusCodeValue = 0x00000001;

// Computing Unit Unspecified Subclass Progress Code definitions.
//

// Computing Unit Host Processor Subclass Progress Code definitions.
//
pub const EFI_CU_HP_PC_POWER_ON_INIT:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_CU_HP_PC_CACHE_INIT:              EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_CU_HP_PC_RAM_INIT:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_CU_HP_PC_MEMORY_CONTROLLER_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_CU_HP_PC_IO_INIT:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
pub const EFI_CU_HP_PC_BSP_SELECT:              EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
pub const EFI_CU_HP_PC_BSP_RESELECT:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
pub const EFI_CU_HP_PC_AP_INIT:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
pub const EFI_CU_HP_PC_SMM_INIT:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;

// Computing Unit Firmware Processor Subclass Progress Code definitions.
//

// Computing Unit IO Processor Subclass Progress Code definitions.
//

// Computing Unit Cache Subclass Progress Code definitions.
//
pub const EFI_CU_CACHE_PC_PRESENCE_DETECT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_CU_CACHE_PC_CONFIGURATION:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;

// Computing Unit Memory Subclass Progress Code definitions.
//
pub const EFI_CU_MEMORY_PC_SPD_READ:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_CU_MEMORY_PC_PRESENCE_DETECT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_CU_MEMORY_PC_TIMING:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_CU_MEMORY_PC_CONFIGURING:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_CU_MEMORY_PC_OPTIMIZING:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
pub const EFI_CU_MEMORY_PC_INIT:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
pub const EFI_CU_MEMORY_PC_TEST:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;

// Computing Unit Chipset Subclass Progress Code definitions.
//

// South Bridge initialization prior to memory detection.
//
pub const EFI_CHIPSET_PC_PEI_CAR_SB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;

// North Bridge initialization prior to memory detection.
//
pub const EFI_CHIPSET_PC_PEI_CAR_NB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000001;

// South Bridge initialization after memory detection.
//
pub const EFI_CHIPSET_PC_PEI_MEM_SB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000002;

// North Bridge initialization after memory detection.
//
pub const EFI_CHIPSET_PC_PEI_MEM_NB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000003;

// PCI Host Bridge DXE initialization.
//
pub const EFI_CHIPSET_PC_DXE_HB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000004;

// North Bridge DXE initialization.
//
pub const EFI_CHIPSET_PC_DXE_NB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000005;

// North Bridge specific SMM initialization in DXE.
//
pub const EFI_CHIPSET_PC_DXE_NB_SMM_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000006;

// Initialization of the South Bridge specific UEFI Runtime Services.
//
pub const EFI_CHIPSET_PC_DXE_SB_RT_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000007;

// South Bridge DXE initialization
//
pub const EFI_CHIPSET_PC_DXE_SB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000008;

// South Bridge specific SMM initialization in DXE.
//
pub const EFI_CHIPSET_PC_DXE_SB_SMM_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000009;

// Initialization of the South Bridge devices.
//
pub const EFI_CHIPSET_PC_DXE_SB_DEVICES_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x0000000a;

// Computing Unit Class Error Code definitions.
// These are shared by all subclasses.
//
pub const EFI_CU_EC_NON_SPECIFIC:    EfiStatusCodeValue = 0x00000000;
pub const EFI_CU_EC_DISABLED:        EfiStatusCodeValue = 0x00000001;
pub const EFI_CU_EC_NOT_SUPPORTED:   EfiStatusCodeValue = 0x00000002;
pub const EFI_CU_EC_NOT_DETECTED:    EfiStatusCodeValue = 0x00000003;
pub const EFI_CU_EC_NOT_CONFIGURED:  EfiStatusCodeValue = 0x00000004;

// Computing Unit Unspecified Subclass Error Code definitions.
//

// Computing Unit Host Processor Subclass Error Code definitions.
//
pub const EFI_CU_HP_EC_INVALID_TYPE:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_CU_HP_EC_INVALID_SPEED:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_CU_HP_EC_MISMATCH:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_CU_HP_EC_TIMER_EXPIRED:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_CU_HP_EC_SELF_TEST:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
pub const EFI_CU_HP_EC_INTERNAL:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
pub const EFI_CU_HP_EC_THERMAL:              EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
pub const EFI_CU_HP_EC_LOW_VOLTAGE:          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
pub const EFI_CU_HP_EC_HIGH_VOLTAGE:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
pub const EFI_CU_HP_EC_CACHE:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
pub const EFI_CU_HP_EC_MICROCODE_UPDATE:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;
pub const EFI_CU_HP_EC_CORRECTABLE:          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000B;
pub const EFI_CU_HP_EC_UNCORRECTABLE:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000C;
pub const EFI_CU_HP_EC_NO_MICROCODE_UPDATE:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000D;

// Computing Unit Firmware Processor Subclass Error Code definitions.
//
pub const EFI_CU_FP_EC_HARD_FAIL:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_CU_FP_EC_SOFT_FAIL:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_CU_FP_EC_COMM_ERROR:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;

// Computing Unit IO Processor Subclass Error Code definitions.
//

// Computing Unit Cache Subclass Error Code definitions.
//
pub const EFI_CU_CACHE_EC_INVALID_TYPE:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_CU_CACHE_EC_INVALID_SPEED:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_CU_CACHE_EC_INVALID_SIZE:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_CU_CACHE_EC_MISMATCH:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;

// Computing Unit Memory Subclass Error Code definitions.
//
pub const EFI_CU_MEMORY_EC_INVALID_TYPE:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_CU_MEMORY_EC_INVALID_SPEED:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_CU_MEMORY_EC_CORRECTABLE:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_CU_MEMORY_EC_UNCORRECTABLE:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_CU_MEMORY_EC_SPD_FAIL:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
pub const EFI_CU_MEMORY_EC_INVALID_SIZE:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
pub const EFI_CU_MEMORY_EC_MISMATCH:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
pub const EFI_CU_MEMORY_EC_S3_RESUME_FAIL:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
pub const EFI_CU_MEMORY_EC_UPDATE_FAIL:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
pub const EFI_CU_MEMORY_EC_NONE_DETECTED:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
pub const EFI_CU_MEMORY_EC_NONE_USEFUL:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;

// Computing Unit Chipset Subclass Error Code definitions.
//
pub const EFI_CHIPSET_EC_BAD_BATTERY:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_CHIPSET_EC_DXE_NB_ERROR:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_CHIPSET_EC_DXE_SB_ERROR:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_CHIPSET_EC_INTRUDER_DETECT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;

// Peripheral Subclass definitions.
// Values of 12-127 are reserved for future use by this specification.
// Values of 128-255 are reserved for OEM use.
//
pub const EFI_PERIPHERAL_UNSPECIFIED:      EfiStatusCodeValue = EFI_PERIPHERAL;
pub const EFI_PERIPHERAL_KEYBOARD:         EfiStatusCodeValue = EFI_PERIPHERAL | 0x00010000;
pub const EFI_PERIPHERAL_MOUSE:            EfiStatusCodeValue = EFI_PERIPHERAL | 0x00020000;
pub const EFI_PERIPHERAL_LOCAL_CONSOLE:    EfiStatusCodeValue = EFI_PERIPHERAL | 0x00030000;
pub const EFI_PERIPHERAL_REMOTE_CONSOLE:   EfiStatusCodeValue = EFI_PERIPHERAL | 0x00040000;
pub const EFI_PERIPHERAL_SERIAL_PORT:      EfiStatusCodeValue = EFI_PERIPHERAL | 0x00050000;
pub const EFI_PERIPHERAL_PARALLEL_PORT:    EfiStatusCodeValue = EFI_PERIPHERAL | 0x00060000;
pub const EFI_PERIPHERAL_FIXED_MEDIA:      EfiStatusCodeValue = EFI_PERIPHERAL | 0x00070000;
pub const EFI_PERIPHERAL_REMOVABLE_MEDIA:  EfiStatusCodeValue = EFI_PERIPHERAL | 0x00080000;
pub const EFI_PERIPHERAL_AUDIO_INPUT:      EfiStatusCodeValue = EFI_PERIPHERAL | 0x00090000;
pub const EFI_PERIPHERAL_AUDIO_OUTPUT:     EfiStatusCodeValue = EFI_PERIPHERAL | 0x000A0000;
pub const EFI_PERIPHERAL_LCD_DEVICE:       EfiStatusCodeValue = EFI_PERIPHERAL | 0x000B0000;
pub const EFI_PERIPHERAL_NETWORK:          EfiStatusCodeValue = EFI_PERIPHERAL | 0x000C0000;
pub const EFI_PERIPHERAL_DOCKING:          EfiStatusCodeValue = EFI_PERIPHERAL | 0x000D0000;
pub const EFI_PERIPHERAL_TPM:              EfiStatusCodeValue = EFI_PERIPHERAL | 0x000E0000;

// Peripheral Class Progress Code definitions.
// These are shared by all subclasses.
//
pub const EFI_P_PC_INIT:             EfiStatusCodeValue = 0x00000000;
pub const EFI_P_PC_RESET:            EfiStatusCodeValue = 0x00000001;
pub const EFI_P_PC_DISABLE:          EfiStatusCodeValue = 0x00000002;
pub const EFI_P_PC_PRESENCE_DETECT:  EfiStatusCodeValue = 0x00000003;
pub const EFI_P_PC_ENABLE:           EfiStatusCodeValue = 0x00000004;
pub const EFI_P_PC_RECONFIG:         EfiStatusCodeValue = 0x00000005;
pub const EFI_P_PC_DETECTED:         EfiStatusCodeValue = 0x00000006;
pub const EFI_P_PC_REMOVED:          EfiStatusCodeValue = 0x00000007;

// Peripheral Class Unspecified Subclass Progress Code definitions.
//

// Peripheral Class Keyboard Subclass Progress Code definitions.
//
pub const EFI_P_KEYBOARD_PC_CLEAR_BUFFER:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_P_KEYBOARD_PC_SELF_TEST:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;

// Peripheral Class Mouse Subclass Progress Code definitions.
//
pub const EFI_P_MOUSE_PC_SELF_TEST:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;

// Peripheral Class Local Console Subclass Progress Code definitions.
//

// Peripheral Class Remote Console Subclass Progress Code definitions.
//

// Peripheral Class Serial Port Subclass Progress Code definitions.
//
pub const EFI_P_SERIAL_PORT_PC_CLEAR_BUFFER:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;

// Peripheral Class Parallel Port Subclass Progress Code definitions.
//

// Peripheral Class Fixed Media Subclass Progress Code definitions.
//

// Peripheral Class Removable Media Subclass Progress Code definitions.
//

// Peripheral Class Audio Input Subclass Progress Code definitions.
//

// Peripheral Class Audio Output Subclass Progress Code definitions.
//

// Peripheral Class LCD Device Subclass Progress Code definitions.
//

// Peripheral Class Network Subclass Progress Code definitions.
//

// Peripheral Class Error Code definitions.
// These are shared by all subclasses.
//
pub const EFI_P_EC_NON_SPECIFIC:       EfiStatusCodeValue = 0x00000000;
pub const EFI_P_EC_DISABLED:           EfiStatusCodeValue = 0x00000001;
pub const EFI_P_EC_NOT_SUPPORTED:      EfiStatusCodeValue = 0x00000002;
pub const EFI_P_EC_NOT_DETECTED:       EfiStatusCodeValue = 0x00000003;
pub const EFI_P_EC_NOT_CONFIGURED:     EfiStatusCodeValue = 0x00000004;
pub const EFI_P_EC_INTERFACE_ERROR:    EfiStatusCodeValue = 0x00000005;
pub const EFI_P_EC_CONTROLLER_ERROR:   EfiStatusCodeValue = 0x00000006;
pub const EFI_P_EC_INPUT_ERROR:        EfiStatusCodeValue = 0x00000007;
pub const EFI_P_EC_OUTPUT_ERROR:       EfiStatusCodeValue = 0x00000008;
pub const EFI_P_EC_RESOURCE_CONFLICT:  EfiStatusCodeValue = 0x00000009;

// Peripheral Class Unspecified Subclass Error Code definitions.
//

// Peripheral Class Keyboard Subclass Error Code definitions.
//
pub const EFI_P_KEYBOARD_EC_LOCKED:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_P_KEYBOARD_EC_STUCK_KEY:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_P_KEYBOARD_EC_BUFFER_FULL:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;

// Peripheral Class Mouse Subclass Error Code definitions.
//
pub const EFI_P_MOUSE_EC_LOCKED:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;

// Peripheral Class Local Console Subclass Error Code definitions.
//

// Peripheral Class Remote Console Subclass Error Code definitions.
//

// Peripheral Class Serial Port Subclass Error Code definitions.
//

// Peripheral Class Parallel Port Subclass Error Code definitions.
//

// Peripheral Class Fixed Media Subclass Error Code definitions.
//

// Peripheral Class Removable Media Subclass Error Code definitions.
//

// Peripheral Class Audio Input Subclass Error Code definitions.
//

// Peripheral Class Audio Output Subclass Error Code definitions.
//

// Peripheral Class LCD Device Subclass Error Code definitions.
//

// Peripheral Class Network Subclass Error Code definitions.
//

// IO Bus Subclass definitions.
// Values of 14-127 are reserved for future use by this specification.
// Values of 128-255 are reserved for OEM use.
//
pub const EFI_IO_BUS_UNSPECIFIED:  EfiStatusCodeValue = EFI_IO_BUS;
pub const EFI_IO_BUS_PCI:          EfiStatusCodeValue = EFI_IO_BUS | 0x00010000;
pub const EFI_IO_BUS_USB:          EfiStatusCodeValue = EFI_IO_BUS | 0x00020000;
pub const EFI_IO_BUS_IBA:          EfiStatusCodeValue = EFI_IO_BUS | 0x00030000;
pub const EFI_IO_BUS_AGP:          EfiStatusCodeValue = EFI_IO_BUS | 0x00040000;
pub const EFI_IO_BUS_PC_CARD:      EfiStatusCodeValue = EFI_IO_BUS | 0x00050000;
pub const EFI_IO_BUS_LPC:          EfiStatusCodeValue = EFI_IO_BUS | 0x00060000;
pub const EFI_IO_BUS_SCSI:         EfiStatusCodeValue = EFI_IO_BUS | 0x00070000;
pub const EFI_IO_BUS_ATA_ATAPI:    EfiStatusCodeValue = EFI_IO_BUS | 0x00080000;
pub const EFI_IO_BUS_FC:           EfiStatusCodeValue = EFI_IO_BUS | 0x00090000;
pub const EFI_IO_BUS_IP_NETWORK:   EfiStatusCodeValue = EFI_IO_BUS | 0x000A0000;
pub const EFI_IO_BUS_SMBUS:        EfiStatusCodeValue = EFI_IO_BUS | 0x000B0000;
pub const EFI_IO_BUS_I2C:          EfiStatusCodeValue = EFI_IO_BUS | 0x000C0000;

// IO Bus Class Progress Code definitions.
// These are shared by all subclasses.
//
pub const EFI_IOB_PC_INIT:      EfiStatusCodeValue = 0x00000000;
pub const EFI_IOB_PC_RESET:     EfiStatusCodeValue = 0x00000001;
pub const EFI_IOB_PC_DISABLE:   EfiStatusCodeValue = 0x00000002;
pub const EFI_IOB_PC_DETECT:    EfiStatusCodeValue = 0x00000003;
pub const EFI_IOB_PC_ENABLE:    EfiStatusCodeValue = 0x00000004;
pub const EFI_IOB_PC_RECONFIG:  EfiStatusCodeValue = 0x00000005;
pub const EFI_IOB_PC_HOTPLUG:   EfiStatusCodeValue = 0x00000006;

// IO Bus Class Unspecified Subclass Progress Code definitions.
//

// IO Bus Class PCI Subclass Progress Code definitions.
//
pub const EFI_IOB_PCI_BUS_ENUM:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_IOB_PCI_RES_ALLOC:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_IOB_PCI_HPC_INIT:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;

// IO Bus Class USB Subclass Progress Code definitions.
//

// IO Bus Class IBA Subclass Progress Code definitions.
//

// IO Bus Class AGP Subclass Progress Code definitions.
//

// IO Bus Class PC Card Subclass Progress Code definitions.
//

// IO Bus Class LPC Subclass Progress Code definitions.
//

// IO Bus Class SCSI Subclass Progress Code definitions.
//

// IO Bus Class ATA/ATAPI Subclass Progress Code definitions.
//
pub const EFI_IOB_ATA_BUS_SMART_ENABLE:          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_IOB_ATA_BUS_SMART_DISABLE:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_IOB_ATA_BUS_SMART_OVERTHRESHOLD:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_IOB_ATA_BUS_SMART_UNDERTHRESHOLD:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
// IO Bus Class FC Subclass Progress Code definitions.
//

// IO Bus Class IP Network Subclass Progress Code definitions.
//

// IO Bus Class SMBUS Subclass Progress Code definitions.
//

// IO Bus Class I2C Subclass Progress Code definitions.
//

// IO Bus Class Error Code definitions.
// These are shared by all subclasses.
//
pub const EFI_IOB_EC_NON_SPECIFIC:       EfiStatusCodeValue = 0x00000000;
pub const EFI_IOB_EC_DISABLED:           EfiStatusCodeValue = 0x00000001;
pub const EFI_IOB_EC_NOT_SUPPORTED:      EfiStatusCodeValue = 0x00000002;
pub const EFI_IOB_EC_NOT_DETECTED:       EfiStatusCodeValue = 0x00000003;
pub const EFI_IOB_EC_NOT_CONFIGURED:     EfiStatusCodeValue = 0x00000004;
pub const EFI_IOB_EC_INTERFACE_ERROR:    EfiStatusCodeValue = 0x00000005;
pub const EFI_IOB_EC_CONTROLLER_ERROR:   EfiStatusCodeValue = 0x00000006;
pub const EFI_IOB_EC_READ_ERROR:         EfiStatusCodeValue = 0x00000007;
pub const EFI_IOB_EC_WRITE_ERROR:        EfiStatusCodeValue = 0x00000008;
pub const EFI_IOB_EC_RESOURCE_CONFLICT:  EfiStatusCodeValue = 0x00000009;

// IO Bus Class Unspecified Subclass Error Code definitions.
//

// IO Bus Class PCI Subclass Error Code definitions.
//
pub const EFI_IOB_PCI_EC_PERR:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_IOB_PCI_EC_SERR:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;

// IO Bus Class USB Subclass Error Code definitions.
//

// IO Bus Class IBA Subclass Error Code definitions.
//

// IO Bus Class AGP Subclass Error Code definitions.
//

// IO Bus Class PC Card Subclass Error Code definitions.
//

// IO Bus Class LPC Subclass Error Code definitions.
//

// IO Bus Class SCSI Subclass Error Code definitions.
//

// IO Bus Class ATA/ATAPI Subclass Error Code definitions.
//
pub const EFI_IOB_ATA_BUS_SMART_NOTSUPPORTED:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_IOB_ATA_BUS_SMART_DISABLED:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;

// IO Bus Class FC Subclass Error Code definitions.
//

// IO Bus Class IP Network Subclass Error Code definitions.
//

// IO Bus Class SMBUS Subclass Error Code definitions.
//

// IO Bus Class I2C Subclass Error Code definitions.
//

// Software Subclass definitions.
// Values of 14-127 are reserved for future use by this specification.
// Values of 128-255 are reserved for OEM use.
//
pub const EFI_SOFTWARE_UNSPECIFIED:          EfiStatusCodeValue = EFI_SOFTWARE;
pub const EFI_SOFTWARE_SEC:                  EfiStatusCodeValue = EFI_SOFTWARE | 0x00010000;
pub const EFI_SOFTWARE_PEI_CORE:             EfiStatusCodeValue = EFI_SOFTWARE | 0x00020000;
pub const EFI_SOFTWARE_PEI_MODULE:           EfiStatusCodeValue = EFI_SOFTWARE | 0x00030000;
pub const EFI_SOFTWARE_DXE_CORE:             EfiStatusCodeValue = EFI_SOFTWARE | 0x00040000;
pub const EFI_SOFTWARE_DXE_BS_DRIVER:        EfiStatusCodeValue = EFI_SOFTWARE | 0x00050000;
pub const EFI_SOFTWARE_DXE_RT_DRIVER:        EfiStatusCodeValue = EFI_SOFTWARE | 0x00060000;
pub const EFI_SOFTWARE_SMM_DRIVER:           EfiStatusCodeValue = EFI_SOFTWARE | 0x00070000;
pub const EFI_SOFTWARE_EFI_APPLICATION:      EfiStatusCodeValue = EFI_SOFTWARE | 0x00080000;
pub const EFI_SOFTWARE_EFI_OS_LOADER:        EfiStatusCodeValue = EFI_SOFTWARE | 0x00090000;
pub const EFI_SOFTWARE_RT:                   EfiStatusCodeValue = EFI_SOFTWARE | 0x000A0000;
pub const EFI_SOFTWARE_AL:                   EfiStatusCodeValue = EFI_SOFTWARE | 0x000B0000;
pub const EFI_SOFTWARE_EBC_EXCEPTION:        EfiStatusCodeValue = EFI_SOFTWARE | 0x000C0000;
pub const EFI_SOFTWARE_IA32_EXCEPTION:       EfiStatusCodeValue = EFI_SOFTWARE | 0x000D0000;
pub const EFI_SOFTWARE_IPF_EXCEPTION:        EfiStatusCodeValue = EFI_SOFTWARE | 0x000E0000;
pub const EFI_SOFTWARE_PEI_SERVICE:          EfiStatusCodeValue = EFI_SOFTWARE | 0x000F0000;
pub const EFI_SOFTWARE_EFI_BOOT_SERVICE:     EfiStatusCodeValue = EFI_SOFTWARE | 0x00100000;
pub const EFI_SOFTWARE_EFI_RUNTIME_SERVICE:  EfiStatusCodeValue = EFI_SOFTWARE | 0x00110000;
pub const EFI_SOFTWARE_EFI_DXE_SERVICE:      EfiStatusCodeValue = EFI_SOFTWARE | 0x00120000;
pub const EFI_SOFTWARE_X64_EXCEPTION:        EfiStatusCodeValue = EFI_SOFTWARE | 0x00130000;
pub const EFI_SOFTWARE_ARM_EXCEPTION:        EfiStatusCodeValue = EFI_SOFTWARE | 0x00140000;


// Software Class Progress Code definitions.
// These are shared by all subclasses.
//
pub const EFI_SW_PC_INIT:                EfiStatusCodeValue = 0x00000000;
pub const EFI_SW_PC_LOAD:                EfiStatusCodeValue = 0x00000001;
pub const EFI_SW_PC_INIT_BEGIN:          EfiStatusCodeValue = 0x00000002;
pub const EFI_SW_PC_INIT_END:            EfiStatusCodeValue = 0x00000003;
pub const EFI_SW_PC_AUTHENTICATE_BEGIN:  EfiStatusCodeValue = 0x00000004;
pub const EFI_SW_PC_AUTHENTICATE_END:    EfiStatusCodeValue = 0x00000005;
pub const EFI_SW_PC_INPUT_WAIT:          EfiStatusCodeValue = 0x00000006;
pub const EFI_SW_PC_USER_SETUP:          EfiStatusCodeValue = 0x00000007;

// Software Class Unspecified Subclass Progress Code definitions.
//

// Software Class SEC Subclass Progress Code definitions.
//
pub const EFI_SW_SEC_PC_ENTRY_POINT:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_SEC_PC_HANDOFF_TO_NEXT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;

// Software Class PEI Core Subclass Progress Code definitions.
//
pub const EFI_SW_PEI_CORE_PC_ENTRY_POINT:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_PEI_CORE_PC_HANDOFF_TO_NEXT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_SW_PEI_CORE_PC_RETURN_TO_LAST:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;

// Software Class PEI Module Subclass Progress Code definitions.
//
pub const EFI_SW_PEI_PC_RECOVERY_BEGIN:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_PEI_PC_CAPSULE_LOAD:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_SW_PEI_PC_CAPSULE_START:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_SW_PEI_PC_RECOVERY_USER:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_SW_PEI_PC_RECOVERY_AUTO:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
pub const EFI_SW_PEI_PC_S3_BOOT_SCRIPT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
pub const EFI_SW_PEI_PC_OS_WAKE:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
pub const EFI_SW_PEI_PC_S3_STARTED:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;

// Software Class DXE Core Subclass Progress Code definitions.
//
pub const EFI_SW_DXE_CORE_PC_ENTRY_POINT:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_DXE_CORE_PC_HANDOFF_TO_NEXT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_SW_DXE_CORE_PC_RETURN_TO_LAST:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_SW_DXE_CORE_PC_START_DRIVER:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_SW_DXE_CORE_PC_ARCH_READY:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;

// Software Class DXE BS Driver Subclass Progress Code definitions.
//
pub const EFI_SW_DXE_BS_PC_LEGACY_OPROM_INIT:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_DXE_BS_PC_READY_TO_BOOT_EVENT:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_SW_DXE_BS_PC_LEGACY_BOOT_EVENT:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_SW_DXE_BS_PC_EXIT_BOOT_SERVICES_EVENT:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_SW_DXE_BS_PC_VIRTUAL_ADDRESS_CHANGE_EVENT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
pub const EFI_SW_DXE_BS_PC_VARIABLE_SERVICES_INIT:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
pub const EFI_SW_DXE_BS_PC_VARIABLE_RECLAIM:              EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
pub const EFI_SW_DXE_BS_PC_ATTEMPT_BOOT_ORDER_EVENT:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
pub const EFI_SW_DXE_BS_PC_CONFIG_RESET:                  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
pub const EFI_SW_DXE_BS_PC_CSM_INIT:                      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
pub const EFI_SW_DXE_BS_PC_BOOT_OPTION_COMPLETE:          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;   // MU_CHANGE

// Software Class SMM Driver Subclass Progress Code definitions.
//

// Software Class EFI Application Subclass Progress Code definitions.
//

// Software Class EFI OS Loader Subclass Progress Code definitions.
//

// Software Class EFI RT Subclass Progress Code definitions.
//
pub const EFI_SW_RT_PC_ENTRY_POINT:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_RT_PC_HANDOFF_TO_NEXT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_SW_RT_PC_RETURN_TO_LAST:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;

// Software Class X64 Exception Subclass Progress Code definitions.
//

// Software Class ARM Exception Subclass Progress Code definitions.
//

// Software Class EBC Exception Subclass Progress Code definitions.
//

// Software Class IA32 Exception Subclass Progress Code definitions.
//

// Software Class X64 Exception Subclass Progress Code definitions.
//

// Software Class IPF Exception Subclass Progress Code definitions.
//

// Software Class PEI Services Subclass Progress Code definitions.
//
pub const EFI_SW_PS_PC_INSTALL_PPI:              EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_PS_PC_REINSTALL_PPI:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_SW_PS_PC_LOCATE_PPI:               EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_SW_PS_PC_NOTIFY_PPI:               EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_SW_PS_PC_GET_BOOT_MODE:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
pub const EFI_SW_PS_PC_SET_BOOT_MODE:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
pub const EFI_SW_PS_PC_GET_HOB_LIST:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
pub const EFI_SW_PS_PC_CREATE_HOB:               EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
pub const EFI_SW_PS_PC_FFS_FIND_NEXT_VOLUME:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
pub const EFI_SW_PS_PC_FFS_FIND_NEXT_FILE:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
pub const EFI_SW_PS_PC_FFS_FIND_SECTION_DATA:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;
pub const EFI_SW_PS_PC_INSTALL_PEI_MEMORY:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000B;
pub const EFI_SW_PS_PC_ALLOCATE_PAGES:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000C;
pub const EFI_SW_PS_PC_ALLOCATE_POOL:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000D;
pub const EFI_SW_PS_PC_COPY_MEM:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000E;
pub const EFI_SW_PS_PC_SET_MEM:                  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000F;
pub const EFI_SW_PS_PC_RESET_SYSTEM:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000010;
pub const EFI_SW_PS_PC_FFS_FIND_FILE_BY_NAME:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000013;
pub const EFI_SW_PS_PC_FFS_GET_FILE_INFO:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000014;
pub const EFI_SW_PS_PC_FFS_GET_VOLUME_INFO:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000015;
pub const EFI_SW_PS_PC_FFS_REGISTER_FOR_SHADOW:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000016;

// Software Class EFI Boot Services Subclass Progress Code definitions.
//
pub const EFI_SW_BS_PC_RAISE_TPL:                      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_BS_PC_RESTORE_TPL:                    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_SW_BS_PC_ALLOCATE_PAGES:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_SW_BS_PC_FREE_PAGES:                     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_SW_BS_PC_GET_MEMORY_MAP:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
pub const EFI_SW_BS_PC_ALLOCATE_POOL:                  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
pub const EFI_SW_BS_PC_FREE_POOL:                      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
pub const EFI_SW_BS_PC_CREATE_EVENT:                   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
pub const EFI_SW_BS_PC_SET_TIMER:                      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
pub const EFI_SW_BS_PC_WAIT_FOR_EVENT:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
pub const EFI_SW_BS_PC_SIGNAL_EVENT:                   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;
pub const EFI_SW_BS_PC_CLOSE_EVENT:                    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000B;
pub const EFI_SW_BS_PC_CHECK_EVENT:                    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000C;
pub const EFI_SW_BS_PC_INSTALL_PROTOCOL_INTERFACE:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000D;
pub const EFI_SW_BS_PC_REINSTALL_PROTOCOL_INTERFACE:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000E;
pub const EFI_SW_BS_PC_UNINSTALL_PROTOCOL_INTERFACE:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000F;
pub const EFI_SW_BS_PC_HANDLE_PROTOCOL:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000010;
pub const EFI_SW_BS_PC_PC_HANDLE_PROTOCOL:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000011;
pub const EFI_SW_BS_PC_REGISTER_PROTOCOL_NOTIFY:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000012;
pub const EFI_SW_BS_PC_LOCATE_HANDLE:                  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000013;
pub const EFI_SW_BS_PC_INSTALL_CONFIGURATION_TABLE:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000014;
pub const EFI_SW_BS_PC_LOAD_IMAGE:                     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000015;
pub const EFI_SW_BS_PC_START_IMAGE:                    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000016;
pub const EFI_SW_BS_PC_EXIT:                           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000017;
pub const EFI_SW_BS_PC_UNLOAD_IMAGE:                   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000018;
pub const EFI_SW_BS_PC_EXIT_BOOT_SERVICES:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000019;
pub const EFI_SW_BS_PC_GET_NEXT_MONOTONIC_COUNT:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000001A;
pub const EFI_SW_BS_PC_STALL:                          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000001B;
pub const EFI_SW_BS_PC_SET_WATCHDOG_TIMER:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000001C;
pub const EFI_SW_BS_PC_CONNECT_CONTROLLER:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000001D;
pub const EFI_SW_BS_PC_DISCONNECT_CONTROLLER:          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000001E;
pub const EFI_SW_BS_PC_OPEN_PROTOCOL:                  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000001F;
pub const EFI_SW_BS_PC_CLOSE_PROTOCOL:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000020;
pub const EFI_SW_BS_PC_OPEN_PROTOCOL_INFORMATION:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000021;
pub const EFI_SW_BS_PC_PROTOCOLS_PER_HANDLE:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000022;
pub const EFI_SW_BS_PC_LOCATE_HANDLE_BUFFER:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000023;
pub const EFI_SW_BS_PC_LOCATE_PROTOCOL:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000024;
pub const EFI_SW_BS_PC_INSTALL_MULTIPLE_INTERFACES:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000025;
pub const EFI_SW_BS_PC_UNINSTALL_MULTIPLE_INTERFACES:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000026;
pub const EFI_SW_BS_PC_CALCULATE_CRC_32:               EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000027;
pub const EFI_SW_BS_PC_COPY_MEM:                       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000028;
pub const EFI_SW_BS_PC_SET_MEM:                        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000029;
pub const EFI_SW_BS_PC_CREATE_EVENT_EX:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000002A;

// Software Class EFI Runtime Services Subclass Progress Code definitions.
//
pub const EFI_SW_RS_PC_GET_TIME:                       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_RS_PC_SET_TIME:                       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_SW_RS_PC_GET_WAKEUP_TIME:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_SW_RS_PC_SET_WAKEUP_TIME:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_SW_RS_PC_SET_VIRTUAL_ADDRESS_MAP:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
pub const EFI_SW_RS_PC_CONVERT_POINTER:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
pub const EFI_SW_RS_PC_GET_VARIABLE:                   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
pub const EFI_SW_RS_PC_GET_NEXT_VARIABLE_NAME:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
pub const EFI_SW_RS_PC_SET_VARIABLE:                   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
pub const EFI_SW_RS_PC_GET_NEXT_HIGH_MONOTONIC_COUNT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
pub const EFI_SW_RS_PC_RESET_SYSTEM:                   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;
pub const EFI_SW_RS_PC_UPDATE_CAPSULE:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000B;
pub const EFI_SW_RS_PC_QUERY_CAPSULE_CAPABILITIES:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000C;
pub const EFI_SW_RS_PC_QUERY_VARIABLE_INFO:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000D;

// Software Class EFI DXE Services Subclass Progress Code definitions
//
pub const EFI_SW_DS_PC_ADD_MEMORY_SPACE:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_DS_PC_ALLOCATE_MEMORY_SPACE:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_SW_DS_PC_FREE_MEMORY_SPACE:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_SW_DS_PC_REMOVE_MEMORY_SPACE:          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_SW_DS_PC_GET_MEMORY_SPACE_DESCRIPTOR:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
pub const EFI_SW_DS_PC_SET_MEMORY_SPACE_ATTRIBUTES:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
pub const EFI_SW_DS_PC_GET_MEMORY_SPACE_MAP:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
pub const EFI_SW_DS_PC_ADD_IO_SPACE:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
pub const EFI_SW_DS_PC_ALLOCATE_IO_SPACE:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
pub const EFI_SW_DS_PC_FREE_IO_SPACE:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
pub const EFI_SW_DS_PC_REMOVE_IO_SPACE:              EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;
pub const EFI_SW_DS_PC_GET_IO_SPACE_DESCRIPTOR:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000B;
pub const EFI_SW_DS_PC_GET_IO_SPACE_MAP:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000C;
pub const EFI_SW_DS_PC_DISPATCH:                     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000D;
pub const EFI_SW_DS_PC_SCHEDULE:                     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000E;
pub const EFI_SW_DS_PC_TRUST:                        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000F;
pub const EFI_SW_DS_PC_PROCESS_FIRMWARE_VOLUME:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000010;

// Software Class Error Code definitions.
// These are shared by all subclasses.
//
pub const EFI_SW_EC_NON_SPECIFIC:                    EfiStatusCodeValue = 0x00000000;
pub const EFI_SW_EC_LOAD_ERROR:                      EfiStatusCodeValue = 0x00000001;
pub const EFI_SW_EC_INVALID_PARAMETER:               EfiStatusCodeValue = 0x00000002;
pub const EFI_SW_EC_UNSUPPORTED:                     EfiStatusCodeValue = 0x00000003;
pub const EFI_SW_EC_INVALID_BUFFER:                  EfiStatusCodeValue = 0x00000004;
pub const EFI_SW_EC_OUT_OF_RESOURCES:                EfiStatusCodeValue = 0x00000005;
pub const EFI_SW_EC_ABORTED:                         EfiStatusCodeValue = 0x00000006;
pub const EFI_SW_EC_ILLEGAL_SOFTWARE_STATE:          EfiStatusCodeValue = 0x00000007;
pub const EFI_SW_EC_ILLEGAL_HARDWARE_STATE:          EfiStatusCodeValue = 0x00000008;
pub const EFI_SW_EC_START_ERROR:                     EfiStatusCodeValue = 0x00000009;
pub const EFI_SW_EC_BAD_DATE_TIME:                   EfiStatusCodeValue = 0x0000000A;
pub const EFI_SW_EC_CFG_INVALID:                     EfiStatusCodeValue = 0x0000000B;
pub const EFI_SW_EC_CFG_CLR_REQUEST:                 EfiStatusCodeValue = 0x0000000C;
pub const EFI_SW_EC_CFG_DEFAULT:                     EfiStatusCodeValue = 0x0000000D;
pub const EFI_SW_EC_PWD_INVALID:                     EfiStatusCodeValue = 0x0000000E;
pub const EFI_SW_EC_PWD_CLR_REQUEST:                 EfiStatusCodeValue = 0x0000000F;
pub const EFI_SW_EC_PWD_CLEARED:                     EfiStatusCodeValue = 0x00000010;
pub const EFI_SW_EC_EVENT_LOG_FULL:                  EfiStatusCodeValue = 0x00000011;
pub const EFI_SW_EC_WRITE_PROTECTED:                 EfiStatusCodeValue = 0x00000012;
pub const EFI_SW_EC_FV_CORRUPTED:                    EfiStatusCodeValue = 0x00000013;
pub const EFI_SW_EC_INCONSISTENT_MEMORY_MAP:         EfiStatusCodeValue = 0x00000014;

// Software Class Unspecified Subclass Error Code definitions.
//

// Software Class SEC Subclass Error Code definitions.
//

// Software Class PEI Core Subclass Error Code definitions.
//
pub const EFI_SW_PEI_CORE_EC_DXE_CORRUPT:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_PEI_CORE_EC_DXEIPL_NOT_FOUND:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_SW_PEI_CORE_EC_MEMORY_NOT_INSTALLED:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;

// Software Class PEI Module Subclass Error Code definitions.
//
pub const EFI_SW_PEI_EC_NO_RECOVERY_CAPSULE:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_PEI_EC_INVALID_CAPSULE_DESCRIPTOR:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_SW_PEI_EC_S3_RESUME_PPI_NOT_FOUND:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_SW_PEI_EC_S3_BOOT_SCRIPT_ERROR:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_SW_PEI_EC_S3_OS_WAKE_ERROR:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
pub const EFI_SW_PEI_EC_S3_RESUME_FAILED:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
pub const EFI_SW_PEI_EC_RECOVERY_PPI_NOT_FOUND:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
pub const EFI_SW_PEI_EC_RECOVERY_FAILED:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
pub const EFI_SW_PEI_EC_S3_RESUME_ERROR:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
pub const EFI_SW_PEI_EC_INVALID_CAPSULE:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;

// Software Class DXE Foundation Subclass Error Code definitions.
//
pub const EFI_SW_DXE_CORE_EC_NO_ARCH:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_DXE_CORE_EC_IMAGE_LOAD_FAILURE:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;    // MU_CHANGE

// Software Class DXE Boot Service Driver Subclass Error Code definitions.
//
pub const EFI_SW_DXE_BS_EC_LEGACY_OPROM_NO_SPACE:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_DXE_BS_EC_INVALID_PASSWORD:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_SW_DXE_BS_EC_BOOT_OPTION_LOAD_ERROR:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_SW_DXE_BS_EC_BOOT_OPTION_FAILED:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_SW_DXE_BS_EC_INVALID_IDE_PASSWORD:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;

// Software Class DXE Runtime Service Driver Subclass Error Code definitions.
//

// Software Class SMM Driver Subclass Error Code definitions.
//

// Software Class EFI Application Subclass Error Code definitions.
//

// Software Class EFI OS Loader Subclass Error Code definitions.
//

// Software Class EFI RT Subclass Error Code definitions.
//

// Software Class EFI AL Subclass Error Code definitions.
//

// Software Class EBC Exception Subclass Error Code definitions.
// These exceptions are derived from the debug protocol definitions in the EFI
// specification.
//
pub const EFI_SW_EC_EBC_UNDEFINED:             EfiStatusCodeValue = 0x00000000;
pub const EFI_SW_EC_EBC_DIVIDE_ERROR:          EfiStatusCodeValue = debug_support::EXCEPT_EBC_DIVIDE_ERROR as u32;
pub const EFI_SW_EC_EBC_DEBUG:                 EfiStatusCodeValue = debug_support::EXCEPT_EBC_DEBUG as u32;
pub const EFI_SW_EC_EBC_BREAKPOINT:            EfiStatusCodeValue = debug_support::EXCEPT_EBC_BREAKPOINT as u32;
pub const EFI_SW_EC_EBC_OVERFLOW:              EfiStatusCodeValue = debug_support::EXCEPT_EBC_OVERFLOW as u32;
pub const EFI_SW_EC_EBC_INVALID_OPCODE:        EfiStatusCodeValue = debug_support::EXCEPT_EBC_INVALID_OPCODE as u32;
pub const EFI_SW_EC_EBC_STACK_FAULT:           EfiStatusCodeValue = debug_support::EXCEPT_EBC_STACK_FAULT as u32;
pub const EFI_SW_EC_EBC_ALIGNMENT_CHECK:       EfiStatusCodeValue = debug_support::EXCEPT_EBC_ALIGNMENT_CHECK as u32;
pub const EFI_SW_EC_EBC_INSTRUCTION_ENCODING:  EfiStatusCodeValue = debug_support::EXCEPT_EBC_INSTRUCTION_ENCODING as u32;
pub const EFI_SW_EC_EBC_BAD_BREAK:             EfiStatusCodeValue = debug_support::EXCEPT_EBC_BAD_BREAK as u32;
pub const EFI_SW_EC_EBC_STEP:                  EfiStatusCodeValue = debug_support::EXCEPT_EBC_SINGLE_STEP as u32;

// Software Class IA32 Exception Subclass Error Code definitions.
// These exceptions are derived from the debug protocol definitions in the EFI
// specification.
//
pub const EFI_SW_EC_IA32_DIVIDE_ERROR:     EfiStatusCodeValue = debug_support::EXCEPT_IA32_DIVIDE_ERROR as u32;
pub const EFI_SW_EC_IA32_DEBUG:            EfiStatusCodeValue = debug_support::EXCEPT_IA32_DEBUG as u32;
pub const EFI_SW_EC_IA32_NMI:              EfiStatusCodeValue = debug_support::EXCEPT_IA32_NMI as u32;
pub const EFI_SW_EC_IA32_BREAKPOINT:       EfiStatusCodeValue = debug_support::EXCEPT_IA32_BREAKPOINT as u32;
pub const EFI_SW_EC_IA32_OVERFLOW:         EfiStatusCodeValue = debug_support::EXCEPT_IA32_OVERFLOW as u32;
pub const EFI_SW_EC_IA32_BOUND:            EfiStatusCodeValue = debug_support::EXCEPT_IA32_BOUND as u32;
pub const EFI_SW_EC_IA32_INVALID_OPCODE:   EfiStatusCodeValue = debug_support::EXCEPT_IA32_INVALID_OPCODE as u32;
pub const EFI_SW_EC_IA32_DOUBLE_FAULT:     EfiStatusCodeValue = debug_support::EXCEPT_IA32_DOUBLE_FAULT as u32;
pub const EFI_SW_EC_IA32_INVALID_TSS:      EfiStatusCodeValue = debug_support::EXCEPT_IA32_INVALID_TSS as u32;
pub const EFI_SW_EC_IA32_SEG_NOT_PRESENT:  EfiStatusCodeValue = debug_support::EXCEPT_IA32_SEG_NOT_PRESENT as u32;
pub const EFI_SW_EC_IA32_STACK_FAULT:      EfiStatusCodeValue = debug_support::EXCEPT_IA32_STACK_FAULT as u32;
pub const EFI_SW_EC_IA32_GP_FAULT:         EfiStatusCodeValue = debug_support::EXCEPT_IA32_GP_FAULT as u32;
pub const EFI_SW_EC_IA32_PAGE_FAULT:       EfiStatusCodeValue = debug_support::EXCEPT_IA32_PAGE_FAULT as u32;
pub const EFI_SW_EC_IA32_FP_ERROR:         EfiStatusCodeValue = debug_support::EXCEPT_IA32_FP_ERROR as u32;
pub const EFI_SW_EC_IA32_ALIGNMENT_CHECK:  EfiStatusCodeValue = debug_support::EXCEPT_IA32_ALIGNMENT_CHECK as u32;
pub const EFI_SW_EC_IA32_MACHINE_CHECK:    EfiStatusCodeValue = debug_support::EXCEPT_IA32_MACHINE_CHECK as u32;
pub const EFI_SW_EC_IA32_SIMD:             EfiStatusCodeValue = debug_support::EXCEPT_IA32_SIMD as u32;

// Software Class IPF Exception Subclass Error Code definitions.
// These exceptions are derived from the debug protocol definitions in the EFI
// specification.
//
pub const EFI_SW_EC_IPF_ALT_DTLB:            EfiStatusCodeValue = debug_support::EXCEPT_IPF_ALT_DATA_TLB as u32;
pub const EFI_SW_EC_IPF_DNESTED_TLB:         EfiStatusCodeValue = debug_support::EXCEPT_IPF_DATA_NESTED_TLB as u32;
pub const EFI_SW_EC_IPF_BREAKPOINT:          EfiStatusCodeValue = debug_support::EXCEPT_IPF_BREAKPOINT as u32;
pub const EFI_SW_EC_IPF_EXTERNAL_INTERRUPT:  EfiStatusCodeValue = debug_support::EXCEPT_IPF_EXTERNAL_INTERRUPT as u32;
pub const EFI_SW_EC_IPF_GEN_EXCEPT:          EfiStatusCodeValue = debug_support::EXCEPT_IPF_GENERAL_EXCEPTION as u32;
pub const EFI_SW_EC_IPF_NAT_CONSUMPTION:     EfiStatusCodeValue = debug_support::EXCEPT_IPF_NAT_CONSUMPTION as u32;
pub const EFI_SW_EC_IPF_DEBUG_EXCEPT:        EfiStatusCodeValue = debug_support::EXCEPT_IPF_DEBUG as u32;
pub const EFI_SW_EC_IPF_UNALIGNED_ACCESS:    EfiStatusCodeValue = debug_support::EXCEPT_IPF_UNALIGNED_REFERENCE as u32;
pub const EFI_SW_EC_IPF_FP_FAULT:            EfiStatusCodeValue = debug_support::EXCEPT_IPF_FP_FAULT as u32;
pub const EFI_SW_EC_IPF_FP_TRAP:             EfiStatusCodeValue = debug_support::EXCEPT_IPF_FP_TRAP as u32;
pub const EFI_SW_EC_IPF_TAKEN_BRANCH:        EfiStatusCodeValue = debug_support::EXCEPT_IPF_TAKEN_BRANCH as u32;
pub const EFI_SW_EC_IPF_SINGLE_STEP:         EfiStatusCodeValue = debug_support::EXCEPT_IPF_SINGLE_STEP as u32;

// Software Class PEI Service Subclass Error Code definitions.
//
pub const EFI_SW_PS_EC_RESET_NOT_AVAILABLE:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_PS_EC_MEMORY_INSTALLED_TWICE:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;

// Software Class EFI Boot Service Subclass Error Code definitions.
//

// Software Class EFI Runtime Service Subclass Error Code definitions.
//

// Software Class EFI DXE Service Subclass Error Code definitions.
//
pub const EFI_SW_DXE_BS_PC_BEGIN_CONNECTING_DRIVERS:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
pub const EFI_SW_DXE_BS_PC_VERIFYING_PASSWORD:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;

// Software Class DXE RT Driver Subclass Progress Code definitions.
//
pub const EFI_SW_DXE_RT_PC_S0:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
pub const EFI_SW_DXE_RT_PC_S1:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
pub const EFI_SW_DXE_RT_PC_S2:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
pub const EFI_SW_DXE_RT_PC_S3:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
pub const EFI_SW_DXE_RT_PC_S4:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
pub const EFI_SW_DXE_RT_PC_S5:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;

// Software Class X64 Exception Subclass Error Code definitions.
// These exceptions are derived from the debug protocol
// definitions in the EFI specification.
//
pub const EFI_SW_EC_X64_DIVIDE_ERROR:     EfiStatusCodeValue = debug_support::EXCEPT_X64_DIVIDE_ERROR as u32;
pub const EFI_SW_EC_X64_DEBUG:            EfiStatusCodeValue = debug_support::EXCEPT_X64_DEBUG as u32;
pub const EFI_SW_EC_X64_NMI:              EfiStatusCodeValue = debug_support::EXCEPT_X64_NMI as u32;
pub const EFI_SW_EC_X64_BREAKPOINT:       EfiStatusCodeValue = debug_support::EXCEPT_X64_BREAKPOINT as u32;
pub const EFI_SW_EC_X64_OVERFLOW:         EfiStatusCodeValue = debug_support::EXCEPT_X64_OVERFLOW as u32;
pub const EFI_SW_EC_X64_BOUND:            EfiStatusCodeValue = debug_support::EXCEPT_X64_BOUND as u32;
pub const EFI_SW_EC_X64_INVALID_OPCODE:   EfiStatusCodeValue = debug_support::EXCEPT_X64_INVALID_OPCODE as u32;
pub const EFI_SW_EC_X64_DOUBLE_FAULT:     EfiStatusCodeValue = debug_support::EXCEPT_X64_DOUBLE_FAULT as u32;
pub const EFI_SW_EC_X64_INVALID_TSS:      EfiStatusCodeValue = debug_support::EXCEPT_X64_INVALID_TSS as u32;
pub const EFI_SW_EC_X64_SEG_NOT_PRESENT:  EfiStatusCodeValue = debug_support::EXCEPT_X64_SEG_NOT_PRESENT as u32;
pub const EFI_SW_EC_X64_STACK_FAULT:      EfiStatusCodeValue = debug_support::EXCEPT_X64_STACK_FAULT as u32;
pub const EFI_SW_EC_X64_GP_FAULT:         EfiStatusCodeValue = debug_support::EXCEPT_X64_GP_FAULT as u32;
pub const EFI_SW_EC_X64_PAGE_FAULT:       EfiStatusCodeValue = debug_support::EXCEPT_X64_PAGE_FAULT as u32;
pub const EFI_SW_EC_X64_FP_ERROR:         EfiStatusCodeValue = debug_support::EXCEPT_X64_FP_ERROR as u32;
pub const EFI_SW_EC_X64_ALIGNMENT_CHECK:  EfiStatusCodeValue = debug_support::EXCEPT_X64_ALIGNMENT_CHECK as u32;
pub const EFI_SW_EC_X64_MACHINE_CHECK:    EfiStatusCodeValue = debug_support::EXCEPT_X64_MACHINE_CHECK as u32;
pub const EFI_SW_EC_X64_SIMD:             EfiStatusCodeValue = debug_support::EXCEPT_X64_SIMD as u32;

// Software Class ARM Exception Subclass Error Code definitions.
// These exceptions are derived from the debug protocol
// definitions in the EFI specification.
//
pub const EFI_SW_EC_ARM_RESET:                  EfiStatusCodeValue = debug_support::EXCEPT_ARM_RESET as u32;
pub const EFI_SW_EC_ARM_UNDEFINED_INSTRUCTION:  EfiStatusCodeValue = debug_support::EXCEPT_ARM_UNDEFINED_INSTRUCTION as u32;
pub const EFI_SW_EC_ARM_SOFTWARE_INTERRUPT:     EfiStatusCodeValue = debug_support::EXCEPT_ARM_SOFTWARE_INTERRUPT as u32;
pub const EFI_SW_EC_ARM_PREFETCH_ABORT:         EfiStatusCodeValue = debug_support::EXCEPT_ARM_PREFETCH_ABORT as u32;
pub const EFI_SW_EC_ARM_DATA_ABORT:             EfiStatusCodeValue = debug_support::EXCEPT_ARM_DATA_ABORT as u32;
pub const EFI_SW_EC_ARM_RESERVED:               EfiStatusCodeValue = debug_support::EXCEPT_ARM_RESERVED as u32;
pub const EFI_SW_EC_ARM_IRQ:                    EfiStatusCodeValue = debug_support::EXCEPT_ARM_IRQ as u32;
pub const EFI_SW_EC_ARM_FIQ:                    EfiStatusCodeValue = debug_support::EXCEPT_ARM_FIQ as u32;

