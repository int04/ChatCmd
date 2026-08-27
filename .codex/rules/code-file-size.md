# Code file size rule

All source code files authored in this repository must stay at or below **500 lines**.

This rule applies at minimum to:
- Rust: `*.rs`
- TypeScript: `*.ts`
- TypeScript React: `*.tsx`

Requirements:
1. Do not create or leave an authored source file with more than 500 lines.
2. Before finishing any change, check every modified/new `.rs`, `.ts`, and `.tsx` file for line count.
3. If a file would exceed 500 lines, refactor it before completion by splitting cohesive responsibilities into smaller modules/files.
4. Prefer logical separation by responsibility (models, services, handlers, helpers, components, hooks, tests, etc.) instead of arbitrary line-based splitting.
5. Preserve behavior, public APIs, imports/exports, tests, and module wiring when splitting files.
6. Tests and source files are both subject to the 500-line maximum when they are authored in this repository.
7. Generated files, vendored dependencies, build outputs, package caches, and files under dependency/build directories are not required to be manually refactored.

**Hard limit:** 500 lines per authored Rust/TypeScript source file. A task is not considered complete while any modified or newly created applicable source file exceeds this limit.
