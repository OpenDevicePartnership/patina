# RFC: `Unstable Feature`

This RFC is to add a way for feature gating new features that we consider unstable.
This would include an issue template and a way of feature gating the unstable feature.

## Change Log

- 2025-05-08: Initial RFC created.

## Motivation

The motivation to have this feature is to be able to merge features that are mostly
working but have some unresolved questions that could lead to an API change. Marking
something unstable allows a user of the unstable feature to not rely heavily on that
feature and participate in the stabilization of this one. Another pro of feature gating
unstable features is that we wouldn't need to increment the version each time such a
feature changes, but only when something considered stable changes. That would result
in less version bumping.

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
