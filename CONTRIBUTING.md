# Contributing to Open Device Partnership

The Open Device Partnership project welcomes your suggestions and contributions! Before opening your first issue or
pull request, please review our [Code of Conduct](CODE_OF_CONDUCT.md) to understand how our community interacts in an
inclusive and respectful manner.

## Contribution Licensing

Most of our code is distributed under the terms of the [MIT license](LICENSE), and when you contribute code that you
wrote to our repositories, you agree that you are contributing under those same terms. In addition, by submitting your
contributions you are indicating that you have the right to submit those contributions under those terms.

## Other Contribution Information

If you wish to contribute code or documentation authored by others, or using the terms of any other license, please
indicate that clearly in your pull request so that the project team can discuss the situation with you.

## Contribution Guideline

* Format the code with `cargo make all`.
* Use meaningful commit messages. See [this blogpost](http://tbaggery.com/2008/04/19/a-note-about-git-commit-messages.html)

## PR Etiquette

* Make sure that GitHub status checks ("PR gates") pass in your PR.

## Careful Use of `Unsafe`

Working with low-level software, `unsafe` usage is a necessity. However, please wrap unsafe code with safe interfaces
to prevent `unsafe` keyword being sprinkled everywhere.

## RFC Draft PR

If you want feedback on your design early, please create a draft PR with title prefix `RFC:`.

## Branch Naming Scheme

For now, we're not using forks. Eventually a personal fork will be required for any PRs to limit the amount of people
with merge access to the main branch. Until that happens, please use meaningful branch names like this
`user_alias/feature` and avoid sending PRs from branches containing prefixes such as "wip", "test", etc. Prior to
sending a PR, please rename the branch.

## Regressions

When reporting a regression, please ensure that you use `git bisect` to find the first offending commit, as that will
help us finding the culprit a lot faster. File issues using GitHub issues in the relevant repository.
