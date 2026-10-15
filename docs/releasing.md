# Releasing Mortsom

One command starts a release. `cargo release` bumps the shared workspace
version, writes the changelog and pushes the branch. Pushed to `main`, GitHub
Actions turns the release commit into a tag and a GitHub release. Pushed to a
`release/vX.Y.Z` branch, it opens a pull request first and tags the merge.

## Prerequisites

```sh
cargo install cargo-release git-cliff
```

Commits must follow Conventional Commits (`feat:`, `fix:`, `refactor:`,
`docs:`, `test:`, `chore:`, `perf:`, `ci:`), which is what the changelog is
generated from. A commit that does not parse is left out of the changelog.

## Cutting a release from `main`

The everyday path. The release commit lands on `main` straight away and CI
publishes it.

```sh
git switch main && git pull

cargo release patch            # dry run: prints the plan, touches nothing
cargo release patch --execute  # bump, changelog, commit, push
```

`--execute` performs, in order:

1. Bumps `[workspace.package] version` (here `0.1.0` to `0.1.1`); every crate
   inherits it.
2. Runs `ci/release/changelog.sh`, which regenerates `CHANGELOG.md` with
   git-cliff up to `v0.1.1`.
3. Commits the lot as `chore(release): v0.1.1`.
4. Pushes `main` to `origin`. No tag is created locally.

`minor`, `major` or an explicit version such as `cargo release 0.2.0` work the
same way.

## Cutting a release through a pull request

When the bump should be reviewed before it lands, run the same command on a
`release/vX.Y.Z` branch whose name matches the version it will produce:

```sh
git switch main && git pull
git switch -c release/v0.2.0

cargo release 0.2.0            # dry run
cargo release 0.2.0 --execute  # bump, changelog, commit, push the branch
```

## What CI does next

`.github/workflows/release.yml` reacts to both pushes:

- **Push to `main`** — if the workspace version has a changelog entry and no
  tag yet, tags the commit `v0.1.1` and publishes a GitHub release with the
  changelog section as its description. A release cut from `main` is
  published by this job alone.
- **Push to `release/v*`** — opens a pull request against `main` titled
  `Release v0.2.0`, with the new changelog section as its body. Pushing again
  to the same branch refreshes the title and body instead of opening a second
  pull request. The merge into `main` then goes through the job above.

CI (`ci.yml`) runs on pushes to `release/v*` branches — a pull request opened
by the workflow bot does not trigger `pull_request` workflows, so the branch
push is what puts the checks on the release. A release cannot land on a red
build.

## Guard rails

- `release.toml` sets `allow-branch = ["main", "release/v[0-9]*"]`:
  `cargo release` refuses to run on a feature branch.
- `publish = false`: nothing is pushed to crates.io. Mortsom is a harness, not
  a library.
- The publish job is idempotent. It does nothing when the tag already exists
  or when the version has no changelog entry, so ordinary pushes to `main`
  are ignored.
- The release workflow refuses a branch whose name does not match the
  workspace version, so a branch pushed before `cargo release --execute`
  fails loudly instead of opening a pull request for the stale version.

## If something goes wrong

- **The push failed but the commit exists.** Push the branch yourself; the
  workflow opens the pull request on arrival.
- **The changelog needs an edit.** Edit `CHANGELOG.md` on the release branch
  and push. The pull request body and, later, the GitHub release description
  both follow the file.
- **The release must be abandoned.** On a release branch, close the pull
  request and delete the branch; no tag exists until the merge. On `main`
  the tag and the GitHub release already exist once CI has run, so cut a new
  patch with the fix instead of rewriting history.
