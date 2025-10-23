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

use crate::pi::protocols::status_code::{EfiStatusCodeType, EfiStatusCodeValue};
// Required for IA32, X64, IPF, ARM and EBC defines for CPU exception types
use r_efi::efi::protocols::debug_support;

// A Status Code Type is made up of the code type and severity.
// All values masked by EFI_STATUS_CODE_RESERVED_MASK are
// reserved for use by this specification.
//
/// Mask for extracting status code type.
pub const EFI_STATUS_CODE_TYPE_MASK:      EfiStatusCodeType = 0x000000FF;
/// Mask for extracting severity level.
pub const EFI_STATUS_CODE_SEVERITY_MASK:  EfiStatusCodeType = 0xFF000000;
/// Mask for reserved bits.
pub const EFI_STATUS_CODE_RESERVED_MASK:  EfiStatusCodeType = 0x00FFFF00;

// Definition of code types. All other values masked by
// EFI_STATUS_CODE_TYPE_MASK are reserved for use by
// this specification.
//
/// Progress code type.
pub const EFI_PROGRESS_CODE:  EfiStatusCodeType = 0x00000001;
/// Error code type.
pub const EFI_ERROR_CODE:     EfiStatusCodeType = 0x00000002;
/// Debug code type.
pub const EFI_DEBUG_CODE:     EfiStatusCodeType = 0x00000003;

// Definitions of severities, all other values masked by
// EFI_STATUS_CODE_SEVERITY_MASK are reserved for use by
// this specification.
// Uncontained errors are major errors that could not contained
// to the specific component that is reporting the error.
// For example, if a memory error was not detected early enough,
// the bad data could be consumed by other drivers.
//
/// Minor error severity.
pub const EFI_ERROR_MINOR:        EfiStatusCodeType = 0x40000000;
/// Major error severity.
pub const EFI_ERROR_MAJOR:        EfiStatusCodeType = 0x80000000;
/// Unrecovered error severity.
pub const EFI_ERROR_UNRECOVERED:  EfiStatusCodeType = 0x90000000;
/// Uncontained error severity.
pub const EFI_ERROR_UNCONTAINED:  EfiStatusCodeType = 0xa0000000;

// A Status Code Value is made up of the class, subclass, and
// an operation.
//
/// Mask for extracting class code.
pub const EFI_STATUS_CODE_CLASS_MASK:      EfiStatusCodeValue = 0xFF000000;
/// Mask for extracting subclass code.
pub const EFI_STATUS_CODE_SUBCLASS_MASK:   EfiStatusCodeValue = 0x00FF0000;
/// Mask for extracting operation code.
pub const EFI_STATUS_CODE_OPERATION_MASK:  EfiStatusCodeValue = 0x0000FFFF;

// General partitioning scheme for Progress and Error Codes are:
//   - 0x0000-0x0FFF    Shared by all sub-classes in a given class.
//   - 0x1000-0x7FFF    Subclass Specific.
//   - 0x8000-0xFFFF    OEM specific.
//
/// Subclass-specific operation code.
pub const EFI_SUBCLASS_SPECIFIC:  EfiStatusCodeValue = 0x1000;
/// OEM-specific operation code.
pub const EFI_OEM_SPECIFIC:       EfiStatusCodeValue = 0x8000;

// Debug Code definitions for all classes and subclass.
// Only one debug code is defined at this point and should
// be used for anything that is sent to the debug stream.
//
/// Unspecified data class.
pub const EFI_DC_UNSPECIFIED:  EfiStatusCodeValue = 0x0;

// Class definitions.
// Values of 4-127 are reserved for future use by this specification.
// Values in the range 127-255 are reserved for OEM use.
//
/// Computing unit device class.
pub const EFI_COMPUTING_UNIT:  EfiStatusCodeValue = 0x00000000;
/// Peripheral device class.
pub const EFI_PERIPHERAL:      EfiStatusCodeValue = 0x01000000;
/// I/O bus device class.
pub const EFI_IO_BUS:          EfiStatusCodeValue = 0x02000000;
/// Software class.
pub const EFI_SOFTWARE:        EfiStatusCodeValue = 0x03000000;

// Computing Unit Subclass definitions.
// Values of 8-127 are reserved for future use by this specification.
// Values of 128-255 are reserved for OEM use.
//
/// Computing unit unspecified status code
pub const EFI_COMPUTING_UNIT_UNSPECIFIED:         EfiStatusCodeValue = EFI_COMPUTING_UNIT;
/// Host processor computing unit.
pub const EFI_COMPUTING_UNIT_HOST_PROCESSOR:      EfiStatusCodeValue = EFI_COMPUTING_UNIT | 0x00010000;
/// Firmware processor computing unit.
pub const EFI_COMPUTING_UNIT_FIRMWARE_PROCESSOR:  EfiStatusCodeValue = EFI_COMPUTING_UNIT | 0x00020000;
/// I/O processor computing unit.
pub const EFI_COMPUTING_UNIT_IO_PROCESSOR:        EfiStatusCodeValue = EFI_COMPUTING_UNIT | 0x00030000;
/// Cache computing unit.
pub const EFI_COMPUTING_UNIT_CACHE:               EfiStatusCodeValue = EFI_COMPUTING_UNIT | 0x00040000;
/// Memory computing unit.
pub const EFI_COMPUTING_UNIT_MEMORY:              EfiStatusCodeValue = EFI_COMPUTING_UNIT | 0x00050000;
/// Chipset computing unit.
pub const EFI_COMPUTING_UNIT_CHIPSET:             EfiStatusCodeValue = EFI_COMPUTING_UNIT | 0x00060000;

// Computing Unit Class Progress Code definitions.
// These are shared by all subclasses.
//
/// Computing unit initialization begin.
pub const EFI_CU_PC_INIT_BEGIN:  EfiStatusCodeValue = 0x00000000;
/// Computing unit initialization end.
pub const EFI_CU_PC_INIT_END:    EfiStatusCodeValue = 0x00000001;

// Computing Unit Unspecified Subclass Progress Code definitions.
//

// Computing Unit Host Processor Subclass Progress Code definitions.
//
/// Host processor power-on initialization progress code
pub const EFI_CU_HP_PC_POWER_ON_INIT:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Host processor cache initialization.
pub const EFI_CU_HP_PC_CACHE_INIT:              EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// Host processor RAM initialization.
pub const EFI_CU_HP_PC_RAM_INIT:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// Host processor memory controller initialization.
pub const EFI_CU_HP_PC_MEMORY_CONTROLLER_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// Host processor I/O initialization.
pub const EFI_CU_HP_PC_IO_INIT:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
/// Host processor BSP selection.
pub const EFI_CU_HP_PC_BSP_SELECT:              EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
/// Host processor BSP reselection.
pub const EFI_CU_HP_PC_BSP_RESELECT:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
/// Host processor AP initialization.
pub const EFI_CU_HP_PC_AP_INIT:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
/// Host processor SMM initialization.
pub const EFI_CU_HP_PC_SMM_INIT:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;

// Computing Unit Firmware Processor Subclass Progress Code definitions.
//

// Computing Unit IO Processor Subclass Progress Code definitions.
//

// Computing Unit Cache Subclass Progress Code definitions.
//
/// Cache presence detect progress code
pub const EFI_CU_CACHE_PC_PRESENCE_DETECT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Cache configuration.
pub const EFI_CU_CACHE_PC_CONFIGURATION:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;

// Computing Unit Memory Subclass Progress Code definitions.
//
/// Memory SPD read progress code
pub const EFI_CU_MEMORY_PC_SPD_READ:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Memory presence detection.
pub const EFI_CU_MEMORY_PC_PRESENCE_DETECT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// Memory timing configuration.
pub const EFI_CU_MEMORY_PC_TIMING:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// Memory configuration.
pub const EFI_CU_MEMORY_PC_CONFIGURING:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// Memory optimization.
pub const EFI_CU_MEMORY_PC_OPTIMIZING:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
/// Memory initialization.
pub const EFI_CU_MEMORY_PC_INIT:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
/// Memory testing.
pub const EFI_CU_MEMORY_PC_TEST:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;

// Computing Unit Chipset Subclass Progress Code definitions.
//

// South Bridge initialization prior to memory detection.
//
/// PEI CAR southbridge initialization progress code
pub const EFI_CHIPSET_PC_PEI_CAR_SB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;

// North Bridge initialization prior to memory detection.
//
/// PEI CAR northbridge initialization progress code
pub const EFI_CHIPSET_PC_PEI_CAR_NB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000001;

// South Bridge initialization after memory detection.
//
/// PEI memory southbridge initialization progress code
pub const EFI_CHIPSET_PC_PEI_MEM_SB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000002;

// North Bridge initialization after memory detection.
//
/// PEI memory northbridge initialization progress code
pub const EFI_CHIPSET_PC_PEI_MEM_NB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000003;

