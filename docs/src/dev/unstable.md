# Use of Unstable Rust Features in Patina

Patina takes a pragmatic approach to using unstable Rust features. These features are allowed under specific
circumstances, balancing the benefits of relevant features with maintainability.

General guidance is to avoid using unstable Rust or Cargo features. Unstable features may not become stable or change
in significant and unpredictable ways that may impact public APIs and critical portions of the codebase. However, since
Patina is firmware code, it has some properties that lead to features that are in a proposed and unstable state, such
as: largely being no-std, implementing and using its own allocator, frequent low-level and unsafe operations, etc.
Below is an explanation of the guiding principles and practices employed when working with unstable Rust features in Patina.

## When Unstable Rust Features May Be Used

Common scenarios for unstable features:

- When there is no alternative: Certain functionalities provided by unstable features may not have stable equivalents.
- Functionality is especially beneficial for Patina: If an unstable feature provides essential capabilities, the
  project may choose to incorporate that feature to better understand if it is appropriate to consider for adoption
  long-term and to provide feedback to the feature owner.
  - The Patina team should strongly consider whether there is value in using the unstable feature and note the risk
    in the GitHub issue that proposes use of the unstable feature.

## Handling the Risks of Instability

Since unstable features come with the risk of API changes or possible removal, maintainers should be ready to perform
the following tasks to mitigate risk.

- Monitor stability updates: When an unstable API transitions to stable, the new version of Rust provides warnings. The
  team should utilize these warnings as cues to update the codebase, aligning it with the stable API. These warnings
  should be addressed at the same time as the Rust toolchain is updated.
- Replace code: If an unstable API is removed, the code must be promptly replaced with functionally equivalent stable
  code.

## Unstable Feature Proposal Process

An RFC should be created with the following information:

1. Tracking Issue - A link to the GitHub tracking issue for the feature.
2. Feature Name - The unstable feature name.
3. Reason - The reason for using the unstable feature.
4. Alternatives - What else could be used instead of the feature. Tradeoffs for the different choices.
5. Constraints – Are there any scenarios in which the feature should not be used in the project?
6. Risks - Any special risks the project may incur due to use of this feature.

The RFC follows the normal RFC process. If approved, the unstable feature may be added to the codebase adhering to any
constraints noted in the RFC. Currently, no further process is required such as a project tracking issue. At any time,
unstable features can be found in the codebase by searching the repo. Their rationale can be found in the corresponding
RFC.
