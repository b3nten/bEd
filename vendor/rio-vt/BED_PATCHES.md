Rio VT 0.5.28 from crates.io, source https://github.com/raphamorim/rio.

Bed limits encoded protocol buffers, file reads and decompression; checks decoded RGBA dimensions before image allocation; and applies a 64 MiB per-image decode limit. Partial Kitty uploads have a 64 MiB aggregate limit and at most 16 concurrent transfers; client-declared sizes do not cause speculative allocations. Large protocol buffers are released after their terminator. The session graphics budget is configured by Bed. Native atlas and Kitty admission rejects uploads when eviction cannot meet that budget; consumed flags let Bed display warnings for rejection or displayed-image eviction. Kitty replacements account for the old generation before calculating required storage. Atlas budget entries remain eviction candidates after pixels are handed to the frontend; eviction removes their native placements and never removes timestamps belonging to a different image namespace.

RIS resets dynamic color overrides and the cursor blink preference, preserving Bed's established reset behavior. These local changes do not replace the VT state machine. Original MIT and Apache source attribution is retained.

XTGETTCAP capability-name accumulation is capped at 64 KiB so requests spanning many PTY reads cannot create an unbounded parser buffer or an oversized reply. Oversized requests are consumed through their terminator and receive the normal small failure response; later terminal traffic continues.

Native graphics admission is capped at 1024 stored images and 4096 placement records across both screens. Existing image/placement IDs can still be replaced at capacity. Atlas clipping fragments obey the placement bound through the existing key recount transition, which releases the oldest excess fragments.
