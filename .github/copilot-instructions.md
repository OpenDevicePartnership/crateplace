# CratePlace Development Instructions

CratePlace is a Rust library and Cargo subcommand that generates and validates
linker scripts for embedded projects. It is a host-side tool that uses the
standard library; do not assume the crate itself supports `no_std` or embedded
targets.

## Code Changes

- Use Rust 1.95 and Edition 2024 features only.
- Preserve the existing builder-style `CratePlacer` API and CLI behavior unless
  a change explicitly requires a breaking interface change.
- Follow the existing `thiserror` error enums and source chains. Use
  `IOToFileResult` when adding file operations so errors retain path and
  read/write context.
- Avoid panics for invalid configuration, malformed binaries, filesystem
  failures, or other user-controlled input. Return a contextual error instead.
- Keep the crate free of `unsafe` code unless it is necessary and its safety
  invariants are documented and tested.
- Add focused tests for behavior changes and regressions. Keep platform
  behavior compatible with Linux and Windows.
- Do not make unrelated formatting, dependency, or refactoring changes.

## Validation

Run the checks relevant to the change before requesting review. CI runs:

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps
cargo check --locked
cargo deny check
cargo machete
cargo vet --locked
```

Keep `Cargo.lock` committed. When dependencies change, update the cargo-vet
store under `supply-chain/` and follow `supply-chain/README.md`.

## Reviews

Pay particular attention to:

- code paths that can panic on user-controlled input;
- integer overflow and address-range calculations;
- symbol classification and mangling assumptions;
- generated linker-script syntax and section ordering;
- file paths, subprocess handling, and cross-platform behavior;
- errors that discard their underlying source or relevant file context.

Do not report compiler, Clippy, or formatting failures solely from inspection;
run the corresponding command and report its output.

## Commit Messages

- Use Conventional Commits, such as `fix: handle missing output directory`.
- Keep the subject line at 50 characters or fewer.
- Separate the subject from the body with a blank line.
- Wrap body text at 72 characters.
- Use the body to explain what changed and why, not how it was implemented.

### AI Attribution

Every commit containing AI-generated or AI-assisted work must include an
`Assisted-by` trailer with the verified model version. Do not guess or copy a
model identifier from another session.

```text
Assisted-by: AGENT_NAME:MODEL_VERSION [SPECIALIZED_TOOL ...]
```

List only specialized analysis tools. Do not list basic development tools such
as Git, Cargo, or editors. AI agents must not add `Signed-off-by`; only humans
can certify the Developer Certificate of Origin.