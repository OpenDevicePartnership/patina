# Patina MM CPU Component

The Patina MM CPU component produces the PI `EFI_MM_CPU_PROTOCOL` inside the MM User Core. The protocol allows MM
drivers to read architecture-standard registers from a CPU's MM save state, including information about the trapping
I/O instruction that generated a software MMI.

Because the MM save state resides in supervisor-only SMRAM, save-state reads are forwarded to the MM Supervisor. The
supervisor enforces the save-state security policy before returning the requested value.