// PCI Host Bridge DXE initialization.
//
/// DXE hostbridge initialization progress code
pub const EFI_CHIPSET_PC_DXE_HB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000004;

// North Bridge DXE initialization.
//
/// DXE northbridge initialization progress code
pub const EFI_CHIPSET_PC_DXE_NB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000005;

// North Bridge specific SMM initialization in DXE.
//
/// DXE northbridge SMM initialization progress code
pub const EFI_CHIPSET_PC_DXE_NB_SMM_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000006;

// Initialization of the South Bridge specific UEFI Runtime Services.
//
/// DXE southbridge runtime initialization progress code
pub const EFI_CHIPSET_PC_DXE_SB_RT_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000007;

// South Bridge DXE initialization
//
/// DXE southbridge initialization progress code
pub const EFI_CHIPSET_PC_DXE_SB_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000008;

// South Bridge specific SMM initialization in DXE.
//
/// DXE southbridge SMM initialization progress code
pub const EFI_CHIPSET_PC_DXE_SB_SMM_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x00000009;

// Initialization of the South Bridge devices.
//
/// DXE southbridge devices initialization progress code
pub const EFI_CHIPSET_PC_DXE_SB_DEVICES_INIT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC|0x0000000a;

// Computing Unit Class Error Code definitions.
// These are shared by all subclasses.
//
/// Non-specific computing unit error.
pub const EFI_CU_EC_NON_SPECIFIC:    EfiStatusCodeValue = 0x00000000;
/// Computing unit disabled error.
pub const EFI_CU_EC_DISABLED:        EfiStatusCodeValue = 0x00000001;
/// Computing unit not supported error.
pub const EFI_CU_EC_NOT_SUPPORTED:   EfiStatusCodeValue = 0x00000002;
/// Computing unit not detected error.
pub const EFI_CU_EC_NOT_DETECTED:    EfiStatusCodeValue = 0x00000003;
/// Computing unit not configured error.
pub const EFI_CU_EC_NOT_CONFIGURED:  EfiStatusCodeValue = 0x00000004;

// Computing Unit Unspecified Subclass Error Code definitions.
//

// Computing Unit Host Processor Subclass Error Code definitions.
//
/// Host processor invalid type error code
pub const EFI_CU_HP_EC_INVALID_TYPE:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Host processor invalid speed error.
pub const EFI_CU_HP_EC_INVALID_SPEED:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// Host processor mismatch error.
pub const EFI_CU_HP_EC_MISMATCH:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// Host processor timer expired error.
pub const EFI_CU_HP_EC_TIMER_EXPIRED:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// Host processor self-test error.
pub const EFI_CU_HP_EC_SELF_TEST:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
/// Host processor internal error.
pub const EFI_CU_HP_EC_INTERNAL:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
/// Host processor thermal error.
pub const EFI_CU_HP_EC_THERMAL:              EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
/// Host processor low voltage error.
pub const EFI_CU_HP_EC_LOW_VOLTAGE:          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
/// Host processor high voltage error.
pub const EFI_CU_HP_EC_HIGH_VOLTAGE:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
/// Host processor cache error.
pub const EFI_CU_HP_EC_CACHE:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
/// Host processor microcode update error.
pub const EFI_CU_HP_EC_MICROCODE_UPDATE:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;
/// Host processor correctable error.
pub const EFI_CU_HP_EC_CORRECTABLE:          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000B;
/// Host processor uncorrectable error.
pub const EFI_CU_HP_EC_UNCORRECTABLE:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000C;
/// Host processor no microcode update error.
pub const EFI_CU_HP_EC_NO_MICROCODE_UPDATE:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000D;

// Computing Unit Firmware Processor Subclass Error Code definitions.
//
/// Firmware processor hard failure error.
pub const EFI_CU_FP_EC_HARD_FAIL:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Firmware processor soft failure error.
pub const EFI_CU_FP_EC_SOFT_FAIL:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// Firmware processor communication error.
pub const EFI_CU_FP_EC_COMM_ERROR:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;

// Computing Unit IO Processor Subclass Error Code definitions.
//

// Computing Unit Cache Subclass Error Code definitions.
//
/// Cache invalid type error code
pub const EFI_CU_CACHE_EC_INVALID_TYPE:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Cache invalid speed error.
pub const EFI_CU_CACHE_EC_INVALID_SPEED:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// Cache invalid size error.
pub const EFI_CU_CACHE_EC_INVALID_SIZE:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// Cache mismatch error.
pub const EFI_CU_CACHE_EC_MISMATCH:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;

// Computing Unit Memory Subclass Error Code definitions.
//
/// Memory invalid type error code
pub const EFI_CU_MEMORY_EC_INVALID_TYPE:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Memory invalid speed error.
pub const EFI_CU_MEMORY_EC_INVALID_SPEED:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// Memory correctable error.
pub const EFI_CU_MEMORY_EC_CORRECTABLE:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// Memory uncorrectable error.
pub const EFI_CU_MEMORY_EC_UNCORRECTABLE:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// Memory SPD failure error.
pub const EFI_CU_MEMORY_EC_SPD_FAIL:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
/// Memory invalid size error.
pub const EFI_CU_MEMORY_EC_INVALID_SIZE:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
/// Memory mismatch error.
pub const EFI_CU_MEMORY_EC_MISMATCH:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
/// Memory S3 resume failure error.
pub const EFI_CU_MEMORY_EC_S3_RESUME_FAIL:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
/// Memory update failure error.
pub const EFI_CU_MEMORY_EC_UPDATE_FAIL:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
/// Memory none detected error.
pub const EFI_CU_MEMORY_EC_NONE_DETECTED:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
/// Memory none useful error.
pub const EFI_CU_MEMORY_EC_NONE_USEFUL:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;

// Computing Unit Chipset Subclass Error Code definitions.
//
/// Chipset bad battery error code
pub const EFI_CHIPSET_EC_BAD_BATTERY:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Chipset DXE north bridge error.
pub const EFI_CHIPSET_EC_DXE_NB_ERROR:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// Chipset DXE south bridge error.
pub const EFI_CHIPSET_EC_DXE_SB_ERROR:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// Chipset intruder detected error.
pub const EFI_CHIPSET_EC_INTRUDER_DETECT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;

// Peripheral Subclass definitions.
// Values of 12-127 are reserved for future use by this specification.
// Values of 128-255 are reserved for OEM use.
//
/// Unspecified peripheral device.
pub const EFI_PERIPHERAL_UNSPECIFIED:      EfiStatusCodeValue = EFI_PERIPHERAL;
/// Keyboard peripheral device.
pub const EFI_PERIPHERAL_KEYBOARD:         EfiStatusCodeValue = EFI_PERIPHERAL | 0x00010000;
/// Mouse peripheral device.
pub const EFI_PERIPHERAL_MOUSE:            EfiStatusCodeValue = EFI_PERIPHERAL | 0x00020000;
/// Local console peripheral device.
pub const EFI_PERIPHERAL_LOCAL_CONSOLE:    EfiStatusCodeValue = EFI_PERIPHERAL | 0x00030000;
/// Remote console peripheral device.
pub const EFI_PERIPHERAL_REMOTE_CONSOLE:   EfiStatusCodeValue = EFI_PERIPHERAL | 0x00040000;
/// Serial port peripheral device.
pub const EFI_PERIPHERAL_SERIAL_PORT:      EfiStatusCodeValue = EFI_PERIPHERAL | 0x00050000;
/// Parallel port peripheral device.
pub const EFI_PERIPHERAL_PARALLEL_PORT:    EfiStatusCodeValue = EFI_PERIPHERAL | 0x00060000;
/// Fixed media peripheral device.
pub const EFI_PERIPHERAL_FIXED_MEDIA:      EfiStatusCodeValue = EFI_PERIPHERAL | 0x00070000;
/// Removable media peripheral device.
pub const EFI_PERIPHERAL_REMOVABLE_MEDIA:  EfiStatusCodeValue = EFI_PERIPHERAL | 0x00080000;
/// Audio input peripheral device.
pub const EFI_PERIPHERAL_AUDIO_INPUT:      EfiStatusCodeValue = EFI_PERIPHERAL | 0x00090000;
/// Audio output peripheral device.
pub const EFI_PERIPHERAL_AUDIO_OUTPUT:     EfiStatusCodeValue = EFI_PERIPHERAL | 0x000A0000;
/// LCD display peripheral device.
pub const EFI_PERIPHERAL_LCD_DEVICE:       EfiStatusCodeValue = EFI_PERIPHERAL | 0x000B0000;
/// Network peripheral device.
pub const EFI_PERIPHERAL_NETWORK:          EfiStatusCodeValue = EFI_PERIPHERAL | 0x000C0000;
/// Docking station peripheral device.
pub const EFI_PERIPHERAL_DOCKING:          EfiStatusCodeValue = EFI_PERIPHERAL | 0x000D0000;
/// TPM peripheral device.
pub const EFI_PERIPHERAL_TPM:              EfiStatusCodeValue = EFI_PERIPHERAL | 0x000E0000;

