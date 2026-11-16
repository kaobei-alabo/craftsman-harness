# Terminal splash

The interactive startup card centers on **匠**, the character for an artisan
or craftsperson. Filled block strokes form its open enclosure and inner 斤;
half blocks keep the strokes distinct within six terminal rows. A lowercase
`craftsman` label and miniature hammer sit beside it, with a short caption about
building WebAssembly gadgets below.

The mark uses light gray, the label uses turquoise, and the hammer has a light
gray head and turquoise handle. These are the existing indexed terminal colors.

At ordinary widths the card includes the full mark, label, hammer and caption.
At narrower widths the caption shortens, then gives way to a smaller mark and
hammer. Very small layouts use the character itself. All rows fit the available
terminal cells; the standard startup card remains 15 rows so its connection
details and the composer fit a 24-row terminal.

The metadata beneath the artwork still comes from the selected local model
configuration, including version, agent, model, endpoint, key presence and
permission mode. Key values are hidden. Startup does not submit a model task,
and the splash continues through the normal SafeBackend renderer.

The artwork and responsive layout live in
`crates/craftsman-rs/src/tui/render.rs`; `tui/app.rs` accounts for the same six
art rows when estimating transcript height. The README image is captured from
the CLI in an isolated native terminal. Existing splash tests cover the border,
mark, small hammer, metadata styles, narrow caption and widths from 0 to 144,
including the unchanged standard startup height.
