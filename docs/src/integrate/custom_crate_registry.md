# Hosting a Custom Crate Registry

[Platform Integration and Crate Patching](./platform_integration_and_patching.md) explains how a platform consumes
Patina packages from [crates.io](https://crates.io). This page focuses on a more advanced and niche topic of using a
custom registry. If the following two conditions do not apply to you, you can ignore this page. This page exists for the
following situations where directly using the public crates.io registry alone is not sufficient:

1. **An organization requires an internally hosted registry.** Policy, such as corporate security rules, may forbid
   builds from fetching directly from crates.io. It is also common to have a delay in propagating new versions of
   releases published to crates.io to provide a small window of time for any upstream security issues in a new release
   to be discovered. The organization can set up a registry that mirrors crates.io, and every package, including
   Patina's, is fetched through it. This is the more common case for a custom registry. The internal registry acts as
   nothing more than a mirror and cache of crates.io.
2. **Hosting Patina packages with custom changes.** This should almost never be needed, and, if so, only for a very
   short period of time. If you find you need to make modifications to Patina source code that the upstream project
   distributes through crates.io, **please contribute your changes back to the upstream Patina repository** instead of
   maintaining a separate internal fork. **It is recommended to avoid custom changes in Patina crates that are publicly
   available entirely**, even for a short period of time, as it increases the "stickiness" of your changes, making it
   harder to eventually get back onto public crate releases and increasing your maintenance burden.

The page also covers a backup that lets you rebuild a release if the registry is unavailable.

## Background

### Packages, Crates, and `.crate` Files

[Cargo](https://doc.rust-lang.org/cargo/) is Rust's build tool and package manager. A Rust library is distributed as a
[package](https://doc.rust-lang.org/book/ch07-01-packages-and-crates.html) described by a `Cargo.toml` file. Running
[`cargo package`](https://doc.rust-lang.org/cargo/commands/cargo-package.html) turns a package into a compressed
`.crate` file containing its source code. That file is what a registry stores and serves.

### Crate Registry Overview

A [registry](https://doc.rust-lang.org/cargo/reference/registries.html) is a place Cargo downloads packages from. The
default is crates.io. A registry has up to four parts:

| Part              | Purpose                                                                                          | Required                 |
| :---------------- | :----------------------------------------------------------------------------------------------- | :----------------------- |
| Index             | Lists every published version of every package, with its dependencies and checksum               | Yes                      |
| `config.json`     | JSON file at the root of the index that tells Cargo where to download (`dl`) and publish (`api`) | Yes                      |
| Download location | Serves the `.crate` files                                                                        | Yes                      |
| Web API           | Lets `cargo publish`, `cargo yank`, and similar commands change the registry                     | Only to publish by Cargo |

The [index format](https://doc.rust-lang.org/cargo/reference/registry-index.html) defines these parts. `cargo publish`
requires an `api` value in `config.json` to work against that registry. The Cargo Book notes that a minimal registry,
which is an index plus a server holding the `.crate` files, is enough for closed environments where users do not publish
through Cargo. See [Running a Registry](https://doc.rust-lang.org/cargo/reference/running-a-registry.html).

### Sparse and Git Indexes

Cargo reads an index using one of two
[protocols](https://doc.rust-lang.org/cargo/reference/registries.html#registry-protocols).

- The [`sparse` protocol](https://doc.rust-lang.org/cargo/reference/registry-index.html#sparse-protocol): more efficient
  because it fetches only the files needed over HTTP. It is selected by an index URL starting with `sparse+`.
- The [`git` protocol](https://doc.rust-lang.org/cargo/reference/registry-index.html#git-protocol): clones an entire Git
  repository of index data.

`sparse` should be used for new registries. Cargo stores the registry URL in `Cargo.lock`, so the Cargo Book advises
against serving one registry over both protocols.

### How Cargo Is Pointed at a Registry

Registries are named in a [`.cargo/config.toml`](https://doc.rust-lang.org/cargo/reference/config.html) file. Cargo
reads these files from the current directory, every parent directory, and finally `$CARGO_HOME` (`%USERPROFILE%\.cargo`
on Windows). It is common to check in a `.cargo/config.toml` file into the platform's Rust repository. Then, everyone
working in the repo automatically shares common configuration specified in the file when running cargo commands, such as
custom registry configuration. The configuration in the home directory is typically reserved for personal settings and
credentials.

## Different Mechanisms for Mirroring Versus Patching

Cargo has two mechanisms for handling custom registries and source replacements:

| Case                        | Cargo mechanism                                                                                             | Change to dependency declarations                          |
| :-------------------------- | :---------------------------------------------------------------------------------------------------------- | :--------------------------------------------------------- |
| 1. Internal mirror          | [Source replacement](https://doc.rust-lang.org/cargo/reference/source-replacement.html) with `replace-with` | None. `patina = "24"` stays as it is                       |
| 2. Temporary custom changes | [`[patch]`](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html)                         | A `[patch.crates-io]` table in the workspace root manifest |

Note that source replacement assumes the replacement is an exact copy of the original and must not contain packages the
original lacks. Due to this, the Cargo Book says it is not the right option for patching or using private registries
with custom changes (not a pure mirror). `[patch]` is the correct mechanism for this when source does not exactly match
the original.

A patch applies to the whole dependency graph, including other packages' dependencies, and Cargo resolves the patched
package to one version for the entire graph.

## Where to Host the Registry

This section is just a recommendation, not a Cargo requirement. Each organization's situation and existing
infrastructure will influence the best choice.

1. **A service the organization already runs and supports.** If the organization has an existing artifact manager that
   already has access control, using Cargo to integrate with it may be the simplest path.
2. **An open-source server operated by a named team.** This may be an option if a team commits to patching, monitoring,
   and maintaining the server.
3. **Static files and a vendored copy.** This does not require special infrastructure and is considered a backup option
   if (1) and (2) are not feasible. This can be achieved using
   [`cargo vendor`](https://doc.rust-lang.org/cargo/commands/cargo-vendor.html), which copies the source of every
   dependency into a directory and prints the Cargo configuration needed to build from it.

Whichever you choose, the registry should be owned by a platform or infrastructure team, not hosted on an individual's
machine.

For case 1, the product must be able to proxy or cache crates.io. The table below shows that this capability is called
out for Azure Artifacts (upstream sources), Artifactory (remote repositories), and Kellnr (proxy cache). The Gitea and
Margo entries describe hosting packages you publish and do not mention proxying crates.io. For case 2, a registry is
optional because an internal Git repository can hold the changes.

### Some Options

The Patina team has not used all of these options. The table is provided for informational purposes only after doing
research on common and popular solutions. However, the Patina team has used Azure Artifacts for mirroring crates.io.

| Option                                                                                  | Type                      | Relevant facts from its documentation                                                                                                                                   |
| :-------------------------------------------------------------------------------------- | :------------------------ | :---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [Azure Artifacts](https://learn.microsoft.com/azure/devops/artifacts/get-started-cargo) | Managed                   | Feeds can include upstream sources for public registries. Requires Cargo 1.74 or later.                                                                                 |
| [AWS CodeArtifact](https://docs.aws.amazon.com/codeartifact/latest/ug/using-cargo.html) | Managed                   | Requires Cargo 1.74 or later.                                                                                                                                           |
| [JFrog Artifactory](https://docs.jfrog.com/artifactory/docs/cargo-repositories)         | Commercial                | Local repositories hold internal crates and remote repositories proxy crates.io. Virtual repositories and federation are not supported for Cargo. Sparse index only.    |
| [Gitea](https://docs.gitea.com/usage/packages/cargo)                                    | Open-source Git host      | Built-in Cargo registry with sparse or Git index. Supports publish, yank, and search.                                                                                   |
| [Kellnr](https://kellnr.io/)                                                            | Open-source, self-hosted  | Private registry with a crates.io proxy cache, S3/GCS/MinIO storage, and OpenID Connect sign-in. Its site states it is built by a single developer in their spare time. |
| [Margo](https://github.com/integer32llc/margo)                                          | Open-source, static files | Generates a registry that is only static files. Crates are added with `margo add` instead of `cargo publish`.                                                           |

The Cargo wiki's [third-party registries](https://github.com/rust-lang/cargo/wiki/Third-party-registries) page lists
additional options.

## Setting Up the Registry

The sections below follow the [two cases](#different-mechanisms-for-mirroring-versus-patching) mention earlier: patching
versus mirroring.

| Section                                                                                        | Case 1: Mirror | Case 2: Patch                 |
| :--------------------------------------------------------------------------------------------- | :------------- | :---------------------------- |
| [Common Setup](#common-setup)                                                                  | Required       | Registry-hosted only          |
| [Case 1: Mirror crates.io](#case-1-mirror-cratesio)                                            | Required       | Not used                      |
| [Case 2: Override a Package Temporarily](#case-2-override-a-package-temporarily)               | Not used       | Required                      |
| [Case 2: Publish a Customized Package](#case-2-publish-a-customized-package)                   | Not used       | Registry-hosted only          |

The examples use a mirror named `corp-mirror` for case 1 and, only if needed, a registry named `corp-registry` for
case 2. The URLs are only examples, you will need to substitute your own.

### Common Setup

#### Register the Registry

Create the registry in your chosen product and add it to `.cargo/config.toml` in the platform's Rust repository. Case 1
needs `corp-mirror`. Case 2 only needs `corp-registry` for the registry-hosted scenario, so create it as a separate
registry from the mirror and skip it when using a Git fork:

```toml
[registries]
corp-mirror = { index = "sparse+https://registry.example.com/cargo-mirror/index/" }
corp-registry = { index = "sparse+https://registry.example.com/cargo/index/" }
```

The index URL format differs per product, so copy it from the product's own instructions.

#### Configure Authentication

Cargo authenticates to registries through
[credential providers](https://doc.rust-lang.org/cargo/reference/registry-authentication.html). For developer machines,
the Cargo Book recommends this setting in the home-directory `config.toml`, which uses the operating system's secure
store:

```toml
[registry]
global-credential-providers = ["cargo:token", "cargo:libsecret", "cargo:macos-keychain", "cargo:wincred"]
```

Some products ship their own provider, so follow your registry's instructions first. For CI, you can supply a token
through the environment variable `CARGO_REGISTRIES_CORP_MIRROR_TOKEN` (the registry name in capitals, with dashes
changed to underscores). That variable only works if `cargo:token` is in the provider list, and its default is that.
Remember to never commit tokens to the repository.

### Case 1: Mirror crates.io

Point Cargo's default source at the mirror in the platform repository's `.cargo/config.toml`:

```toml
[source.crates-io]
replace-with = "corp-mirror"
```

`replace-with` can name a registry defined under `[registries]`, which is why `corp-mirror` was registered in
[Common Setup](#common-setup).

Nothing else changes. Dependencies stay `patina = "24"` (with no `registry` key), and third-party components resolve
through the same mirror. Check this file in so developers and CI share it.

Keep these points in mind:

- While source replacement is active, commands that work with a registry directly, such as `cargo publish`, need
  `--registry` to remove ambiguity.
- The mirror must stay an exact copy of crates.io. Do not place customized copies of public packages in it. Use
  [Case 2](#case-2-override-a-package-temporarily) if that's needed.
- Run [`cargo-deny`](https://embarkstudios.github.io/cargo-deny/) and the other supply-chain checks against mirrored
  packages too.

### Case 2: Override a Package Temporarily

First, open a pull request against the Patina project with the change. The override should exist only to keep the
platform building until a Patina release containing the change is published to crates.io.

Add a [`[patch.crates-io]`](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html#the-patch-section)
table to the root `Cargo.toml` of the platform's Rust workspace. Cargo reads `[patch]` from the workspace root, and the
patch also applies to packages that depend on Patina indirectly, so the build only needs a single copy. The table is
named `crates-io` even when crates.io is mirrored, because the key identifies the source being patched. Note that
patched packages keep their names, so a dependency on `patina` still refers to `patina` and is not renamed.

[This section](https://opendevicepartnership.github.io/patina/integrate/platform_integration_and_patching.html#example-patching-multiple-packages)
of the Patina documentation also covers some patching details.

**Very short-term approach: An internal Git fork.** Keep the change in a fork of the Patina repository and pin an exact
commit:

```toml
[patch.crates-io]
patina = { git = "https://git.example.com/forks/patina.git", rev = "<commit>" }
patina_dxe_core = { git = "https://git.example.com/forks/patina.git", rev = "<commit>" }
```

Cargo finds each package anywhere in a Git repository, so every entry can use the same URL. As described in the linked
guide above, patch every related Patina package, base the fork on the release the platform already uses, and make sure
each patched package's version satisfies the platform's version requirements. The previous guide's troubleshooting table
explains what to do when Cargo keeps using the registry release (often fixed with `cargo update <package>`).

If the override is meant to persist longer than the period of time it will take to pick up the next Patina release,
consider using a registry-hosted build as described below to avoid some of the resolution problems that can arise with
Git overrides.

**Non short-term approach: A registry-hosted build.** Publish to `corp-registry` as described in
[Case 2: Publish a Customized Package](#case-2-publish-a-customized-package) and then point the patch at that registry:

```toml
[patch.crates-io]
patina = { version = "24.1.0", registry = "corp-registry" }
```

Each `[patch]` entry is a normal dependency specification, so it accepts the `registry` key. Packages that depend on
`patina` from crates.io receive this build too. You can run `cargo tree -d` to confirm there are no duplicate Patina
packages.

### Case 2: Publish a Customized Package

This step can be skipped if using a Git repository. Otherwise, add a `publish` key to each package's `Cargo.toml` so it
cannot accidentally be published anywhere else by mistake.

```toml
[package]
publish = ["corp-registry"]
```

Then check and publish:

```powershell
cargo publish --registry corp-registry --dry-run
cargo publish --registry corp-registry
```

[`cargo publish`](https://doc.rust-lang.org/cargo/commands/cargo-publish.html) builds the `.crate` file, uploads it, and
waits for the package to appear in the index. There are a few points to note:

- Patina packages depend on each other, so you should only publish a package after the packages it depends on are
  available.
- A dependency that uses a `path` must also state a `version`. Cargo uses the path locally and the version once
  published. See
  [Multiple locations](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#multiple-locations).
- `cargo publish` also accepts `--workspace` and `--package`. It is recommended to publish from CI rather than from a
  developer machine so releases are repeatable and auditable.
- A published version is not meant to change. To retire a bad release, publish a fixed version and then
  [yank](https://doc.rust-lang.org/cargo/commands/cargo-yank.html) the bad one. Yanking hides a version from new
  resolution but does not delete data, and existing `Cargo.lock` files keep working.

### Case 2: Retire the Override

Once a Patina release that contains the change is on crates.io (published by the Patina project), raise the version
requirement to that release if needed, delete the `[patch.crates-io]` entries, and run `cargo update`. If you used a
registry-hosted build, then yank the customized versions afterward.

A few habits can help keep overrides short-lived. These are recommendations, not Cargo requirements:

- Put a comment above each patch entry with the Patina pull request link and the person responsible for driving
  acceptance of the change into Patina.
- Review every override whenever the platform moves to a new Patina release.

### Operating Checklist

- Require Cargo 1.74 or later, the first version with registry authentication that the managed services above document.
- Give publish rights to CI and a small set of maintainers. Give read access to everyone who builds firmware.
- Commit `Cargo.lock` and build releases with `--locked` so a build uses the recorded versions.
- Treat the registry's storage and database like any other build artifact store, with scheduled backups and a tested
  restore.
- Document the `.cargo/config.toml` contents in the platform repository to explain what is being configured and why.

## Troubleshooting

| Symptom                                                  | What to Check                                                                                                                                                |
| :------------------------------------------------------- | :----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cargo publish` fails with "no token" or similar         | A credential provider is configured and, for environment-variable tokens, `cargo:token` is in the list.                                                      |
| `cargo publish` fails before uploading                   | The `publish` key in `Cargo.toml` lists this registry, and the registry's `config.json` has an `api` value.                                                  |
| A registry command is ambiguous after replacing a source | Add `--registry <name>` to commands that contact a registry directly.                                                                                        |
| A version cannot be resolved                             | The version may be yanked. Cargo skips yanked versions for new resolution, while an existing `Cargo.lock` keeps using them.                                  |
| Everyone's lockfile changed after a move                 | The registry URL or protocol changed. Cargo records the URL in `Cargo.lock`, so keep a stable name and one protocol.                                         |
| A local patch is ignored                                 | See the troubleshooting table in [Platform Integration and Crate Patching](./platform_integration_and_patching.md#troubleshooting-when-a-patch-is-not-used). |