// Peripheral Class Progress Code definitions.
// These are shared by all subclasses.
//
/// Peripheral initialize progress code.
pub const EFI_P_PC_INIT:             EfiStatusCodeValue = 0x00000000;
/// Peripheral reset progress code.
pub const EFI_P_PC_RESET:            EfiStatusCodeValue = 0x00000001;
/// Peripheral disable progress code.
pub const EFI_P_PC_DISABLE:          EfiStatusCodeValue = 0x00000002;
/// Peripheral presence detect progress code.
pub const EFI_P_PC_PRESENCE_DETECT:  EfiStatusCodeValue = 0x00000003;
/// Peripheral enable progress code.
pub const EFI_P_PC_ENABLE:           EfiStatusCodeValue = 0x00000004;
/// Peripheral reconfigure progress code.
pub const EFI_P_PC_RECONFIG:         EfiStatusCodeValue = 0x00000005;
/// Peripheral detected progress code.
pub const EFI_P_PC_DETECTED:         EfiStatusCodeValue = 0x00000006;
/// Peripheral removed progress code.
pub const EFI_P_PC_REMOVED:          EfiStatusCodeValue = 0x00000007;

// Peripheral Class Unspecified Subclass Progress Code definitions.
//

// Peripheral Class Keyboard Subclass Progress Code definitions.
//
/// Keyboard clear buffer progress code.
pub const EFI_P_KEYBOARD_PC_CLEAR_BUFFER:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Keyboard self-test progress code.
pub const EFI_P_KEYBOARD_PC_SELF_TEST:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;

// Peripheral Class Mouse Subclass Progress Code definitions.
//
/// Mouse self-test progress code.
pub const EFI_P_MOUSE_PC_SELF_TEST:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;

// Peripheral Class Local Console Subclass Progress Code definitions.
//

// Peripheral Class Remote Console Subclass Progress Code definitions.
//

// Peripheral Class Serial Port Subclass Progress Code definitions.
//
/// Serial port clear buffer progress code.
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
/// Non-specific peripheral error.
pub const EFI_P_EC_NON_SPECIFIC:       EfiStatusCodeValue = 0x00000000;
/// Peripheral disabled error.
pub const EFI_P_EC_DISABLED:           EfiStatusCodeValue = 0x00000001;
/// Peripheral not supported error.
pub const EFI_P_EC_NOT_SUPPORTED:      EfiStatusCodeValue = 0x00000002;
/// Peripheral not detected error.
pub const EFI_P_EC_NOT_DETECTED:       EfiStatusCodeValue = 0x00000003;
/// Peripheral not configured error.
pub const EFI_P_EC_NOT_CONFIGURED:     EfiStatusCodeValue = 0x00000004;
/// Peripheral interface error.
pub const EFI_P_EC_INTERFACE_ERROR:    EfiStatusCodeValue = 0x00000005;
/// Peripheral controller error.
pub const EFI_P_EC_CONTROLLER_ERROR:   EfiStatusCodeValue = 0x00000006;
/// Peripheral input error.
pub const EFI_P_EC_INPUT_ERROR:        EfiStatusCodeValue = 0x00000007;
/// Peripheral output error.
pub const EFI_P_EC_OUTPUT_ERROR:       EfiStatusCodeValue = 0x00000008;
/// Peripheral resource conflict error.
pub const EFI_P_EC_RESOURCE_CONFLICT:  EfiStatusCodeValue = 0x00000009;

// Peripheral Class Unspecified Subclass Error Code definitions.
//

// Peripheral Class Keyboard Subclass Error Code definitions.
//
/// Keyboard locked error.
pub const EFI_P_KEYBOARD_EC_LOCKED:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Keyboard stuck key error.
pub const EFI_P_KEYBOARD_EC_STUCK_KEY:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// Keyboard buffer full error.
pub const EFI_P_KEYBOARD_EC_BUFFER_FULL:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;

// Peripheral Class Mouse Subclass Error Code definitions.
//
/// Mouse locked error.
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
/// Unspecified I/O bus.
pub const EFI_IO_BUS_UNSPECIFIED:  EfiStatusCodeValue = EFI_IO_BUS;
/// PCI I/O bus.
pub const EFI_IO_BUS_PCI:          EfiStatusCodeValue = EFI_IO_BUS | 0x00010000;
/// USB I/O bus.
pub const EFI_IO_BUS_USB:          EfiStatusCodeValue = EFI_IO_BUS | 0x00020000;
/// IBA I/O bus.
pub const EFI_IO_BUS_IBA:          EfiStatusCodeValue = EFI_IO_BUS | 0x00030000;
/// AGP I/O bus.
pub const EFI_IO_BUS_AGP:          EfiStatusCodeValue = EFI_IO_BUS | 0x00040000;
/// PC Card I/O bus.
pub const EFI_IO_BUS_PC_CARD:      EfiStatusCodeValue = EFI_IO_BUS | 0x00050000;
/// LPC I/O bus.
pub const EFI_IO_BUS_LPC:          EfiStatusCodeValue = EFI_IO_BUS | 0x00060000;
/// SCSI I/O bus.
pub const EFI_IO_BUS_SCSI:         EfiStatusCodeValue = EFI_IO_BUS | 0x00070000;
/// ATA/ATAPI I/O bus.
pub const EFI_IO_BUS_ATA_ATAPI:    EfiStatusCodeValue = EFI_IO_BUS | 0x00080000;
/// Fibre Channel I/O bus.
pub const EFI_IO_BUS_FC:           EfiStatusCodeValue = EFI_IO_BUS | 0x00090000;
/// IP Network I/O bus.
pub const EFI_IO_BUS_IP_NETWORK:   EfiStatusCodeValue = EFI_IO_BUS | 0x000A0000;
/// SMBus I/O bus.
pub const EFI_IO_BUS_SMBUS:        EfiStatusCodeValue = EFI_IO_BUS | 0x000B0000;
/// I2C I/O bus.
pub const EFI_IO_BUS_I2C:          EfiStatusCodeValue = EFI_IO_BUS | 0x000C0000;

// IO Bus Class Progress Code definitions.
// These are shared by all subclasses.
//
/// I/O bus initialize progress code.
pub const EFI_IOB_PC_INIT:      EfiStatusCodeValue = 0x00000000;
/// I/O bus reset progress code.
pub const EFI_IOB_PC_RESET:     EfiStatusCodeValue = 0x00000001;
/// I/O bus disable progress code.
pub const EFI_IOB_PC_DISABLE:   EfiStatusCodeValue = 0x00000002;
/// I/O bus detect progress code.
pub const EFI_IOB_PC_DETECT:    EfiStatusCodeValue = 0x00000003;
/// I/O bus enable progress code.
pub const EFI_IOB_PC_ENABLE:    EfiStatusCodeValue = 0x00000004;
/// I/O bus reconfigure progress code.
pub const EFI_IOB_PC_RECONFIG:  EfiStatusCodeValue = 0x00000005;
/// I/O bus hotplug progress code.
pub const EFI_IOB_PC_HOTPLUG:   EfiStatusCodeValue = 0x00000006;

// IO Bus Class Unspecified Subclass Progress Code definitions.
//

// IO Bus Class PCI Subclass Progress Code definitions.
//
/// PCI bus enumeration progress code.
pub const EFI_IOB_PCI_BUS_ENUM:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// PCI resource allocation progress code.
pub const EFI_IOB_PCI_RES_ALLOC:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// PCI hot-plug controller initialization progress code.
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
/// ATA bus SMART enable progress code.
pub const EFI_IOB_ATA_BUS_SMART_ENABLE:          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// ATA bus SMART disable progress code.
pub const EFI_IOB_ATA_BUS_SMART_DISABLE:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// ATA bus SMART over threshold progress code.
pub const EFI_IOB_ATA_BUS_SMART_OVERTHRESHOLD:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// ATA bus SMART under threshold progress code.
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
/// Non-specific I/O bus error.
pub const EFI_IOB_EC_NON_SPECIFIC:       EfiStatusCodeValue = 0x00000000;
/// I/O bus disabled error.
pub const EFI_IOB_EC_DISABLED:           EfiStatusCodeValue = 0x00000001;
/// I/O bus not supported error.
pub const EFI_IOB_EC_NOT_SUPPORTED:      EfiStatusCodeValue = 0x00000002;
/// I/O bus not detected error.
pub const EFI_IOB_EC_NOT_DETECTED:       EfiStatusCodeValue = 0x00000003;
/// I/O bus not configured error.
pub const EFI_IOB_EC_NOT_CONFIGURED:     EfiStatusCodeValue = 0x00000004;
/// I/O bus interface error.
pub const EFI_IOB_EC_INTERFACE_ERROR:    EfiStatusCodeValue = 0x00000005;
/// I/O bus controller error.
pub const EFI_IOB_EC_CONTROLLER_ERROR:   EfiStatusCodeValue = 0x00000006;
/// I/O bus read error.
pub const EFI_IOB_EC_READ_ERROR:         EfiStatusCodeValue = 0x00000007;
/// I/O bus write error.
pub const EFI_IOB_EC_WRITE_ERROR:        EfiStatusCodeValue = 0x00000008;
/// I/O bus resource conflict error.
pub const EFI_IOB_EC_RESOURCE_CONFLICT:  EfiStatusCodeValue = 0x00000009;

