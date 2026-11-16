# Contributing

Thank you for helping. Issues and pull requests are welcome.

## Before you open a pull request

- **Open an issue first for anything larger than a small fix**, so we can agree
  on the shape before you spend time on it.
- **Run the checks CI runs**, from the devshell in `flake.nix`:

  ```sh
  nix develop -c cargo fmt --check
  nix develop -c cargo clippy --all-targets --locked
  nix develop -c cargo nextest run --locked
  nix develop -c cargo test --doc --locked
  nix develop -c cargo deny check all
  ```

- **Keep secrets and real people's data out of tests and fixtures.** Use
  `example.com` addresses.

## How a change lands

This repository is published from WealthReach's internal development repository.
A pull request merged here is carried over to the internal repository, with you
credited as a co-author, and appears in the next release. Release commits here
are snapshots, so your change may show up inside one rather than as a separate
commit.

## License

By contributing, you agree that your contribution is licensed under the
[MIT license](LICENSE), as the rest of this repository is.
