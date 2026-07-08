# Branching strategy

rktop uses a lightweight Git Flow style that keeps the public default branch stable while leaving room for iterative UI and collector work.

## Branch roles

| branch | purpose |
| --- | --- |
| `main` | Stable, release-ready branch. Keep CI green. Tag releases from here. |
| `develop` | Integration branch for the next batch of changes. Work lands here before a release merge into `main`. |
| `feat/<short-name>` | New user-facing behavior or UI work. |
| `fix/<short-name>` | Bug fixes. |
| `docs/<short-name>` | Documentation, screenshots, social preview, README work. |
| `chore/<short-name>` | Maintenance, CI, dependency, repository hygiene. |
| `release/vX.Y.Z` | Optional stabilization branch before merging to `main`. |
| `hotfix/<short-name>` | Urgent fix branched from `main`, then merged back to `main` and `develop`. |

## Normal workflow

```bash
git switch develop
git pull --ff-only

git switch -c feat/my-change
# edit, test, commit

git push -u origin feat/my-change
```

Before merging a work branch:

```bash
cargo fmt --check
cargo check --locked
cargo test --locked
```

Merge policy:

1. Work branches merge into `develop` after checks pass.
2. `develop` merges into `main` when the next public release is ready.
3. Tag releases on `main` with `vX.Y.Z`.

## Commit style

Use Conventional Commits:

```text
feat: add host filtering
fix: handle missing hwmon sensors
docs: update social preview
chore: update ci workflow
```

## Release flow

```bash
git switch develop
git pull --ff-only
cargo fmt --check && cargo check --locked && cargo test --locked

git switch main
git pull --ff-only
git merge --no-ff develop -m "chore: release vX.Y.Z"
git tag vX.Y.Z
git push origin main vX.Y.Z
```

## Main-branch integration checklist

Use this order when keeping `main` stable while integrating through `develop`:

1. Confirm the remote default branch is `main` and local clones have fetched it.
2. Keep CI running on `main`, `develop`, and pull requests.
3. Update documentation and scripts to name `main` as the release branch.
4. Enable or edit branch protection only after maintainers confirm the expected CI check names on `main`.

Do not add branch protection rules as part of documentation-only migration prep; protection is a final repository-admin step after CI has reported on the stable and integration branches.