// IO Bus Class Unspecified Subclass Error Code definitions.
//

// IO Bus Class PCI Subclass Error Code definitions.
//
/// PCI parity error.
pub const EFI_IOB_PCI_EC_PERR:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// PCI system error.
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
/// ATA bus SMART not supported error.
pub const EFI_IOB_ATA_BUS_SMART_NOTSUPPORTED:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// ATA bus SMART disabled error.
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
/// Unspecified software subclass.
pub const EFI_SOFTWARE_UNSPECIFIED:          EfiStatusCodeValue = EFI_SOFTWARE;
/// SEC software subclass.
pub const EFI_SOFTWARE_SEC:                  EfiStatusCodeValue = EFI_SOFTWARE | 0x00010000;
/// PEI Core software subclass.
pub const EFI_SOFTWARE_PEI_CORE:             EfiStatusCodeValue = EFI_SOFTWARE | 0x00020000;
/// PEI Module software subclass.
pub const EFI_SOFTWARE_PEI_MODULE:           EfiStatusCodeValue = EFI_SOFTWARE | 0x00030000;
/// DXE Core software subclass.
pub const EFI_SOFTWARE_DXE_CORE:             EfiStatusCodeValue = EFI_SOFTWARE | 0x00040000;
/// DXE BS Driver software subclass.
pub const EFI_SOFTWARE_DXE_BS_DRIVER:        EfiStatusCodeValue = EFI_SOFTWARE | 0x00050000;
/// DXE RT Driver software subclass.
pub const EFI_SOFTWARE_DXE_RT_DRIVER:        EfiStatusCodeValue = EFI_SOFTWARE | 0x00060000;
/// SMM Driver software subclass.
pub const EFI_SOFTWARE_SMM_DRIVER:           EfiStatusCodeValue = EFI_SOFTWARE | 0x00070000;
/// EFI Application software subclass.
pub const EFI_SOFTWARE_EFI_APPLICATION:      EfiStatusCodeValue = EFI_SOFTWARE | 0x00080000;
/// EFI OS Loader software subclass.
pub const EFI_SOFTWARE_EFI_OS_LOADER:        EfiStatusCodeValue = EFI_SOFTWARE | 0x00090000;
/// Runtime software subclass.
pub const EFI_SOFTWARE_RT:                   EfiStatusCodeValue = EFI_SOFTWARE | 0x000A0000;
/// Application level software subclass.
pub const EFI_SOFTWARE_AL:                   EfiStatusCodeValue = EFI_SOFTWARE | 0x000B0000;
/// EBC Exception software subclass.
pub const EFI_SOFTWARE_EBC_EXCEPTION:        EfiStatusCodeValue = EFI_SOFTWARE | 0x000C0000;
/// IA32 Exception software subclass.
pub const EFI_SOFTWARE_IA32_EXCEPTION:       EfiStatusCodeValue = EFI_SOFTWARE | 0x000D0000;
/// IPF Exception software subclass.
pub const EFI_SOFTWARE_IPF_EXCEPTION:        EfiStatusCodeValue = EFI_SOFTWARE | 0x000E0000;
/// PEI Service software subclass.
pub const EFI_SOFTWARE_PEI_SERVICE:          EfiStatusCodeValue = EFI_SOFTWARE | 0x000F0000;
/// EFI Boot Service software subclass.
pub const EFI_SOFTWARE_EFI_BOOT_SERVICE:     EfiStatusCodeValue = EFI_SOFTWARE | 0x00100000;
/// EFI Runtime Service software subclass.
pub const EFI_SOFTWARE_EFI_RUNTIME_SERVICE:  EfiStatusCodeValue = EFI_SOFTWARE | 0x00110000;
/// EFI DXE Service software subclass.
pub const EFI_SOFTWARE_EFI_DXE_SERVICE:      EfiStatusCodeValue = EFI_SOFTWARE | 0x00120000;
/// X64 Exception software subclass.
pub const EFI_SOFTWARE_X64_EXCEPTION:        EfiStatusCodeValue = EFI_SOFTWARE | 0x00130000;
/// ARM Exception software subclass.
pub const EFI_SOFTWARE_ARM_EXCEPTION:        EfiStatusCodeValue = EFI_SOFTWARE | 0x00140000;


// Software Class Progress Code definitions.
// These are shared by all subclasses.
//
/// Software initialize progress code.
pub const EFI_SW_PC_INIT:                EfiStatusCodeValue = 0x00000000;
/// Software load progress code.
pub const EFI_SW_PC_LOAD:                EfiStatusCodeValue = 0x00000001;
/// Software initialize begin progress code.
pub const EFI_SW_PC_INIT_BEGIN:          EfiStatusCodeValue = 0x00000002;
/// Software initialize end progress code.
pub const EFI_SW_PC_INIT_END:            EfiStatusCodeValue = 0x00000003;
/// Software authenticate begin progress code.
pub const EFI_SW_PC_AUTHENTICATE_BEGIN:  EfiStatusCodeValue = 0x00000004;
/// Software authenticate end progress code.
pub const EFI_SW_PC_AUTHENTICATE_END:    EfiStatusCodeValue = 0x00000005;
/// Software input wait progress code.
pub const EFI_SW_PC_INPUT_WAIT:          EfiStatusCodeValue = 0x00000006;
/// Software user setup progress code.
pub const EFI_SW_PC_USER_SETUP:          EfiStatusCodeValue = 0x00000007;

// Software Class Unspecified Subclass Progress Code definitions.
//

// Software Class SEC Subclass Progress Code definitions.
//
/// SEC entry point progress code.
pub const EFI_SW_SEC_PC_ENTRY_POINT:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// SEC handoff to next progress code.
pub const EFI_SW_SEC_PC_HANDOFF_TO_NEXT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;

// Software Class PEI Core Subclass Progress Code definitions.
//
/// PEI Core entry point progress code.
pub const EFI_SW_PEI_CORE_PC_ENTRY_POINT:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// PEI Core handoff to next progress code.
pub const EFI_SW_PEI_CORE_PC_HANDOFF_TO_NEXT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// PEI Core return to last progress code.
pub const EFI_SW_PEI_CORE_PC_RETURN_TO_LAST:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;

// Software Class PEI Module Subclass Progress Code definitions.
//
/// PEI recovery begin progress code.
pub const EFI_SW_PEI_PC_RECOVERY_BEGIN:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// PEI capsule load progress code.
pub const EFI_SW_PEI_PC_CAPSULE_LOAD:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// PEI capsule start progress code.
pub const EFI_SW_PEI_PC_CAPSULE_START:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// PEI recovery user progress code.
pub const EFI_SW_PEI_PC_RECOVERY_USER:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// PEI recovery auto progress code.
pub const EFI_SW_PEI_PC_RECOVERY_AUTO:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
/// PEI S3 boot script progress code.
pub const EFI_SW_PEI_PC_S3_BOOT_SCRIPT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
/// PEI OS wake progress code.
pub const EFI_SW_PEI_PC_OS_WAKE:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
/// PEI S3 started progress code.
pub const EFI_SW_PEI_PC_S3_STARTED:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;

// Software Class DXE Core Subclass Progress Code definitions.
//
/// DXE Core entry point progress code.
pub const EFI_SW_DXE_CORE_PC_ENTRY_POINT:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// DXE Core handoff to next progress code.
pub const EFI_SW_DXE_CORE_PC_HANDOFF_TO_NEXT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// DXE Core return to last progress code.
pub const EFI_SW_DXE_CORE_PC_RETURN_TO_LAST:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// DXE Core start driver progress code.
pub const EFI_SW_DXE_CORE_PC_START_DRIVER:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// DXE Core architecture ready progress code.
pub const EFI_SW_DXE_CORE_PC_ARCH_READY:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;

