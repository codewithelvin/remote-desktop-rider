---
name: publish
description: Publish Remote Desktop Rider - check, commit and push to main, then follow the GitHub Actions run until the new release (installer) is on the repo's Releases page and report it. Use whenever the user asks to publish, release, ship, or "commit and push" finished work in this repo, and after every push to main.
---

# Publish

Every push to `main` makes GitHub build the next version (patch number after
the latest `v<major>.<minor>.*` tag) and publish
`rdr-setup-<version>.exe` as a release
(`.github/workflows/ci.yml`). Pushes that only touch `*.md`, `docs/` or
`.claude/` are not built or released.

## 1. Check locally (same as CI)

```sh
export PATH="$USERPROFILE/.cargo/bin:$PATH"
cargo fmt --all --check
cargo clippy --profile quick --workspace --all-targets -- -D warnings
cargo test --workspace   # debug build, as CI: debug assertions catch more (0.5.0 failed CI here)
```

Fix every failure before going on: a red CI run publishes nothing.

## 2. Commit and push

- Stage only the files of this change (`git status` first; never `target/`).
- Commit message: a short summary line, then what changed and why for users.
  End it with the attribution lines the session asks for.
- `git push origin main`.
- For a bigger step (new features, breaking changes) bump `major.minor` in
  `[workspace.package] version` of `Cargo.toml` first; otherwise leave it.

## 3. Follow the GitHub build

```sh
sha=$(git rev-parse HEAD)
sleep 10   # the run needs a moment to appear
id=$(gh run list --workflow ci --commit "$sha" --json databaseId -q '.[0].databaseId')
gh run watch "$id" --exit-status --interval 30   # run in the background: ~15-25 min
```

- If it fails: `gh run view "$id" --log-failed | tail -60`, fix the cause, and
  publish again (a new push gets the next version; nothing to clean up, since
  the tag is only created when the release succeeds).
- If the change was docs-only, there is no run: say so and stop.

## 4. Confirm and report

```sh
gh release view --json tagName,url,assets -q '{tag: .tagName, url: .url, files: [.assets[].name]}'
```

Tell the user the version, the release link, and the installer file name.
