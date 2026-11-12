---
description: 'Review code changes against Patina project conventions and standards'
tools:
  - search
  - read
---
# Patina Code Review

Review the provided code against Patina project conventions. For each issue found,
cite the specific convention being violated and suggest a concrete fix.

## Review Checklist

Work through each category below. Report findings grouped by category. If a
category has no issues, skip it silently. Apply the conventions from
[copilot-instructions.md](../copilot-instructions.md).

### 1. Module Organization

- No `mod.rs` files; no public definitions in `lib.rs`; correct crate naming.

### 2. Safety

- Raw pointer/slice work where `zerocopy` would suffice.
- `unsafe` blocks missing documented invariants or missing safe wrappers.
- Functions marked `unsafe` that could validate inputs internally.

### 3. Error Handling

- `unwrap()` in non-test code.
- Missing `Result` returns, missing `From<>` conversions at error boundaries.

### 4. Component Model

- Correct use of dependency injection, stored dependencies pattern, and parameter
  types (`Config`/`ConfigMut`/`Service<dyn Trait>`).
- Components depending on anything outside `sdk/`.

### 5. Testing

- New logic missing tests; incorrect test naming.
- Default methods on `#[automock]` traits instead of extension traits.

### 6. Documentation

- Public items missing doc comments.
- `unsafe` functions missing `# Safety`; fallible functions missing `# Errors`.
- Functions that have TPL restrictions missing `# TPL` function documentation.

### 7. UEFI-Specific

- Non-TPL-aware synchronization primitives.
- Possible allocation after `ExitBootServices`.

## Output Format

For each finding, use this format:

**[Category] Issue title**
- **File:** `path/to/file.rs:line`
- **Convention:** Brief description of the rule being violated
- **Suggestion:** Concrete fix or code example

Conclude with a summary: total findings by category and an overall assessment.