// Software Class DXE BS Driver Subclass Progress Code definitions.
//
/// DXE BS legacy option ROM init progress code.
pub const EFI_SW_DXE_BS_PC_LEGACY_OPROM_INIT:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// DXE BS ready to boot event progress code.
pub const EFI_SW_DXE_BS_PC_READY_TO_BOOT_EVENT:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// DXE BS legacy boot event progress code.
pub const EFI_SW_DXE_BS_PC_LEGACY_BOOT_EVENT:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// DXE BS exit boot services event progress code.
pub const EFI_SW_DXE_BS_PC_EXIT_BOOT_SERVICES_EVENT:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// DXE BS virtual address change event progress code.
pub const EFI_SW_DXE_BS_PC_VIRTUAL_ADDRESS_CHANGE_EVENT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
/// DXE BS variable services init progress code.
pub const EFI_SW_DXE_BS_PC_VARIABLE_SERVICES_INIT:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
/// DXE BS variable reclaim progress code.
pub const EFI_SW_DXE_BS_PC_VARIABLE_RECLAIM:              EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
/// DXE BS attempt boot order event progress code.
pub const EFI_SW_DXE_BS_PC_ATTEMPT_BOOT_ORDER_EVENT:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
/// DXE BS config reset progress code.
pub const EFI_SW_DXE_BS_PC_CONFIG_RESET:                  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
/// DXE BS CSM init progress code.
pub const EFI_SW_DXE_BS_PC_CSM_INIT:                      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
/// DXE BS boot option complete progress code.
pub const EFI_SW_DXE_BS_PC_BOOT_OPTION_COMPLETE:          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;   // MU_CHANGE

// Software Class SMM Driver Subclass Progress Code definitions.
//

// Software Class EFI Application Subclass Progress Code definitions.
//

// Software Class EFI OS Loader Subclass Progress Code definitions.
//

// Software Class EFI RT Subclass Progress Code definitions.
//
/// Runtime entry point progress code.
pub const EFI_SW_RT_PC_ENTRY_POINT:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Runtime handoff to next progress code.
pub const EFI_SW_RT_PC_HANDOFF_TO_NEXT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// Runtime return to last progress code.
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
/// PEI Services install PPI progress code.
pub const EFI_SW_PS_PC_INSTALL_PPI:              EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// PEI Services reinstall PPI progress code.
pub const EFI_SW_PS_PC_REINSTALL_PPI:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// PEI Services locate PPI progress code.
pub const EFI_SW_PS_PC_LOCATE_PPI:               EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// PEI Services notify PPI progress code.
pub const EFI_SW_PS_PC_NOTIFY_PPI:               EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// PEI Services get boot mode progress code.
pub const EFI_SW_PS_PC_GET_BOOT_MODE:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
/// PEI Services set boot mode progress code.
pub const EFI_SW_PS_PC_SET_BOOT_MODE:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
/// PEI Services get HOB list progress code.
pub const EFI_SW_PS_PC_GET_HOB_LIST:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
/// PEI Services create HOB progress code.
pub const EFI_SW_PS_PC_CREATE_HOB:               EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
/// PEI Services FFS find next volume progress code.
pub const EFI_SW_PS_PC_FFS_FIND_NEXT_VOLUME:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
/// PEI Services FFS find next file progress code.
pub const EFI_SW_PS_PC_FFS_FIND_NEXT_FILE:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
/// PEI Services FFS find section data progress code.
pub const EFI_SW_PS_PC_FFS_FIND_SECTION_DATA:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;
/// PEI Services install PEI memory progress code.
pub const EFI_SW_PS_PC_INSTALL_PEI_MEMORY:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000B;
/// PEI Services allocate pages progress code.
pub const EFI_SW_PS_PC_ALLOCATE_PAGES:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000C;
/// PEI Services allocate pool progress code.
pub const EFI_SW_PS_PC_ALLOCATE_POOL:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000D;
/// PEI Services copy memory progress code.
pub const EFI_SW_PS_PC_COPY_MEM:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000E;
/// PEI Services set memory progress code.
pub const EFI_SW_PS_PC_SET_MEM:                  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000F;
/// PEI Services reset system progress code.
pub const EFI_SW_PS_PC_RESET_SYSTEM:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000010;
/// PEI Services FFS find file by name progress code.
pub const EFI_SW_PS_PC_FFS_FIND_FILE_BY_NAME:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000013;
/// PEI Services FFS get file info progress code.
pub const EFI_SW_PS_PC_FFS_GET_FILE_INFO:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000014;
/// PEI Services FFS get volume info progress code.
pub const EFI_SW_PS_PC_FFS_GET_VOLUME_INFO:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000015;
/// PEI Services FFS register for shadow progress code.
pub const EFI_SW_PS_PC_FFS_REGISTER_FOR_SHADOW:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000016;

// Software Class EFI Boot Services Subclass Progress Code definitions.
//
/// Boot Services RaiseTPL progress code.
pub const EFI_SW_BS_PC_RAISE_TPL:                      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Boot Services RestoreTPL progress code.
pub const EFI_SW_BS_PC_RESTORE_TPL:                    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// Boot Services AllocatePages progress code.
pub const EFI_SW_BS_PC_ALLOCATE_PAGES:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// Boot Services FreePages progress code.
pub const EFI_SW_BS_PC_FREE_PAGES:                     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// Boot Services GetMemoryMap progress code.
pub const EFI_SW_BS_PC_GET_MEMORY_MAP:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
/// Boot Services AllocatePool progress code.
pub const EFI_SW_BS_PC_ALLOCATE_POOL:                  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
/// Boot Services FreePool progress code.
pub const EFI_SW_BS_PC_FREE_POOL:                      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
/// Boot Services CreateEvent progress code.
pub const EFI_SW_BS_PC_CREATE_EVENT:                   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
/// Boot Services SetTimer progress code.
pub const EFI_SW_BS_PC_SET_TIMER:                      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
/// Boot Services WaitForEvent progress code.
pub const EFI_SW_BS_PC_WAIT_FOR_EVENT:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
/// Boot Services SignalEvent progress code.
pub const EFI_SW_BS_PC_SIGNAL_EVENT:                   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;
/// Boot Services CloseEvent progress code.
pub const EFI_SW_BS_PC_CLOSE_EVENT:                    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000B;
/// Boot Services CheckEvent progress code.
pub const EFI_SW_BS_PC_CHECK_EVENT:                    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000C;
/// Boot Services InstallProtocolInterface progress code.
pub const EFI_SW_BS_PC_INSTALL_PROTOCOL_INTERFACE:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000D;
/// Boot Services ReinstallProtocolInterface progress code.
pub const EFI_SW_BS_PC_REINSTALL_PROTOCOL_INTERFACE:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000E;
/// Boot Services UninstallProtocolInterface progress code.
pub const EFI_SW_BS_PC_UNINSTALL_PROTOCOL_INTERFACE:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000F;
/// Boot Services HandleProtocol progress code.
pub const EFI_SW_BS_PC_HANDLE_PROTOCOL:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000010;
/// Boot Services PCHandleProtocol progress code.
pub const EFI_SW_BS_PC_PC_HANDLE_PROTOCOL:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000011;
/// Boot Services HandleProtocol progress code.
pub const EFI_SW_BS_PC_REGISTER_PROTOCOL_NOTIFY:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000012;
/// Boot Services LocateHandle progress code.
pub const EFI_SW_BS_PC_LOCATE_HANDLE:                  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000013;
/// Boot Services InstallConfigurationTable progress code.
pub const EFI_SW_BS_PC_INSTALL_CONFIGURATION_TABLE:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000014;
/// Boot Services LoadImage progress code.
pub const EFI_SW_BS_PC_LOAD_IMAGE:                     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000015;
/// Boot Services StartImage progress code.
pub const EFI_SW_BS_PC_START_IMAGE:                    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000016;
/// Boot Services Exit progress code.
pub const EFI_SW_BS_PC_EXIT:                           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000017;
/// Boot Services UnloadImage progress code.
pub const EFI_SW_BS_PC_UNLOAD_IMAGE:                   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000018;
/// Boot Services ExitBootServices progress code.
pub const EFI_SW_BS_PC_EXIT_BOOT_SERVICES:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000019;
/// Boot Services GetNextMonotonicCount progress code.
pub const EFI_SW_BS_PC_GET_NEXT_MONOTONIC_COUNT:       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000001A;
/// Boot Services Stall progress code.
pub const EFI_SW_BS_PC_STALL:                          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000001B;
/// Boot Services SetWatchdogTimer progress code.
pub const EFI_SW_BS_PC_SET_WATCHDOG_TIMER:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000001C;
/// Boot Services ConnectController progress code.
pub const EFI_SW_BS_PC_CONNECT_CONTROLLER:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000001D;
/// Boot Services DisconnectController progress code.
pub const EFI_SW_BS_PC_DISCONNECT_CONTROLLER:          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000001E;
/// Boot Services OpenProtocol progress code.
pub const EFI_SW_BS_PC_OPEN_PROTOCOL:                  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000001F;
/// Boot Services CloseProtocol progress code.
pub const EFI_SW_BS_PC_CLOSE_PROTOCOL:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000020;
/// Boot Services OpenProtocolInformation progress code.
pub const EFI_SW_BS_PC_OPEN_PROTOCOL_INFORMATION:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000021;
/// Boot Services ProtocolsPerHandle progress code.
pub const EFI_SW_BS_PC_PROTOCOLS_PER_HANDLE:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000022;
/// Boot Services LocateHandleBuffer progress code.
pub const EFI_SW_BS_PC_LOCATE_HANDLE_BUFFER:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000023;
/// Boot Services LocateProtocol progress code.
pub const EFI_SW_BS_PC_LOCATE_PROTOCOL:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000024;
/// Boot Services InstallMultipleProtocolInterfaces progress code.
pub const EFI_SW_BS_PC_INSTALL_MULTIPLE_INTERFACES:    EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000025;
/// Boot Services UninstallMultipleProtocolInterfaces progress code.
pub const EFI_SW_BS_PC_UNINSTALL_MULTIPLE_INTERFACES:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000026;
/// Boot Services CalculateCrc32 progress code.
pub const EFI_SW_BS_PC_CALCULATE_CRC_32:               EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000027;
/// Boot Services CopyMem progress code.
pub const EFI_SW_BS_PC_COPY_MEM:                       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000028;
/// Boot Services SetMem progress code.
pub const EFI_SW_BS_PC_SET_MEM:                        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000029;
/// Boot Services CreateEventEx progress code.
pub const EFI_SW_BS_PC_CREATE_EVENT_EX:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000002A;

