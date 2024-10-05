# NuGet Packaging

## Manual Packaging (X64, Debug)

1. `pip install edk2-pytool-extensions`

2. `cargo make build-arch`

3. `mkdir temp`

4. `copy target/x86_64-unknown-uefi/debug/dxe_core_x64.efi temp`

5. `nuget-publish --Operation PackAndPush --ConfigFilePath nuget_publishing/debug_x64_config.yaml --Version <X.X.X> --CustomLicensePath <WORKSPACE>/nuget_publishing/license.txt --InputFolderPath temp`
    ** NOTE: The CustomLicensePath must be absolute

## Manual Packaging (X64, Release)

1. `pip install edk2-pytool-extensions`

2. `cargo make -p release build-arch`

3. `mkdir temp`

4. `copy target/x86_64-unknown-uefi/release/dxe_core_x64.efi temp`

5. `nuget-publish --Operation PackAndPush --ConfigFilePath nuget_publishing/release_x64_config.yaml --Version <X.X.X> --CustomLicensePath <WORKSPACE>/nuget_publishing/license.txt --InputFolderPath temp`
    ** NOTE: The CustomLicensePath must be absolute
