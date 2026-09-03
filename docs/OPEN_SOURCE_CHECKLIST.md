# Open-source publication checklist

Use this checklist before changing the GitHub repository visibility to public. Repository files alone cannot configure every required GitHub or third-party setting.

## Legal and ownership

- [ ] Confirm the copyright line in `LICENSE` names the correct rights holder.
- [ ] Confirm every contributor and employer has the right to publish their contributions.
- [ ] Review bundled images, sounds, fonts, icons, and copied code for redistribution rights.
- [ ] Review third-party dependency licenses and notices for compatibility with MIT distribution.
- [ ] Confirm the project name, logo, and domain can be used publicly; document any trademark policy if needed.
- [ ] Obtain legal review if the project is published by a company or includes third-party proprietary work.

## Secrets and private data

- [ ] Scan the **entire Git history**, not only the current tree, with a maintained secret scanner.
- [ ] Rotate every credential that ever appeared in a commit, release archive, log, issue, or build output.
- [ ] Check for MCP URLs, public plugin tokens, cookies, API keys, tunnel credentials, signing keys, notarization credentials, database files, logs, conversations, user names, emails, and absolute personal paths.
- [ ] Review deleted backend, authentication, payment, and tunnel code still present in Git history.
- [ ] Remove private artifacts from existing tags, branches, pull requests, and release assets.
- [ ] If history must be rewritten, coordinate it before public release and rotate affected credentials even after removal.

## Repository settings

- [ ] Set the GitHub description, website, topics, and social preview image.
- [ ] Confirm GitHub detects the MIT license.
- [ ] Enable private vulnerability reporting and security advisories.
- [ ] Enable dependency graph, Dependabot alerts, and secret scanning where available.
- [ ] Create the `bug`, `enhancement`, and `documentation` labels used by issue templates.
- [ ] Decide whether to enable Discussions for support and design proposals.
- [ ] Configure branch protection for `main` and `dev`, including required review and status checks.
- [ ] Restrict force pushes and branch deletion for protected branches.
- [ ] Configure release/tag permissions and require two-factor authentication for maintainers.
- [ ] Review Actions permissions before enabling workflows from forks.

## Engineering readiness

- [ ] Make `cargo fmt --check`, Cargo check/test/Clippy, frontend lint/test/build, and extension tests pass from a clean checkout.
- [ ] Add CI only after the baseline is green, then require it on protected branches.
- [ ] Test source setup exactly as documented on supported platforms.
- [ ] Test Windows and macOS release archives on clean machines.
- [ ] Verify database migrations and restart recovery from the latest supported release.
- [ ] Verify public tunnel behavior with HTTPS and a disposable, least-privilege profile.
- [ ] Verify the extension in a dedicated browser profile and document the supported browsers.
- [ ] Generate software-bill-of-materials or dependency-license reports if required by the distributor.

## Documentation and community

- [ ] Review current README screenshots before every release and redact personal or sensitive information when necessary.
- [ ] Verify all relative links and public URLs after the repository becomes public.
- [ ] Review `README.md`, `SECURITY.md`, `SUPPORT.md`, `CONTRIBUTING.md`, `GOVERNANCE.md`, and the Code of Conduct.
- [ ] Confirm the private security and conduct-reporting channels actually work.
- [ ] Publish a first changelog section, signed/annotated tag, release notes, checksums, and supported-platform statement.
- [ ] Triage initial issues and clearly label good first issues where appropriate.

## Final publication review

- [ ] Clone the future-public repository into a clean directory using an unauthenticated session.
- [ ] Build and test from that clone without relying on ignored files or global private configuration.
- [ ] Inspect the rendered README, Mermaid diagram, badges, issue templates, and license page.
- [ ] Download and verify every release artifact and checksum as an anonymous user.
- [ ] Announce the supported version and avoid promising warranties or service levels beyond `SUPPORT.md`.
