# Workspace path safety and traversal policy

Filesystem requests are authorized after canonicalization. An absolute path is not an implicit
grant: it must be inside a configured workspace root or an explicit path scope derived from the
current task's user message. A file scope grants only that file; a directory scope grants its
subtree. Creation authorizes the canonical parent and validates the final component. Workspace
roots and explicit grant roots cannot be moved or deleted.

Source and destination paths are resolved independently. Runtime operations carry an internal
validated capability containing the canonical path or parent, authorized root, entry kind, access
intent, and a best-effort filesystem identity. The identity and parent are checked again immediately
before blocking reads or mutations. Recursive traversal and copies do not follow symbolic links.

## Platform guarantees

- Unix records device/inode plus size and modification time and rejects symbolic-link components.
  The implementation revalidates before use, but portable path-based syscalls still leave a narrow
  race window; it does not claim `openat2`-level race freedom.
- Windows rejects symbolic links, junctions, and other reparse points. Identity revalidation uses
  creation time, size, and modification time because stable `std` does not expose a portable held
  directory-handle workflow. It is best-effort rather than fully race-free.
- macOS follows the Unix policy. Filesystem Unicode normalization and case behavior remain those of
  the mounted filesystem; authorization always compares canonical paths.

## Shared ignore precedence

Search, find, and the turn file-change watcher share one default generated-directory list. Walkers
respect nested `.gitignore` rules (including negation) unless `includeIgnored` is enabled. Explicit
exclude patterns always win, including when ignored files are otherwise included. The requested
root itself is always visited, even when its name appears in the default list, so a direct-root
request remains usable. Symbolic links are never followed and traversal depth is bounded.
