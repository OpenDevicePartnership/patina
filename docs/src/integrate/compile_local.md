# Compiling through EDK II

Building Rust code with the EDK II build system is currently only supported in [Project Mu](https://github.com/microsoft/mu_basecore)
and **not** in EDK II. It is recommended that the Patina DXE Core be built in a separate repo and the resulting
EFI binary be referenced in the platform FDF file than attempting to build it directly in the platform firmware
workspace. Support for building Rust code in Project Mu may be removed in the future.

Instructions have been removed due to complexity. Reach out to the Patina team if you have a strong need for this
feature.
