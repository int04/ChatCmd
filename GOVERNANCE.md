# Governance

ChatCMD currently follows a maintainer-led governance model designed for a small open-source project.

## Roles

### Contributors

Anyone who submits useful issues, documentation, code, tests, design work, translations, or reviews is a contributor.

### Reviewers

Reviewers are trusted contributors who regularly provide technically sound reviews in one or more areas. They may triage issues and recommend changes but do not merge without repository permission.

### Maintainers

Maintainers set release direction, protect the project's security and compatibility boundaries, review and merge contributions, manage releases, and enforce community policies. The current lead maintainer is [Nghia Duc (`@int04`)](https://github.com/int04).

## Decision process

- Routine fixes and documentation changes are decided through pull-request review.
- Significant protocol, security, persistence, compatibility, or architecture changes should begin with a public proposal issue.
- Maintainers seek practical consensus and document important trade-offs.
- When consensus cannot be reached in a reasonable time, the lead maintainer makes the final decision and explains it publicly when security or confidentiality does not prevent that.

Security embargoes, private conduct reports, legal matters, and unreleased credentials may be handled privately.

## Becoming a reviewer or maintainer

There is no application quota or required employment relationship. Maintainers consider sustained high-quality contributions, sound security judgment, respectful collaboration, review participation, and familiarity with the architecture. Access can be reduced or removed for inactivity, security needs, policy violations, or conflicts of interest.

## Branches and releases

- `dev` is the normal active-development and pull-request branch.
- `main` represents stable integrated code.
- Releases are tagged from reviewed stable history and documented in [CHANGELOG.md](CHANGELOG.md).

This model may evolve as the contributor community grows. Governance changes should be proposed and reviewed like other significant project changes.
