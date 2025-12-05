# Unstable Feature

Unstable features are a way of adding new functionality where the API is not guaranteed to be stable. This
includes:

- A new feature with a transition period to gather additional feedback.
- A working feature that still has some unresolved questions.
- A proof of concept.

## How to Create an Unstable Feature

1. Create a GitHub issue titled `Tracking unstable feature issue` for the new feature.
2. Link this issue to every pull request (PR) related to the feature.
3. Add a feature flag to the `Cargo.toml` to gate the new feature, following the naming convention:
   `unstable-<feature-name>`.
4. Link the tracking issue to every task that contributes to the feature's stabilization.

> **Note:** Feature gating ensures that unstable features are only enabled when explicitly requested, reducing the
> risk of accidental use in production.

## Stabilization of an Unstable Feature

1. Remove the feature gate from the code and `Cargo.toml` once the feature is stable.
2. Close the tracking issue with the pull request that removes the feature gate.
3. Increment the crate's minor version to reflect the new stable feature.
