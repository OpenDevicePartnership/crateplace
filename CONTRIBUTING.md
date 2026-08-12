# Contributing to Open Device Partnership

The Open Device Partnership project welcomes suggestions and contributions.
Before opening your first issue or pull request, review our
[Code of Conduct](CODE_OF_CONDUCT.md) to understand how our community interacts
in an inclusive and respectful manner.

## Contribution Licensing

This project is distributed under the terms of the [MIT license](LICENSE). By
contributing code that you wrote, you agree to contribute it under those same
terms and indicate that you have the right to do so.

If you contribute code or documentation authored by others, or content under
another license, identify it clearly in your pull request so the project team
can review it.

## Pull Requests

Create a draft pull request first. Before requesting review, run the checks used
by CI:

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo doc --locked --no-deps
```

Use meaningful commit messages and keep each pull request focused on one change.

When reporting a regression, identify the first offending commit with
`git bisect` when practical.
