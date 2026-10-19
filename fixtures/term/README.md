# Terminal goldens (gate for unit T1)

`src/terminalPreview.ts` on `@xterm/headless` is the oracle; T1 reimplements it on `alacritty_terminal` and must
reproduce these files.

- `streams/<name>.bin` — raw PTY master bytes, exactly as they arrived.
- `streams/<name>.json` — meta (`parity/gen/streams/format.ts` `StreamMeta`): initial `cols`/`rows`, `chunks` (byte
  length of each `onData` chunk, in order; they sum to the .bin size), `events` (`resize` / `checkpoint`, each at a byte
  offset on a chunk boundary, applied in listed order), optional `scrollOffsets`, and how it was recorded.
- `<name>.golden.json` — what the TS oracle produced for that stream (`parity/gen/term.gen.ts`).

Regenerate: `node --import tsx parity/gen/term.gen.ts` (`--check` exits 1 on drift). Deterministic. Recording new
streams (`node --import tsx parity/gen/streams/record.ts [name...]`) is separate and changes the inputs.

## Corpus

| stream | source | covers |
|---|---|---|
| `sh-ls-colors` | /bin/sh via node-pty | `ls -G` colours (dirs, symlinks, executables, other-writable), columns, `clear` (3J H 2J) |
| `sh-sgr` | /bin/sh | SGR 16/bright fg+bg, 256 palette, truecolor (`;` and `:` forms), bold/dim/italic/underline (incl. `4:3`, `21`, underline colour)/blink/inverse/hidden/strike/overline and their resets |
| `sh-cursor` | /bin/sh | CUP/CUU/CUD/CUF/CUB/CHA/VPA, EL 0/1/2, ED 0/1, DECSTBM scrolling + RI, tabs + HTS/TBC, ICH/DCH/ECH, DECSC/DECRC and CSI s/u, IL/DL |
| `sh-unicode` | /bin/sh | CJK, emoji (ZWJ, flags, VS15/VS16, keycap), combining marks, exact-width lines, wide char at the last column, readline's own wrapping, two resizes (reflow) |
| `sh-modes` | /bin/sh | alt screen 1049/47/1047 and `tput smcup/rmcup`, DEC 2026, bracketed paste, mouse/focus/app-cursor/keypad, cursor visibility, OSC 0/2 titles, OSC 8 links |
| `sh-scrollback` | /bin/sh | 150-line scrollback, a 200-column line, resizes narrower/wider-taller/shorter, deep scroll offsets |
| `lazygit` | lazygit 0.x in a throwaway repo (temp HOME/XDG, popups and update checks off) | full-screen TUI: panels, a commit's files, the keybindings menu, a resize, quit |
| `synthetic-claude-ink` | synthetic | Ink log-update (2K + cursor-up erase, `\n`→`\r\n`), DEC 2026 frames, static history, a live frame taller than the screen, clear-and-rerender on resize, kitty keyboard push/pop |
| `synthetic-codex-inline` | synthetic | ratatui inline viewport: DSR query, history inserted through a scroll region, cell-diff CUP/SGR redraws, viewport reaching the bottom, resize |
| `synthetic-codex-alt` | synthetic | ratatui alternate-screen view over normal-screen history, mouse modes, diff scrolling, resize inside alt, leaving alt |
| `synthetic-pi-diff` | synthetic | pi-tui differential renderer: repaint from the first changed line, `J` when shorter, full `3J 2J H` repaint when the change is above the viewport |
| `synthetic-utf8-split` | synthetic | UTF-8 split at every byte position, split CSI/OSC, invalid bytes, a truncated sequence at EOF |
| `synthetic-edge-cases` | synthetic | 20x6 parser edge cases (pending wrap, DECAWM off, wide at edge, tabs, editing, origin mode, IRM, DECALN, DCS/APC/PM/OSC 52 ignored, ED 3, RIS) and clamped/no-op resizes |
| `synthetic-c1-controls` | synthetic | C1 controls as UTF-8 code points (kept apart: emulators disagree) |
| `synthetic-scrollback-reflow` | synthetic | wrapped styled lines through resizes in both directions, resize inside the alternate screen |

