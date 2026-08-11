# Working with cargo-vet

This repository uses [cargo-vet](https://github.com/mozilla/cargo-vet) to ensure
third-party Rust dependencies are audited by a trusted entity or explicitly
exempted.

The initial dependency graph is recorded as exemptions in `config.toml`.
Imported audits from Open Device Partnership, Google, and Mozilla reduce that
baseline. New and updated dependencies must be audited or intentionally
exempted before CI will pass.

## Updating dependencies

After changing `Cargo.toml` or `Cargo.lock`, refresh imported audits and inspect
the remaining work:

```sh
cargo vet
```

Commit any resulting `imports.lock` changes. Follow cargo-vet's recommendation
to inspect a new crate or diff an upgraded crate, then record the review with
`cargo vet certify`.

Shared audits should preferably be contributed to the
[ODP audit repository](https://github.com/OpenDevicePartnership/rust-crate-audits).
Use `cargo vet add-exemption` only when an audit cannot be completed and the
reason for accepting the dependency is documented in the pull request.

Before requesting review, run the same locked check used by CI:

```sh
cargo vet --locked
```
