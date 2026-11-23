# Contributing

Thanks for helping with Remote Desktop Rider! Bug reports, test results from
real networks, translations of docs, and code are all welcome.

## Where to start

- Issues labelled [`good first issue`](https://github.com/codewithelvin/remote-desktop-rider/labels/good%20first%20issue)
  are small and self-contained.
- Issues labelled [`help wanted`](https://github.com/codewithelvin/remote-desktop-rider/labels/help%20wanted)
  are bigger pieces where outside help makes the most difference.
- Questions and ideas: [Discussions](https://github.com/codewithelvin/remote-desktop-rider/discussions).
- For a larger change, open an issue or discussion first, so we can agree on
  the approach before you spend time on it.

## Reporting a bug

Please include:

- the app version (**Settings**, or `rider-client --version`) on both computers
- operating systems, and how they were connected (same network or through a
  server)
- what you did, what you expected and what happened
- for connection or picture problems on a self-hosted server: the `diag` lines
  from its log for that session (see
  [Connection diagnostics](README.md#connection-diagnostics); numbers only)

Security problems: please don't open a public issue. Use
[GitHub's private vulnerability reporting](https://github.com/codewithelvin/remote-desktop-rider/security/advisories/new)
instead.

## Building

See [Building from source](README.md#building-from-source). In short:

```powershell
cargo build --profile quick   # fast day-to-day builds (target\quick)
```

The host (being controlled) builds on Windows only; the app and viewer also
build on macOS and Linux:

```sh
cargo build -p rider-app -p rider-client
```

## Before opening a pull request

CI runs the same checks; running them locally saves a round trip:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

On macOS or Linux, limit clippy and test to the crates that build there:
`-p rider-core -p rider-server -p rider-media -p rider-client -p rider-app`.

- Keep pull requests focused: one fix or feature each.
- Add a test when the logic can be tested without a screen or network.
- User-visible text is written in both languages where it is used:
  `tr("Connect", "Qoşul")`. If you don't speak Azerbaijani, put the English
  text in both places and say so in the PR; it will be translated before merge.
- Update the README when behaviour or options change.

Every push to `main` is released automatically as the next version, so `main`
must always be releasable.

## Testing on virtual machines

`test/` has Vagrant setups used for end-to-end checks: a Windows 11 host
(`test/vm`), the server on Ubuntu (`test/server-vm`), and Linux desktops for
the viewer (`test/ubuntu-desktop`, `test/lubuntu`). `test/bad-network/netem.sh`
turns a Linux machine's link into hotel Wi-Fi, weak 4G or worse, for testing
how sessions hold up.

## License

By contributing you agree that your contributions are licensed under the
[MIT License](LICENSE).
