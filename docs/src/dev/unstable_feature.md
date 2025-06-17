# Unstable Feature

Unstable feature are a way of adding new functionality where the API is not guaranteed to be set in stone.
This include:

- A new feature with a transition period to ask for additional feedback.
- A working feature that still have some unresolved question.
- A proof of concept.

## How to create an unstable feature

1. Create an issue with the name `Tracking unstable feature issue`.
2. Link the issue to every PR that is related to this feature.
3. Add a feature to the `Cargo.toml` to feature gate the new feature
following the naming convention: `unstable-<feature name>`.
4. Link the tracking unstable feature issue to every task that adds to its stabilization.

## Stabilization of an unstable feature

1. Remove the feature gate from the code and `Cargo.toml`.
2. Close the tracking unstable feature issue with the pull request that remove the feature gate.
3. Increment crate minor version.
