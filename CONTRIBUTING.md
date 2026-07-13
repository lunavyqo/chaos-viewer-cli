# Contributing

This is a personal private repository. If you are collaborating with the maintainer,
follow these conventions.

## Identity

Commits and tags must use:

```
lunavyqo <60808132+lunavyqo@users.noreply.github.com>
```

Do not add assistant or automation authorship trailers.

## Workflow

1. Create a focused branch from `main`.
2. Make one logical change per PR.
3. Use Conventional Commit messages (`feat:`, `fix:`, `docs:`, `test:`, `chore:`).
4. Update `CHANGELOG.md` `[Unreleased]` for user-visible changes.
5. Open a PR with a Conventional Commit title; prefer squash merge and delete the branch.

## Checks

Run before opening a PR:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

CI (`.github/workflows/ci.yml`) runs the same gates on `main` and PRs.

## Releases

Version lives in `Cargo.toml` (`package.version`). To publish:

1. Cut `[Unreleased]` notes into a new `## [X.Y.Z]` section in `CHANGELOG.md`.
2. Ensure `Cargo.toml` version matches.
3. Merge to `main`, then tag and push:

   ```bash
   git tag -a v0.1.0 -m "v0.1.0"
   git push origin v0.1.0
   ```

4. GitHub Actions (`.github/workflows/release.yml`) builds and attaches binaries
   for Linux x86_64, Windows x86_64, macOS Intel, and Apple Silicon.

Do not add assistant/tool trailers to the tag or release notes.

## Local reference tree

`reference/chaos-viewer/` may be cloned locally for context. It must remain gitignored
and must never be committed.
