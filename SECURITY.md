# Security Policy

ChatCMD intentionally gives AI clients controlled access to terminals, files, Git repositories, processes, tasks, and other local resources. Security reports are taken seriously.

## Supported versions

| Version | Supported |
| --- | --- |
| Latest release on `main` | Yes |
| Current `dev` branch | Best effort |
| Older releases or forks | No |

Security fixes are normally prepared for the latest maintained code. Maintainers may choose to backport a critical fix, but no backport is guaranteed.

## Report a vulnerability privately

Do not open a public issue for a suspected vulnerability and do not include live secrets, private files, or conversation data in a public report.

Use [GitHub's private vulnerability reporting form](https://github.com/int04/ChatCmdClient/security/advisories/new). If private reporting is unavailable, contact the maintainer through a non-public method listed on the [maintainer's GitHub profile](https://github.com/int04).

Include, when available:

- affected version, commit, operating system, and browser;
- required configuration and whether the server was loopback-only or publicly reachable;
- a minimal reproduction or proof of concept;
- expected and observed behavior;
- realistic impact and attack preconditions;
- suggested mitigation;
- whether any token, key, personal data, or third-party account may have been exposed.

Use dummy credentials and minimal test data. Do not access data that you do not own or have explicit permission to test.

## Coordinated disclosure

Maintainers will acknowledge the report when possible, validate it, establish severity and affected versions, and coordinate a fix and disclosure. Please allow reasonable remediation time before publishing details. Credit will be given if requested unless doing so would expose sensitive information.

## High-value areas

Reports are especially useful for:

- bypassing MCP profile authentication or per-tool authorization;
- leaking or recovering raw MCP tokens from storage, logs, API responses, or UI history;
- origin, host-header, tunnel, reverse-proxy, or local-management API bypasses;
- escaping canonical workspace or path restrictions, including through symlinks or race conditions;
- command or argument injection in shell, Git, process, folder-picker, or packaging flows;
- unauthorized browser-extension access to cookies, login tokens, tabs, conversations, or non-local callbacks;
- approval bypasses, confused-deputy behavior, sub-agent privilege escalation, or cross-task data leakage;
- unsafe deserialization, migration, encryption, or secret-handling behavior;
- denial of service caused by unbounded input, traversal, output, persistence, or concurrency.

## Security assumptions and exclusions

- A tokenized MCP endpoint is a bearer secret. Possession of the URL grants the access profile's allowed tools.
- The optional ChatGPT extension controls the DOM of an already signed-in tab. A malicious browser extension, compromised browser profile, or local administrator is outside its trust boundary.
- Local API and WebSocket encryption is defense in depth against casual inspection. It cannot protect plaintext from code executing inside the same browser or a compromised machine.
- Public exposure through a tunnel or port forward changes the threat model. Operators are responsible for HTTPS, tunnel access policy, firewalling, endpoint secrecy, and third-party terms.
- Social engineering, unsupported versions, deliberately disabled security controls, and vulnerabilities that require prior full control of the same operating-system account may be closed as out of scope unless they create a meaningful new boundary violation.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/ENCRYPTION_PROTOCOL.md](docs/ENCRYPTION_PROTOCOL.md) for implementation details.