// Software Class EFI Runtime Services Subclass Progress Code definitions.
//
/// Runtime Services GetTime progress code.
pub const EFI_SW_RS_PC_GET_TIME:                       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Runtime Services SetTime progress code.
pub const EFI_SW_RS_PC_SET_TIME:                       EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// Runtime Services GetWakeupTime progress code.
pub const EFI_SW_RS_PC_GET_WAKEUP_TIME:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// Runtime Services SetWakeupTime progress code.
pub const EFI_SW_RS_PC_SET_WAKEUP_TIME:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// Runtime Services SetVirtualAddressMap progress code.
pub const EFI_SW_RS_PC_SET_VIRTUAL_ADDRESS_MAP:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
/// Runtime Services ConvertPointer progress code.
pub const EFI_SW_RS_PC_CONVERT_POINTER:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
/// Runtime Services GetVariable progress code.
pub const EFI_SW_RS_PC_GET_VARIABLE:                   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
/// Runtime Services GetNextVariableName progress code.
pub const EFI_SW_RS_PC_GET_NEXT_VARIABLE_NAME:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
/// Runtime Services SetVariable progress code.
pub const EFI_SW_RS_PC_SET_VARIABLE:                   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
/// Runtime Services GetNextHighMonotonicCount progress code.
pub const EFI_SW_RS_PC_GET_NEXT_HIGH_MONOTONIC_COUNT:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
/// Runtime Services ResetSystem progress code.
pub const EFI_SW_RS_PC_RESET_SYSTEM:                   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;
/// Runtime Services UpdateCapsule progress code.
pub const EFI_SW_RS_PC_UPDATE_CAPSULE:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000B;
/// Runtime Services QueryCapsuleCapabilities progress code.
pub const EFI_SW_RS_PC_QUERY_CAPSULE_CAPABILITIES:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000C;
/// Runtime Services QueryVariableInfo progress code.
pub const EFI_SW_RS_PC_QUERY_VARIABLE_INFO:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000D;

// Software Class EFI DXE Services Subclass Progress Code definitions
//
/// DXE Services AddMemorySpace progress code.
pub const EFI_SW_DS_PC_ADD_MEMORY_SPACE:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// DXE Services AllocateMemorySpace progress code.
pub const EFI_SW_DS_PC_ALLOCATE_MEMORY_SPACE:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// DXE Services FreeMemorySpace progress code.
pub const EFI_SW_DS_PC_FREE_MEMORY_SPACE:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// DXE Services RemoveMemorySpace progress code.
pub const EFI_SW_DS_PC_REMOVE_MEMORY_SPACE:          EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// DXE Services GetMemorySpaceDescriptor progress code.
pub const EFI_SW_DS_PC_GET_MEMORY_SPACE_DESCRIPTOR:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
/// DXE Services SetMemorySpaceAttributes progress code.
pub const EFI_SW_DS_PC_SET_MEMORY_SPACE_ATTRIBUTES:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
/// DXE Services GetMemorySpaceMap progress code.
pub const EFI_SW_DS_PC_GET_MEMORY_SPACE_MAP:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
/// DXE Services AddIoSpace progress code.
pub const EFI_SW_DS_PC_ADD_IO_SPACE:                 EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
/// DXE Services AllocateIoSpace progress code.
pub const EFI_SW_DS_PC_ALLOCATE_IO_SPACE:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
/// DXE Services FreeIoSpace progress code.
pub const EFI_SW_DS_PC_FREE_IO_SPACE:                EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;
/// DXE Services RemoveIoSpace progress code.
pub const EFI_SW_DS_PC_REMOVE_IO_SPACE:              EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000A;
/// DXE Services GetIoSpaceDescriptor progress code.
pub const EFI_SW_DS_PC_GET_IO_SPACE_DESCRIPTOR:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000B;
/// DXE Services GetIoSpaceMap progress code.
pub const EFI_SW_DS_PC_GET_IO_SPACE_MAP:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000C;
/// DXE Services Dispatch progress code.
pub const EFI_SW_DS_PC_DISPATCH:                     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000D;
/// DXE Services Schedule progress code.
pub const EFI_SW_DS_PC_SCHEDULE:                     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000E;
/// DXE Services Trust progress code.
pub const EFI_SW_DS_PC_TRUST:                        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x0000000F;
/// DXE Services ProcessFirmwareVolume progress code.
pub const EFI_SW_DS_PC_PROCESS_FIRMWARE_VOLUME:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000010;

// Software Class Error Code definitions.
// These are shared by all subclasses.
//
/// Non-specific software error.
pub const EFI_SW_EC_NON_SPECIFIC:                    EfiStatusCodeValue = 0x00000000;
/// Load error.
pub const EFI_SW_EC_LOAD_ERROR:                      EfiStatusCodeValue = 0x00000001;
/// Invalid parameter error.
pub const EFI_SW_EC_INVALID_PARAMETER:               EfiStatusCodeValue = 0x00000002;
/// Unsupported operation error.
pub const EFI_SW_EC_UNSUPPORTED:                     EfiStatusCodeValue = 0x00000003;
/// Invalid buffer error.
pub const EFI_SW_EC_INVALID_BUFFER:                  EfiStatusCodeValue = 0x00000004;
/// Out of resources error.
pub const EFI_SW_EC_OUT_OF_RESOURCES:                EfiStatusCodeValue = 0x00000005;
/// Operation aborted error.
pub const EFI_SW_EC_ABORTED:                         EfiStatusCodeValue = 0x00000006;
/// Illegal software state error.
pub const EFI_SW_EC_ILLEGAL_SOFTWARE_STATE:          EfiStatusCodeValue = 0x00000007;
/// Illegal hardware state error.
pub const EFI_SW_EC_ILLEGAL_HARDWARE_STATE:          EfiStatusCodeValue = 0x00000008;
/// Start error.
pub const EFI_SW_EC_START_ERROR:                     EfiStatusCodeValue = 0x00000009;
/// Bad date/time error.
pub const EFI_SW_EC_BAD_DATE_TIME:                   EfiStatusCodeValue = 0x0000000A;
/// Invalid configuration error.
pub const EFI_SW_EC_CFG_INVALID:                     EfiStatusCodeValue = 0x0000000B;
/// Configuration clear request.
pub const EFI_SW_EC_CFG_CLR_REQUEST:                 EfiStatusCodeValue = 0x0000000C;
/// Default configuration.
pub const EFI_SW_EC_CFG_DEFAULT:                     EfiStatusCodeValue = 0x0000000D;
/// Invalid password error.
pub const EFI_SW_EC_PWD_INVALID:                     EfiStatusCodeValue = 0x0000000E;
/// Password clear request.
pub const EFI_SW_EC_PWD_CLR_REQUEST:                 EfiStatusCodeValue = 0x0000000F;
/// Password cleared.
pub const EFI_SW_EC_PWD_CLEARED:                     EfiStatusCodeValue = 0x00000010;
/// Event log full error.
pub const EFI_SW_EC_EVENT_LOG_FULL:                  EfiStatusCodeValue = 0x00000011;
/// Write protected error.
pub const EFI_SW_EC_WRITE_PROTECTED:                 EfiStatusCodeValue = 0x00000012;
/// Firmware volume corrupted error.
pub const EFI_SW_EC_FV_CORRUPTED:                    EfiStatusCodeValue = 0x00000013;
/// Inconsistent memory map error.
pub const EFI_SW_EC_INCONSISTENT_MEMORY_MAP:         EfiStatusCodeValue = 0x00000014;

// Software Class Unspecified Subclass Error Code definitions.
//

// Software Class SEC Subclass Error Code definitions.
//

