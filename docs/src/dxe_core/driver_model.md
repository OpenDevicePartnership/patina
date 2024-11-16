# UEFI Driver Model

This portion of the core is concerned with implementing boot services that support the [UEFI Driver Model](https://uefi.org/specs/UEFI/2.10_A/02_Overview.html#uefi-driver-model),
in particular the [`EFI_BOOT_SERVICES.ConnectController`](https://uefi.org/specs/UEFI/2.10_A/07_Services_Boot_Services.html#efi-boot-services-connectcontroller)
and [`EFI_BOOT_SERVICES.DisconnectController`](https://uefi.org/specs/UEFI/2.10_A/07_Services_Boot_Services.html#efi-boot-services-disconnectcontroller)
APIs. These routines are technically part of the "Protocol Handler Services" portion of the [UEFI Spec](https://uefi.org/specs/UEFI/2.10_A/07_Services_Boot_Services.html#protocol-handler-services)
but are complex enough to merit their own module and documentation.

The driver_services.rs module within the Rust DXE Core is responsible for implementing the driver service logic for the
core, and uses the services of the [Protocol Database](protocol_database.md) module to implement most of the logic.

In UEFI parlance, "connecting" a controller means discovering and starting any drivers that have support for managing a
given controller and providing services or capabilities on top of that controller. This is is enabled via means of the
[`EFI_DRIVER_BINDING_PROTOCOL`](https://uefi.org/specs/UEFI/2.10_A/11_Protocols_UEFI_Driver_Model.html#efi-driver-binding-protocol)
which provides APIs to determine whether a given driver supports a given controller and start the driver managing that
controller or stop a driver from managing a controller. In addition to `EFI_DRIVER_BINDING_PROTOCOL` the UEFI spec
describes [a number of other protocols](https://uefi.org/specs/UEFI/2.10_A/11_Protocols_UEFI_Driver_Model.html#) for
driver configuration and management. These protocols allow for platform control of driver priority, as well as
diagnostics, configuration, and user interface support. With the exception of the protocols that control driver
selection and priority (which are discussed below), most of these protocols do not directly impact core operation and
are beyond the scope of this documentation.

## Connecting a Controller

Call `core_connect_controller` with a controller `handle` to search the protocol database for drivers to manage the
given controller handle and start a driver. This routine takes optional inputs such as a list of `driver_handles`
containing preferred drivers for the controller, as well as `remaining_device_path` and `recursive` arguments that
control how the tree of controllers underneath this handle (if any) is expanded. This function directly implements the
semantics of [`EFI_BOOT_SERVICES.ConnectController`](https://uefi.org/specs/UEFI/2.10_A/07_Services_Boot_Services.html#efi-boot-services-connectcontroller).

Prior to executing the logic to connect a controller, the device path on the controller handle to be connected is passed
to the [Security Architectural Protocol](https://uefi.org/specs/PI/1.8A/V2_DXE_Architectural_Protocols.html#security-architectural-protocols)
to check and enforce any Platform security policy around connection of the device.

### Determining the Priority Order of Drivers

A firmware implementation may have multiple drivers that are capable of managing a given controller. In many cases, a
driver will claim exclusive access to the controller, meaning that whichever driver executes first will be able manage
the controller. The UEFI spec specifies five precedence rules that are used to order the set of drivers it discovers for
managing a controller to allow the platform some measure of control over which driver is selected to manage the
controller. The precedence rules are used to generate a list of candidate drivers as follows:

1. Drivers in the optional `driver_handles` input parameter to `core_connect_controller` are added to the candidate list
in order.
2. If an instance of the [`EFI_PLATFORM_DRIVER_OVERRIDE`](https://uefi.org/specs/UEFI/2.10_A/11_Protocols_UEFI_Driver_Model.html#efi-platform-driver-override-protocol-protocols-uefi-driver-model)
protocol is found in the system, then drivers it returns are added to the list in order, skipping any that are already
in the list.
3. The set of driver image handles in the protocol database supporting the [`EFI_DRIVER_FAMILY_OVERRIDE_PROTOCOL`](https://uefi.org/specs/UEFI/2.10_A/11_Protocols_UEFI_Driver_Model.html#efi-driver-family-override-protocol),
ordered by the version returned by `GetVersion()` API of that protocol are added to the list, skipping any that are
already in the list.
4. If an instance of the [`EFI_BUS_SPECIFIC_DRIVER_OVERRIDE'](https://uefi.org/specs/UEFI/2.10_A/11_Protocols_UEFI_Driver_Model.html#efi-bus-specific-driver-override-protocol)
protocol is found in the system, then drivers it returns are added to the list in order, skipping any that are already
in the list.
5. All remaining drivers in the [Protocol Database](protocol_database.md) not already in the list are added to the end
of the list.

### Starting Drivers

Once the ordered list of driver candidates is generated as described in the previous section, the
`core_connect_controller` logic will then loop through the driver candidates calling [`EFI_DRIVER_BINDING_PROTOCOL.Supported()`](https://uefi.org/specs/UEFI/2.10_A/11_Protocols_UEFI_Driver_Model.html#efi-driver-binding-protocol-supported)
on each driver. If `Supported()` indicates that the driver supports the controller handle passed to
`core_connect_controller`, then [`EFI_DRIVER_BINDING_PROTOCOL.Start()`](https://uefi.org/specs/UEFI/2.10_A/11_Protocols_UEFI_Driver_Model.html#efi-driver-binding-protocol-start-protocols-uefi-driver-model)
is invoked for the driver. This is done for all drivers in the list, and more than one driver may be started for a
single call to `core_connect_controller`.

```admonish warning
No provision is made in the specification to handle the scenario where a Driver Binding instance is uninstalled between
a call to `Supported()` and a call to `Start()`. Because mutable access to the protocol database is required by
`Supported()` and `Start()` calls, it is possible to uninstall a Driver Binding instance while a
`core_connect_controller` is in process which will result in undefined behavior when `core_connet_controller` attempts
to invoke the `Supported()` or `Start()` functions on a driver binding that has been removed (and potentially freed).
For this reason, `core_connect_controller` is marked unsafe; and care must be taken to ensure that Driver Binding
instances are stable during calls to `core_connect_controller`. This should usually be the case.
```

## Disconnecting a Controller

Call `core_disconnect_controller` with a controller `handle` to initiate an orderly shutdown of the drivers currently
managing that controller. This routine takes optional inputs of `driver_handle` and `child_handle` to allow
finer-grained control over which drivers and/or child controllers of the present controller should be shut down. This
function directly implements the semantics of [`EFI_BOOT_SERVICES.DisconnectController`](https://uefi.org/specs/UEFI/2.10_A/07_Services_Boot_Services.html#efi-boot-services-disconnectcontroller).

To determine which drivers to stop, the [Protocol Database](protocol_database.md#querying-protocol-usages-information)
is queried to determine which driver handles are listed as an `agent_handle` that have the controller `handle` open with
`BY_DRIVER` attribute. This set of drivers is the list of drivers that are "managing" the current controller `handle`
and will be stopped when `core_disconnect_controller` is called.

The caller can narrow the scope of the disconnect operation by supplying the optional `driver_handle` parameter to the
function. If this parameter is supplied, then only that specific driver will be stopped, rather than all of the drivers
managing the controller.

### Shutting Down a Driver

If a driver is a bus driver, the [Protocol Database](protocol_database.md#querying-protocol-usages-information) is queried
to determine the set of child controllers for the current `handle`. A "child controller" is defined as the
`controller_handles` for any usages of this `handle` where the `agent_handle` is the driver being stopped and the usage
has an attribute of `BY_CHILD_CONTROLLER`. If the optional `child_handle` is specified to `core_disconnect_controller`,
then the list of child_controllers is filtered to only include that single `child_handle` if present. Once the set of
child controllers is generated, then [`EFI_DRIVER_BINDING_PROTOCOL.Stop()`](https://uefi.org/specs/UEFI/2.10_A/11_Protocols_UEFI_Driver_Model.html#efi-driver-binding-protocol-stop)
function is invoked to stop all drivers managing the child controllers.

If the driver is not a bus driver, or if all child handles were closed (i.e. the optional `child_handle` was not
specified, or it was specified and that was the only `child_handle` found on the controller), then [`EFI_DRIVER_BINDING_PROTOCOL.Stop()`](https://uefi.org/specs/UEFI/2.10_A/11_Protocols_UEFI_Driver_Model.html#efi-driver-binding-protocol-stop)
is then invoked on the `handle` itself.

### Driver Model State after Disconnecting a Controller

In general, if `core_disconnect_controller` succeeds without failure, it implies that drivers managing the controller
should have had the [`EFI_DRIVER_BINDING_PROTOCOL.Stop()`](https://uefi.org/specs/UEFI/2.10_A/11_Protocols_UEFI_Driver_Model.html#efi-driver-binding-protocol-stop)
method invoked, and this should have caused the driver to release all resources and usages associated with the
controller.

This behavior is key to implementing some of the other flows in the boot services such as [OpenProtocol and CloseProtocol](protocol_database.md#managing-protocol-usages)
operations that require interaction with the driver model.