The real claude, codex and pi were never run; their streams imitate their renderers (`parity/gen/streams/synthetic.ts`).

## Replay contract (how the goldens were made)

The golden `ops` list is the exact sequence to replay:

- `{"op":"new","cols","rows"}` — `new TerminalPreview(cols, rows)` (scrollback 10 000). The constructor and `resize`
  clamp: non-finite → fallback, else `max(1, floor(n))`.
- `{"op":"write","from","to"}` — bytes `[from, to)` of the .bin go through ONE streaming UTF-8 decoder per stream
  (Node `string_decoder`, which node-pty uses via `socket.setEncoding('utf8')`: invalid bytes → U+FFFD, an incomplete
  trailing sequence is held for the next chunk), and the decoded text is passed to `TerminalPreview.write` without
  awaiting. `"empty": true` marks a chunk that decoded to nothing (node-pty then emits no data event, so nothing was
  written).
- `{"op":"eof"}` — present when the decoder still held an incomplete sequence at the end: `decoder.end()` (U+FFFD) is
  written as one last chunk.
- `{"op":"resize","cols","rows"}` — `TerminalPreview.resize(cols, rows)`, not awaited; it is always followed by a
  checkpoint (`after-resize-<c>x<r>`), as a worker snapshots the preview after resizing it.
- `{"op":"checkpoint","label","at","screen","calls"}` — all queued writes/resizes are settled (awaiting the preview's
  internal promise chain, which does not touch its snapshot cache), then:
  - `screen`: the preview's own xterm terminal (see below), captured before any call;
  - `calls`: these TerminalPreview calls in this order, with their results:
    `getCachedSnapshot()`, `getAnsiFrame()`, `getCachedSnapshot()`, then for each `s` of `scrollOffsets`
    `getSnapshot(s)` and `getScrollInfo(s)`, then `getCachedSnapshot()`, `getSnapshot()`, `getScrollInfo()`.
    The order matters: `getAnsiFrame` refreshes the cached snapshot (at offset 0) only when dirty, and
    `getCachedSnapshot` returns whatever the last refresh produced.

A final checkpoint `end` follows the last write.

### `screen`

| field | meaning |
|---|---|
| `cols`, `rows` | the terminal's size (xterm enforces at least 2 columns, so it can differ from the preview's own clamp to 1: see `synthetic-edge-cases` `after-resize-0x0`) |
| `buffer` | `normal` or `alternate` |
| `cursor` | `{x, y}` viewport-relative; `x` may equal `cols` (pending wrap after writing the last column) |
| `baseY`, `viewportY`, `length` | xterm buffer metrics (`baseY` = scrollback lines) |
| `lines` | the `rows` viewport rows from `baseY`, `translateToString(true)` (right-trimmed) |
| `wrapped` | viewport row indexes whose line continues the previous one (`isWrapped`) |
| `attrs` | per row index (string key): non-default attribute runs `[x, cells, fg, bg, flags]`; cells outside a run are default |
| `wide` | per row index: columns holding a width-2 cell |
| `scrollback`, `scrollbackWrapped` | the active buffer's scrollback lines (trimmed) and which of them are wrapped |
| `modes` | `terminal.modes` (xterm 5.5 has no DEC 2026 mode: synchronized output is a no-op there) |
| `title` | the last OSC 0/2 title, or null (OSC 1 icon names do not count) |

Colours: `d` default, `p16:N` (SGR 30-37/90-97 and 40-47/100-107 → 0-15), `p256:N`, `rgb:rrggbb`. Flags, in this
order: `b` bold, `d` dim, `i` italic, `u` underline (any style), `k` blink, `r` inverse, `h` invisible, `s` strikethrough,
`o` overline. OSC 8 hyperlinks and underline style/colour are not observable through the public cell API and are not
recorded.
