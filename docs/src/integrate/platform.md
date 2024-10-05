# Updating the Platform

This section will go over how to update your platform repository to be able to add the new pure
ust DXE Core into the final flash image such that it is discovered by, and executed by, the PEI
phase.

Similar to [Workspace Setup](integrate/workspace.md), the method that you do this will depend on
if your dxe_core is compiled in a separate repository and provided to the platform as a binary,or
if you are having the EDKII build system compile it. Please review one of the two options depending
on your goal:

1. [Compiling Locally](integrate/platform_local.md)
1. [Compiling Externally](integrate/platform_external.md)
