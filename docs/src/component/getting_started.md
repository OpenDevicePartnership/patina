# Getting Started with Components

Components are the mechanism used to attach additional functionality to the Patina core while keeping each piece
decoupled from the others. In a systems programming context, components are akin to drivers. Patina uses dependency
injection through each component's entry point function. The component's entry point defines what it requires before it
can run. As an example, a component whose interface is `fn entry_point() -> Result<()>` has **no dependencies** where
as a component with the interface `fn entry_point(service: Service<dyn Interface>) -> Result<()>` has a dependency on
the `dyn Interface` service being registered. The latter component will not execute until this service has been
produced.

This architecture ensures:

- Loose coupling - Components do not depend directly on each other.
- Explicit dependencies - Components declare the services or configuration they need.
- Flexibility - It does not matter who provides a service or sets a configuration, only that it is available.
- Maintainability - Each component can be developed and maintained independently.

At runtime, the dependency injection model allows the patina core to track which component provides which service(s)
and identify the missing dependency that is preventing any given component from running, making it easy to determine
which components are not executing and why.

When it comes to understanding components and how they interact with each other, there are three main topics that you
must understand - (1) Components, (2) Configuration, and (3) Services. Each will be discussed below in broad scopes,
but more details can be found in this mdbook, and in the component documentation for [patina](https://github.com/OpenDevicePartnership/patina/tree/main/sdk/patina).

## Components

As mentioned above, Components are a way to attach additional functionality to the core that is executed in a
controlled manner based off of the component function interface. Components can be used to set configuration, create
services, communicate with physical devices, and many other things. The components section of the [patina](https://github.com/OpenDevicePartnership/patina/tree/main/sdk/patina)
goes into much more detail regarding components.

## Configuration

Configuration comes in two types of flavors - public configuration and private configuration. As the name suggests,
public configuration can be accessed by any executing component, and is typically set by the platform to generically
configure multiple components at once. Private configuration is configuration set when registering the component with
the core.

Public configuration is consumed by components by using the `Config<Type>` in the function interface for the component
(See [Interface](https://opendevicepartnership.github.io/patina/component/interface.html#component-params)).
Configuration is typically set by the platform using the [Core::with_config](https://github.com/OpenDevicePartnership/patina),
however each configuration type must implement `Default`, so configuration will always be available to components.

Private configuration is the configuration set when instantiating a component that is registered with the core using
[Core::with_component](https://github.com/OpenDevicePartnership/patina). Not all components will have private
configuration; it depends on the component implementor and the needs of the component.

## Services

Services are the mechanism in which to share functionality between components via a well-defined interface (trait)
while allowing the underlying implementation to have different implementations per platform. This enables platforms
to switch implementations without directly breaking any other components that depend on that functionality. Services
may be registered by the core itself, by components, or by the platform via the [Core::with_service](https://github.com/OpenDevicePartnership/patina)
during Core setup. See the Patina [Service](https://opendevicepartnership.github.io/patina/component/interface.html#servicet)
or [Patina Sdk](https://github.com/OpenDevicePartnership/patina) crate documentation for more information.

``` admonish note
The core may take an **optional** dependency on some services. These services will be directly communicated in the
inline documentation from the core and must be directly registered using [Core::with_service](https://github.com/OpenDevicePartnership/patina).
If not, there is no guarantee that the service will be available before the core needs it. The core must be able to
operate (albeit with potentially reduced capabilities) if no services are provided via [Core::with_service](https://github.com/OpenDevicePartnership/patina).

If the core requires platform-specific functionality mandatory for **core** operation, it will be enforced via
mechanisms other than the [Core::with_service] as missing services can only be determined at runtime. Typically this
will involve using an API exposed from the core that will cause a build break if a platform fails to provide the
required functionality. 
```

Services can be registered and made available to components in a few different ways. The first way is that the core
itself produces some services directly, such as the Memory Manager. This is a way to expose controlled access to
internal functionality. The second is that a service can be registered directly with a core using [Core::with_service](https://github.com/OpenDevicePartnership/patina).
This is only available for services that have no external dependencies, and can be instantiated directly. Finally, a
component can register a service by using [Storage::add_service](https://github.com/OpenDevicePartnership/patina),
which is used when a service has a dependency, be it another service, configuration, or something else.
