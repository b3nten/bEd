# bEd patches to arboard 3.6.1

Upstream: https://github.com/1Password/arboard (published crate 3.6.1).
Original MIT and Apache 2.0 licenses are retained alongside this file.

File clipboard exports on macOS and Linux retain absolute lexical paths so
symbolic links, including dangling links staged from remote projects, are copied
as links. All paths are validated before replacing clipboard contents, so a
conversion failure preserves the preceding clipboard. Both Linux X11 and Wayland
backends share the corrected URI-list serialization. Other clipboard APIs retain
upstream behavior.

Linux URI-list reads accept standard CRLF endings and localhost URLs, preserve
non-UTF-8 Unix filename bytes, and reject remote hosts and embedded NUL bytes.
Regression tests use a private macOS pasteboard so they preserve the user's
general clipboard.
