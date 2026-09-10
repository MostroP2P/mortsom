# Releasing Mortsom

One command starts a release. `cargo release` bumps the shared workspace
version, writes the changelog and pushes the branch; GitHub Actions turns that
push into a pull request, and the merge into a tag and a GitHub release.

## Prerequisites

```sh
cargo install cargo-release git-cliff
```

Commits must follow Conventional Commits (`feat:`, `fix:`, `refactor:`,
`docs:`, `test:`, `chore:`, `perf:`, `ci:`), which is what the changelog is
generated from. A commit that does not parse is left out of the changelog.

## Cutting a release

```sh
git switch main && git pull
git switch -c release/v0.2.0

cargo release 0.2.0            # dry run: prints the plan, touches nothing
cargo release 0.2.0 --execute  # bump, changelog, commit, push
```

`--execute` performs, in order:

1. Bumps `[workspace.package] version` to `0.2.0`; every crate inherits it.
2. Runs `ci/release/changelog.sh`, which regenerates `CHANGELOG.md` with
   git-cliff up to `v0.2.0`.
3. Commits the lot as `chore(release): v0.2.0`.
4. Pushes `release/v0.2.0` to `origin`. No tag is created locally.

`cargo release patch`, `minor` or `major` work too, if you would rather not
name the version.

## What CI does next

`.github/workflows/release.yml` reacts to both pushes:

- **Push to `release/v*`** — opens a pull request against `main` titled
  `Release v0.2.0`, with the new changelog section as its body. Pushing again
  to the same branch refreshes the title and body instead of opening a second
  pull request.
- **Merge into `main`** — if the workspace version has a changelog entry and
  no tag yet, tags the merge commit `v0.2.0` and publishes a GitHub release
  with the same changelog section as its description.

CI (`ci.yml`) runs on the pull request as usual, so a release cannot land on a
red build.

## Guard rails

- `release.toml` sets `allow-branch = ["release/v[0-9]*"]`: `cargo release`
  refuses to run on `main` or on a feature branch.
- `publish = false`: nothing is pushed to crates.io. Mortsom is a harness, not
  a library.
- The publish job is idempotent. It does nothing when the tag already exists
  or when the version has no changelog entry, so ordinary pushes to `main`
  are ignored.

## If something goes wrong

- **The push failed but the commit exists.** Push the branch yourself; the
  workflow opens the pull request on arrival.
- **The changelog needs an edit.** Edit `CHANGELOG.md` on the release branch
  and push. The pull request body and, later, the GitHub release description
  both follow the file.
- **The release must be abandoned.** Close the pull request and delete the
  branch. No tag exists until the merge, so nothing else has to be undone.