// Software Class PEI Core Subclass Error Code definitions.
//
/// DXE image corrupted error.
pub const EFI_SW_PEI_CORE_EC_DXE_CORRUPT:           EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// DXE IPL not found error.
pub const EFI_SW_PEI_CORE_EC_DXEIPL_NOT_FOUND:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// Memory not installed error.
pub const EFI_SW_PEI_CORE_EC_MEMORY_NOT_INSTALLED:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;

// Software Class PEI Module Subclass Error Code definitions.
//
/// No recovery capsule error.
pub const EFI_SW_PEI_EC_NO_RECOVERY_CAPSULE:         EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Invalid capsule descriptor error.
pub const EFI_SW_PEI_EC_INVALID_CAPSULE_DESCRIPTOR:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// S3 Resume PPI not found error.
pub const EFI_SW_PEI_EC_S3_RESUME_PPI_NOT_FOUND:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// S3 boot script error.
pub const EFI_SW_PEI_EC_S3_BOOT_SCRIPT_ERROR:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// S3 OS wake error.
pub const EFI_SW_PEI_EC_S3_OS_WAKE_ERROR:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
/// S3 resume failed error.
pub const EFI_SW_PEI_EC_S3_RESUME_FAILED:            EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
/// Recovery PPI not found error.
pub const EFI_SW_PEI_EC_RECOVERY_PPI_NOT_FOUND:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;
/// Recovery failed error.
pub const EFI_SW_PEI_EC_RECOVERY_FAILED:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000007;
/// S3 resume error.
pub const EFI_SW_PEI_EC_S3_RESUME_ERROR:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000008;
/// Invalid capsule error.
pub const EFI_SW_PEI_EC_INVALID_CAPSULE:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000009;

// Software Class DXE Foundation Subclass Error Code definitions.
//
/// No architecture protocol error.
pub const EFI_SW_DXE_CORE_EC_NO_ARCH:             EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Image load failure error.
pub const EFI_SW_DXE_CORE_EC_IMAGE_LOAD_FAILURE:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;    // MU_CHANGE

// Software Class DXE Boot Service Driver Subclass Error Code definitions.
//
/// Legacy option ROM no space error.
pub const EFI_SW_DXE_BS_EC_LEGACY_OPROM_NO_SPACE:   EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Invalid password error.
pub const EFI_SW_DXE_BS_EC_INVALID_PASSWORD:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// Boot option load error.
pub const EFI_SW_DXE_BS_EC_BOOT_OPTION_LOAD_ERROR:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// Boot option failed error.
pub const EFI_SW_DXE_BS_EC_BOOT_OPTION_FAILED:      EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// Invalid IDE password error.
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
/// EBC undefined exception.
pub const EFI_SW_EC_EBC_UNDEFINED:             EfiStatusCodeValue = 0x00000000;
/// EBC divide error exception.
pub const EFI_SW_EC_EBC_DIVIDE_ERROR:          EfiStatusCodeValue = debug_support::EXCEPT_EBC_DIVIDE_ERROR as u32;
/// EBC debug exception.
pub const EFI_SW_EC_EBC_DEBUG:                 EfiStatusCodeValue = debug_support::EXCEPT_EBC_DEBUG as u32;
/// EBC breakpoint exception.
pub const EFI_SW_EC_EBC_BREAKPOINT:            EfiStatusCodeValue = debug_support::EXCEPT_EBC_BREAKPOINT as u32;
/// EBC overflow exception.
pub const EFI_SW_EC_EBC_OVERFLOW:              EfiStatusCodeValue = debug_support::EXCEPT_EBC_OVERFLOW as u32;
/// EBC invalid opcode exception.
pub const EFI_SW_EC_EBC_INVALID_OPCODE:        EfiStatusCodeValue = debug_support::EXCEPT_EBC_INVALID_OPCODE as u32;
/// EBC stack fault exception.
pub const EFI_SW_EC_EBC_STACK_FAULT:           EfiStatusCodeValue = debug_support::EXCEPT_EBC_STACK_FAULT as u32;
/// EBC alignment check exception.
pub const EFI_SW_EC_EBC_ALIGNMENT_CHECK:       EfiStatusCodeValue = debug_support::EXCEPT_EBC_ALIGNMENT_CHECK as u32;
/// EBC instruction encoding exception.
pub const EFI_SW_EC_EBC_INSTRUCTION_ENCODING:  EfiStatusCodeValue = debug_support::EXCEPT_EBC_INSTRUCTION_ENCODING as u32;
/// EBC bad break exception.
pub const EFI_SW_EC_EBC_BAD_BREAK:             EfiStatusCodeValue = debug_support::EXCEPT_EBC_BAD_BREAK as u32;
/// EBC single step exception.
pub const EFI_SW_EC_EBC_STEP:                  EfiStatusCodeValue = debug_support::EXCEPT_EBC_SINGLE_STEP as u32;

// Software Class IA32 Exception Subclass Error Code definitions.
// These exceptions are derived from the debug protocol definitions in the EFI
// specification.
//
/// IA32 divide error exception.
pub const EFI_SW_EC_IA32_DIVIDE_ERROR:     EfiStatusCodeValue = debug_support::EXCEPT_IA32_DIVIDE_ERROR as u32;
/// IA32 debug exception.
pub const EFI_SW_EC_IA32_DEBUG:            EfiStatusCodeValue = debug_support::EXCEPT_IA32_DEBUG as u32;
/// IA32 NMI exception.
pub const EFI_SW_EC_IA32_NMI:              EfiStatusCodeValue = debug_support::EXCEPT_IA32_NMI as u32;
/// IA32 breakpoint exception.
pub const EFI_SW_EC_IA32_BREAKPOINT:       EfiStatusCodeValue = debug_support::EXCEPT_IA32_BREAKPOINT as u32;
/// IA32 overflow exception.
pub const EFI_SW_EC_IA32_OVERFLOW:         EfiStatusCodeValue = debug_support::EXCEPT_IA32_OVERFLOW as u32;
/// IA32 bound exception.
pub const EFI_SW_EC_IA32_BOUND:            EfiStatusCodeValue = debug_support::EXCEPT_IA32_BOUND as u32;
/// IA32 invalid opcode exception.
pub const EFI_SW_EC_IA32_INVALID_OPCODE:   EfiStatusCodeValue = debug_support::EXCEPT_IA32_INVALID_OPCODE as u32;
/// IA32 double fault exception.
pub const EFI_SW_EC_IA32_DOUBLE_FAULT:     EfiStatusCodeValue = debug_support::EXCEPT_IA32_DOUBLE_FAULT as u32;
/// IA32 invalid TSS exception.
pub const EFI_SW_EC_IA32_INVALID_TSS:      EfiStatusCodeValue = debug_support::EXCEPT_IA32_INVALID_TSS as u32;
/// IA32 segment not present exception.
pub const EFI_SW_EC_IA32_SEG_NOT_PRESENT:  EfiStatusCodeValue = debug_support::EXCEPT_IA32_SEG_NOT_PRESENT as u32;
/// IA32 stack fault exception.
pub const EFI_SW_EC_IA32_STACK_FAULT:      EfiStatusCodeValue = debug_support::EXCEPT_IA32_STACK_FAULT as u32;
/// IA32 general protection fault exception.
pub const EFI_SW_EC_IA32_GP_FAULT:         EfiStatusCodeValue = debug_support::EXCEPT_IA32_GP_FAULT as u32;
/// IA32 page fault exception.
pub const EFI_SW_EC_IA32_PAGE_FAULT:       EfiStatusCodeValue = debug_support::EXCEPT_IA32_PAGE_FAULT as u32;
/// IA32 floating-point error exception.
pub const EFI_SW_EC_IA32_FP_ERROR:         EfiStatusCodeValue = debug_support::EXCEPT_IA32_FP_ERROR as u32;
/// IA32 alignment check exception.
pub const EFI_SW_EC_IA32_ALIGNMENT_CHECK:  EfiStatusCodeValue = debug_support::EXCEPT_IA32_ALIGNMENT_CHECK as u32;
/// IA32 machine check exception.
pub const EFI_SW_EC_IA32_MACHINE_CHECK:    EfiStatusCodeValue = debug_support::EXCEPT_IA32_MACHINE_CHECK as u32;
/// IA32 SIMD exception.
pub const EFI_SW_EC_IA32_SIMD:             EfiStatusCodeValue = debug_support::EXCEPT_IA32_SIMD as u32;

