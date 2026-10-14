# Releasing tapin

Releases are built by [dist](https://opensource.axo.dev/cargo-dist/) (`dist-workspace.toml`, `.github/workflows/release.yml`) when a version tag is pushed.

## One-time setup (needs the maintainer's accounts)

1. **Make `Thevi99/tapin` public.** The shell installer and Homebrew download release assets anonymously, so they can't work while the repo is private.
2. **Homebrew tap:** create a public repo `Thevi99/homebrew-tap` (a README is enough). Then add a fine-grained GitHub token with *Contents: read and write* on that repo as the `HOMEBREW_TAP_TOKEN` secret on `Thevi99/tapin`.
3. **crates.io:** create an API token with the publish-new and publish-update scopes. Add it as the `CARGO_REGISTRY_TOKEN` secret on `Thevi99/tapin`.

## Each release

1. Set `version` in `Cargo.toml` (for example `0.1.0`), then run `cargo build` to update `Cargo.lock`.
2. Check:
   ```bash
   cargo test && cargo clippy --all-targets -- -D warnings
   cargo publish --dry-run
   dist plan            # what CI will build
   dist generate --check
   ```
3. Commit, then tag and push:
   ```bash
   git tag v0.1.0 && git push origin v0.1.0
   ```
4. CI builds macOS (arm64, x86_64) and static Linux musl (arm64, x86_64) binaries. It then creates the GitHub Release with installers, updates `Thevi99/homebrew-tap`, and publishes to crates.io.
5. Verify on a clean machine:
   ```bash
   brew install Thevi99/tap/tapin && tapin --version
   curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Thevi99/tapin/releases/latest/download/tapin-installer.sh | sh
   cargo install tapin
   ```

`tapin --version` reads the Cargo.toml version, so it matches the tag as long as step 1 was done.
