# Contributing to ChatCMD

Thank you for helping improve ChatCMD. Contributions of code, tests, documentation, design, translations, bug reports, and reproducible investigations are welcome.

By participating, you agree to follow our [Code of Conduct](CODE_OF_CONDUCT.md).

## Before you start

- Search the [issue tracker](https://github.com/int04/ChatCmd/issues) before opening a duplicate.
- Use a public issue for bugs and feature discussions, but follow [SECURITY.md](SECURITY.md) for vulnerabilities.
- Discuss large changes before implementation. This reduces duplicated work and helps align the design with the local-first security model.
- Keep a pull request focused on one problem. Unrelated refactors make review and rollback harder.

## Branch policy

- `main` is the stable integration branch.
- `dev` is the normal target for active development pull requests.
- Maintainers promote reviewed changes from `dev` to `main` for stable releases.

Unless a maintainer requests otherwise, branch from the latest `dev` and open the pull request against `dev`.

## Development setup

Follow [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for prerequisites, local commands, repository layout, and test suites.

The shortest setup is:

```bash
git clone https://github.com/int04/ChatCmd.git
cd ChatCmd
git switch dev
cd web
npm ci
npm run build
cd ..
cargo run
```

## Making a change

1. Create a topic branch from `dev`.
2. Add or update tests for observable behavior.
3. Update user-facing or protocol documentation when behavior changes.
4. Run formatters, linters, tests, and builds appropriate to the affected area.
5. Review the final diff for secrets, generated files, unrelated changes, and accidental debug output.
6. Open a pull request using the repository template.

Do not commit:

- MCP endpoint tokens, cookies, API keys, signing credentials, personal paths, or private conversation content;
- `target/`, `web/node_modules/`, `web/dist/`, `release/`, runtime logs, or local SQLite databases;
- minified/obfuscated release output when the source file is already tracked.

## Engineering expectations

- Preserve the local-first architecture and least-privilege defaults.
- Keep operations bounded in time, memory, output size, and filesystem scope.
- Prefer structured arguments over shell interpolation.
- Treat destructive filesystem, process, Git, and terminal operations as security-sensitive.
- Preserve token redaction and avoid logging user content unless it is required for diagnosis.
- Maintain backward compatibility for persisted data and public MCP contracts, or document an explicit migration.
- Keep English and Vietnamese UI strings aligned when adding user-visible text.
- Update `docs/mcp_method.md` whenever the exposed tool catalog changes.

## Verification

Run the complete suite when practical:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

cd web
npm ci
npm run lint
npm test -- --run
npm run build

cd ../chatgpt-extension
node --test content-chatgpt.test.cjs
```

If a platform-specific command cannot run on your machine, state that clearly in the pull request and include the commands you did run.

## Pull request review

A pull request should explain:

- the user or maintainer problem;
- the approach and important trade-offs;
- security, privacy, persistence, and compatibility impact;
- verification performed;
- screenshots or recordings for visible UI changes.

Maintainers may request smaller commits, additional tests, documentation, or a design discussion. Approval does not guarantee an immediate release.

## Licensing

The project is licensed under the [MIT License](LICENSE). By submitting a contribution, you agree that your contribution may be distributed under that license and that you have the right to submit it.