// Software Class IPF Exception Subclass Error Code definitions.
// These exceptions are derived from the debug protocol definitions in the EFI
// specification.
//
/// IPF alternate data TLB exception.
pub const EFI_SW_EC_IPF_ALT_DTLB:            EfiStatusCodeValue = debug_support::EXCEPT_IPF_ALT_DATA_TLB as u32;
/// IPF data nested TLB exception.
pub const EFI_SW_EC_IPF_DNESTED_TLB:         EfiStatusCodeValue = debug_support::EXCEPT_IPF_DATA_NESTED_TLB as u32;
/// IPF breakpoint exception.
pub const EFI_SW_EC_IPF_BREAKPOINT:          EfiStatusCodeValue = debug_support::EXCEPT_IPF_BREAKPOINT as u32;
/// IPF external interrupt exception.
pub const EFI_SW_EC_IPF_EXTERNAL_INTERRUPT:  EfiStatusCodeValue = debug_support::EXCEPT_IPF_EXTERNAL_INTERRUPT as u32;
/// IPF general exception.
pub const EFI_SW_EC_IPF_GEN_EXCEPT:          EfiStatusCodeValue = debug_support::EXCEPT_IPF_GENERAL_EXCEPTION as u32;
/// IPF NAT consumption exception.
pub const EFI_SW_EC_IPF_NAT_CONSUMPTION:     EfiStatusCodeValue = debug_support::EXCEPT_IPF_NAT_CONSUMPTION as u32;
/// IPF debug exception.
pub const EFI_SW_EC_IPF_DEBUG_EXCEPT:        EfiStatusCodeValue = debug_support::EXCEPT_IPF_DEBUG as u32;
/// IPF unaligned access exception.
pub const EFI_SW_EC_IPF_UNALIGNED_ACCESS:    EfiStatusCodeValue = debug_support::EXCEPT_IPF_UNALIGNED_REFERENCE as u32;
/// IPF floating-point fault exception.
pub const EFI_SW_EC_IPF_FP_FAULT:            EfiStatusCodeValue = debug_support::EXCEPT_IPF_FP_FAULT as u32;
/// IPF floating-point trap exception.
pub const EFI_SW_EC_IPF_FP_TRAP:             EfiStatusCodeValue = debug_support::EXCEPT_IPF_FP_TRAP as u32;
/// IPF taken branch exception.
pub const EFI_SW_EC_IPF_TAKEN_BRANCH:        EfiStatusCodeValue = debug_support::EXCEPT_IPF_TAKEN_BRANCH as u32;
/// IPF single step exception.
pub const EFI_SW_EC_IPF_SINGLE_STEP:         EfiStatusCodeValue = debug_support::EXCEPT_IPF_SINGLE_STEP as u32;

// Software Class PEI Service Subclass Error Code definitions.
//
/// Reset not available error.
pub const EFI_SW_PS_EC_RESET_NOT_AVAILABLE:     EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// Memory installed twice error.
pub const EFI_SW_PS_EC_MEMORY_INSTALLED_TWICE:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;

// Software Class EFI Boot Service Subclass Error Code definitions.
//

// Software Class EFI Runtime Service Subclass Error Code definitions.
//

// Software Class EFI DXE Service Subclass Error Code definitions.
//
/// DXE BS begin connecting drivers progress code.
pub const EFI_SW_DXE_BS_PC_BEGIN_CONNECTING_DRIVERS:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;
/// DXE BS verifying password progress code.
pub const EFI_SW_DXE_BS_PC_VERIFYING_PASSWORD:        EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000006;

// Software Class DXE RT Driver Subclass Progress Code definitions.
//
/// ACPI S0 sleep state progress code.
pub const EFI_SW_DXE_RT_PC_S0:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC;
/// ACPI S1 sleep state progress code.
pub const EFI_SW_DXE_RT_PC_S1:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000001;
/// ACPI S2 sleep state progress code.
pub const EFI_SW_DXE_RT_PC_S2:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000002;
/// ACPI S3 sleep state progress code.
pub const EFI_SW_DXE_RT_PC_S3:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000003;
/// ACPI S4 sleep state progress code.
pub const EFI_SW_DXE_RT_PC_S4:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000004;
/// ACPI S5 sleep state progress code.
pub const EFI_SW_DXE_RT_PC_S5:  EfiStatusCodeValue = EFI_SUBCLASS_SPECIFIC | 0x00000005;

// Software Class X64 Exception Subclass Error Code definitions.
// These exceptions are derived from the debug protocol
// definitions in the EFI specification.
//
/// X64 divide error exception.
pub const EFI_SW_EC_X64_DIVIDE_ERROR:     EfiStatusCodeValue = debug_support::EXCEPT_X64_DIVIDE_ERROR as u32;
/// X64 debug exception.
pub const EFI_SW_EC_X64_DEBUG:            EfiStatusCodeValue = debug_support::EXCEPT_X64_DEBUG as u32;
/// X64 NMI exception.
pub const EFI_SW_EC_X64_NMI:              EfiStatusCodeValue = debug_support::EXCEPT_X64_NMI as u32;
/// X64 breakpoint exception.
pub const EFI_SW_EC_X64_BREAKPOINT:       EfiStatusCodeValue = debug_support::EXCEPT_X64_BREAKPOINT as u32;
/// X64 overflow exception.
pub const EFI_SW_EC_X64_OVERFLOW:         EfiStatusCodeValue = debug_support::EXCEPT_X64_OVERFLOW as u32;
/// X64 bound exception.
pub const EFI_SW_EC_X64_BOUND:            EfiStatusCodeValue = debug_support::EXCEPT_X64_BOUND as u32;
/// X64 invalid opcode exception.
pub const EFI_SW_EC_X64_INVALID_OPCODE:   EfiStatusCodeValue = debug_support::EXCEPT_X64_INVALID_OPCODE as u32;
/// X64 double fault exception.
pub const EFI_SW_EC_X64_DOUBLE_FAULT:     EfiStatusCodeValue = debug_support::EXCEPT_X64_DOUBLE_FAULT as u32;
/// X64 invalid TSS exception.
pub const EFI_SW_EC_X64_INVALID_TSS:      EfiStatusCodeValue = debug_support::EXCEPT_X64_INVALID_TSS as u32;
/// X64 segment not present exception.
pub const EFI_SW_EC_X64_SEG_NOT_PRESENT:  EfiStatusCodeValue = debug_support::EXCEPT_X64_SEG_NOT_PRESENT as u32;
/// X64 stack fault exception.
pub const EFI_SW_EC_X64_STACK_FAULT:      EfiStatusCodeValue = debug_support::EXCEPT_X64_STACK_FAULT as u32;
/// X64 general protection fault exception.
pub const EFI_SW_EC_X64_GP_FAULT:         EfiStatusCodeValue = debug_support::EXCEPT_X64_GP_FAULT as u32;
/// X64 page fault exception.
pub const EFI_SW_EC_X64_PAGE_FAULT:       EfiStatusCodeValue = debug_support::EXCEPT_X64_PAGE_FAULT as u32;
/// X64 floating-point error exception.
pub const EFI_SW_EC_X64_FP_ERROR:         EfiStatusCodeValue = debug_support::EXCEPT_X64_FP_ERROR as u32;
/// X64 alignment check exception.
pub const EFI_SW_EC_X64_ALIGNMENT_CHECK:  EfiStatusCodeValue = debug_support::EXCEPT_X64_ALIGNMENT_CHECK as u32;
/// X64 machine check exception.
pub const EFI_SW_EC_X64_MACHINE_CHECK:    EfiStatusCodeValue = debug_support::EXCEPT_X64_MACHINE_CHECK as u32;
/// X64 SIMD exception.
pub const EFI_SW_EC_X64_SIMD:             EfiStatusCodeValue = debug_support::EXCEPT_X64_SIMD as u32;

// Software Class ARM Exception Subclass Error Code definitions.
// These exceptions are derived from the debug protocol
// definitions in the EFI specification.
//
/// ARM reset exception.
pub const EFI_SW_EC_ARM_RESET:                  EfiStatusCodeValue = debug_support::EXCEPT_ARM_RESET as u32;
/// ARM undefined instruction exception.
pub const EFI_SW_EC_ARM_UNDEFINED_INSTRUCTION:  EfiStatusCodeValue = debug_support::EXCEPT_ARM_UNDEFINED_INSTRUCTION as u32;
/// ARM software interrupt exception.
pub const EFI_SW_EC_ARM_SOFTWARE_INTERRUPT:     EfiStatusCodeValue = debug_support::EXCEPT_ARM_SOFTWARE_INTERRUPT as u32;
/// ARM prefetch abort exception.
pub const EFI_SW_EC_ARM_PREFETCH_ABORT:         EfiStatusCodeValue = debug_support::EXCEPT_ARM_PREFETCH_ABORT as u32;
/// ARM data abort exception.
pub const EFI_SW_EC_ARM_DATA_ABORT:             EfiStatusCodeValue = debug_support::EXCEPT_ARM_DATA_ABORT as u32;
/// ARM reserved exception.
pub const EFI_SW_EC_ARM_RESERVED:               EfiStatusCodeValue = debug_support::EXCEPT_ARM_RESERVED as u32;
/// ARM IRQ exception.
pub const EFI_SW_EC_ARM_IRQ:                    EfiStatusCodeValue = debug_support::EXCEPT_ARM_IRQ as u32;
/// ARM FIQ exception.
pub const EFI_SW_EC_ARM_FIQ:                    EfiStatusCodeValue = debug_support::EXCEPT_ARM_FIQ as u32;


