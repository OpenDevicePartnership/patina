# RFC: `Unstable Feature`

This RFC is to add a way for feature gating new features that we consider unstable.
This would include an issue template and a way of feature gating the unstable feature.

Use cases I see that would make sense to have unstable are:

- New feature, this gives time to get additional feedback to apply changes.
Having a transition period allows to make API changes without incrementing the major version.
- Working feature that still has some unresolved questions that has a potential to change the API design.
- Proof of concept features.

## Change Log

- 2025-05-08: Initial RFC created.

## Dev flow example

- Add a feature to the cargo.toml named `unstable-<feature-name>`
- Use `#[cfg(feature = "unstable-<feature-name>")]` to feature gate the unstable code.
- Create an issue to track the stabilization of the feature.

Example of issue template for unstable feature stabilization:

```yml

# GitHub issue form for tracking unstable features.
#
##
# Copyright (c) Microsoft Corporation.
# SPDX-License-Identifier: BSD-2-Clause-Patent
##
name: 📓 Tracking unstable feature issue.
description: "Track an unstable feature."
title: "[Unstable] Tracking Issue for `<feature gate name>`"

body:
  - type: markdown 
    id: feature-gate
    attributes:
      value: "Feature gate: `#![feature(<feature gate name>)]`"

  - type: markdown
    id: description
    attributes:
      value: A concise description of the unstable feature.

 - type: checkboxes
    attributes:
      label: Type of unstable feature
      description: Select what kind of unstable feature this is.
      options:
      - label: New feature, this gives time to get additional feedback to apply changes. 
      Having a transition period allows to make API changes without incrementing the major version.
      - label: Working feature that still has some unresolved questions that has a potential to change the API design.
      - label: Proof of concept features.
      required: true

  - type: checkboxes
    attributes:
      label: Unresolved Questions
      description: List every unresolved questions that need to be answered for the feature to be stable.
      options:
      - label: <Unresolved Questions>
      required: true
```

## Motivation

The motivation to have this feature is to be able to merge features that are mostly working
but have some unresolved questions that could lead to an API change.
Marking something unstable allows a user of the unstable feature to not rely heavily on that feature
and participate in the stabilization of this one.
Another pro of feature gating unstable features is that we wouldn't need to increment
the version each time such a feature changes,
but only when something considered stable changes. That would result in less version bumping.

## Goals

Making the API clearer by having a way of telling what is unstable and keeping versioning cleaner.

## Requirements

Easy to follow the state of the feature stabilization and enabling / disabling the feature should be easy.

## Unresolved Questions

- None for now.

## Prior Art (Existing PI C Implementation)

Doing a PR marked as breaking change and increment the version.

## Alternatives

- Not feature gating potentially unstable new features.
- Add a doc comment saying that the API could change.
- Having a nightly branch
