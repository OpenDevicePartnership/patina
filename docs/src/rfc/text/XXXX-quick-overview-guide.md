# Transition Hurdle

  The Patina project is a fundamental change to the UEFI ecosystem to be more secure, performant,
  and reliable which when taken as a whole can be a large and complex change.  Due to engineering
  teams being on short delivery timelines and rooted in Tianocore as a base, rolling out the entire
  change across an entire product portfolio is nearly impossible to coordinate especially if all
  platforms share a common codebase.

  To help with this transition, the Patina project has been designed to be ingested in small incremental
  steps that can be adopted based on the engineering teams' schedule.  Each module provides incremental
  improvements that are stable on their own allowing to ship partial transtions while slowly moving to a
  full implementation.

# Transition Guidance

  Getting started is as simple as replacing the Tianocore DxeCore.efi driver with the Patina DXE core driver
  in the UEFI build.  On it's own, it provides enhanced memory safety, type safe programming, and safe interfaces
  for future growth in addition to the standard Tianocore interfaces allowing the system to boot with no other
  changes.

  This single change is stable and can be part of a shipping project which should be manageable integrating
  across all product lines.  Once that has been ingested, the platform can be advertised as having a secure
  baseline ready for typesafe languate development.  Then as time and resources permit, individual drivers
  can be ported to take advantage of those more secure interfaces and retiring the older insecure interfaces.
  Once all support has been transitioned, the insecure APIs indicitive of UEFI can be disabled to produce a
  locked down and safe firmware.

# Transition Sample

  The following is a representative sample of what is needed to transition an AARCH64 platform to use the minimal
  Patina support, a DXE core driver.

## Patina DXE core Driver
  1) Create a platform specific DXE core driver
       - A sample QEMU driver is provided that can be duplicated to use as the skeleton and when built using
         cargo, produces a Rust based .efi DXE core driver
  2) Configure DXE core driver for the platform
       - The DXE core has 2 platform requirements of setting the GIC base address for interrupts and providing a
         UART crate to allow debug message output
  3) Compile and replace the Tianocore DXE core binary
       - Compiling the Rust code will produce a DXE core .efi binary that can be used in the original platfom's
         FDF file to replace the Tianocore DXE core driver

## Silicon Code Changes

  Everything else needed to boot the platform will be obtained from the HOB list such as memory regions, firmware
  volume access, etc.  There are assumptions the Patina core makes that must be in place to boot which may require
  changes to the silicon provided code.

  1) Tianocore V2 defined resource HOBs
       - Version 2 of the resource hobs have tighter requirements for non-overlaping regions and definitions of region
         assignments, so for security reasons, Patina requires the HOBs provided to the core on entry be updated to V2.
  2) No reliance on APRIORI
       - Patina does not support an APRIORI to enforce good coding practices with proper dependencies.  If silicon code
         makes excessive use of the apriori, ordering in the FDF file and stripping the DEPEX footer from specific
         drivers can be done to help manage forced load ordering.

## Found Errors

  In the representative platform testing, once the 2 assumptions were accounted for, booting the platform exposed
  several issues currently shipping without knowledge in a Tianocore based system.

  1) Over-usage of apriori included 6 DXE drivers
       - To allow quick porting, moved the APRIORI drivers to the top of the root firmware volume in the FDF and ran a tool
         during the build process to replace the DEXEX with a TRUE DEPEX.

  2) Mis-aligned global pointers in memory
       - Mis-aligned pointers are handled properly by HW in the CPU core, but publishing a mis-aligned global protocol pointer
         to the core for any driver to use can cause unforseen errors.

  3) Allocating memory during the "Exit Boot Services" event
       - The exit boot services callbacks are not guaranteed to exit in a specific order, so allocating memory in the callbacks
         is not allowed by spec.

  4) Using global declared FFA Hafnium service buffers
       - Code was written to statically declare a global buffer that was used as a mailbox to the protected Hafnium services instead
         of specifically allocating a protected region from execution in the pre-defined area for the mailbox

  5) Multiple calls to free unallocated memory
       - A driver provided in binary form was attempting to free memory buffers that were never allocated.  Could not debug and
         found through the checks performed by the Rust codebase.

  6) MMIO regions not being declared as un-useable
       - The HOB list was incomplete and did not cover the regions used by MMIO devices.  Access was not guarded in the original code.

## Differences from Tianocore

  Once these issues were worked around in the silicon code, the platform was able to boot normally and no special testing was required
  to make sure the Patina core is executing properly.  All drivers that were dispatched from the firmware volume behaved the same as
  if dispatched from the Tianocore DXE core.
  

