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

## Local reference tree

`reference/chaos-viewer/` may be cloned locally for context. It must remain gitignored
and must never be committed.
