//! Byte-stream filter that strips OSC 7 (`ESC ] 7 ; <text> ST`) escape
//! sequences from a stream while preserving every other byte.
//!
//! Why we need this: when berth opens a remote workspace via ssh, the
//! remote shell's `precmd` / `PROMPT_COMMAND` typically emits OSC 7
//! reporting its cwd (e.g. `file://hostname/home/dev/foo`). Local
//! terminal emulators record that as the pane's cwd and use it as the
//! starting cwd for any new tab the user opens — but the path lives on
//! the *remote* host. On WSL, where the local user may not even own a
//! `/home/<remote-user>/` path, the new tab fails with EACCES on chdir.
//!
//! Berth emits its own OSC 7 pointing at a local marker directory the
//! shell hook recognises. By stripping OSC 7 from the ssh output stream
//! before it reaches the user's terminal, berth's marker stays
//! authoritative for new-tab cwd inheritance.
//!
//! We strip only OSC 7. Other OSC payloads (title-setting OSC 0/1/2,
//! 1337 user-vars, etc.) pass through unchanged.

use std::io::Write;

/// Streaming state machine. Feed bytes in via `filter`, write the
/// passing bytes out via the configured sink.
pub struct Osc7Filter<W: Write> {
    out: W,
    state: State,
    /// Holds bytes belonging to a partial OSC sequence whose Ps we don't
    /// yet know. Once we discover Ps==7 we drop them; for any other Ps
    /// (or a malformed sequence) we flush them and keep passing through.
    pending: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Normal byte stream, no escape sequence in progress.
    Normal,
    /// Just saw `\x1b`. Held in `pending`.
    AfterEsc,
    /// Saw `\x1b]`. Held in `pending`. Collecting digits of Ps.
    OscPs,
    /// Saw `\x1b]7;`. Dropping body bytes until terminator.
    DropBody,
    /// Saw `\x1b` while inside `DropBody`. If next byte is `\\` we have ST.
    DropAfterEsc,
    /// Saw `\x1b]<other>;`. Forwarding bytes until terminator (no drop).
    PassBody,
    /// Saw `\x1b` while inside `PassBody`. Forwarded; if next is `\\`, ST.
    PassAfterEsc,
}

const ESC: u8 = 0x1B;
const BEL: u8 = 0x07;

impl<W: Write> Osc7Filter<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            state: State::Normal,
            pending: Vec::with_capacity(16),
        }
    }

    /// Push bytes through the filter. Output flushes via the underlying
    /// writer; caller decides when to call `out.flush()`.
    pub fn filter(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        for &b in bytes {
            self.step(b)?;
        }
        Ok(())
    }

    fn step(&mut self, b: u8) -> std::io::Result<()> {
        match self.state {
            State::Normal => {
                if b == ESC {
                    self.pending.clear();
                    self.pending.push(ESC);
                    self.state = State::AfterEsc;
                } else {
                    self.out.write_all(&[b])?;
                }
            }
            State::AfterEsc => {
                if b == b']' {
                    self.pending.push(b);
                    self.state = State::OscPs;
                } else {
                    // Some other ESC sequence (CSI, single-shift, …).
                    // Pass it through and resume normal mode. The first
                    // byte AFTER ESC determined the sequence kind; we
                    // don't try to interpret CSI/etc. ourselves.
                    self.flush_pending()?;
                    self.out.write_all(&[b])?;
                    self.state = State::Normal;
                }
            }
            State::OscPs => {
                if b.is_ascii_digit() {
                    self.pending.push(b);
                } else if b == b';' {
                    // Commit on the separator. Ps is everything we've
                    // accumulated after the `\x1b]` prefix in pending.
                    let ps = &self.pending[2..];
                    if ps == b"7" {
                        // Begin dropping. Don't emit anything yet —
                        // including pending, including this `;`.
                        self.pending.clear();
                        self.state = State::DropBody;
                    } else {
                        // Pass through. Emit pending + `;`.
                        self.pending.push(b);
                        self.flush_pending()?;
                        self.state = State::PassBody;
                    }
                } else if b == BEL || b == ESC {
                    // Empty / malformed OSC ended right away. Pass
                    // everything we held through, including this byte,
                    // and resume normal mode.
                    self.flush_pending()?;
                    self.out.write_all(&[b])?;
                    self.state = if b == ESC {
                        State::PassAfterEsc
                    } else {
                        State::Normal
                    };
                } else {
                    // Non-digit, non-`;`: malformed but we'll pass it
                    // through and switch to PassBody. Some emulators
                    // accept OSC params with letters (e.g. `OSC L;…`).
                    self.pending.push(b);
                    self.flush_pending()?;
                    self.state = State::PassBody;
                }
            }
            State::DropBody => {
                if b == BEL {
                    self.state = State::Normal;
                } else if b == ESC {
                    self.state = State::DropAfterEsc;
                }
                // else: keep dropping
            }
            State::DropAfterEsc => {
                if b == b'\\' {
                    // ST consumed; sequence ends silently.
                    self.state = State::Normal;
                } else {
                    // Bare ESC inside OSC body. Stay dropping; if it
                    // started a new OSC/CSI we'd be re-entering the
                    // state machine, but practically terminals reject
                    // this and our job is just "swallow until
                    // terminator," so treat the inner ESC as part of
                    // the body and resume body-drop mode. If a new
                    // OSC began here we'd be eating it too, which is
                    // strictly more conservative than passing through.
                    self.state = State::DropBody;
                }
            }
            State::PassBody => {
                self.out.write_all(&[b])?;
                if b == BEL {
                    self.state = State::Normal;
                } else if b == ESC {
                    self.state = State::PassAfterEsc;
                }
            }
            State::PassAfterEsc => {
                self.out.write_all(&[b])?;
                if b == b'\\' {
                    self.state = State::Normal;
                } else {
                    self.state = State::PassBody;
                }
            }
        }
        Ok(())
    }

    fn flush_pending(&mut self) -> std::io::Result<()> {
        if !self.pending.is_empty() {
            self.out.write_all(&self.pending)?;
            self.pending.clear();
        }
        Ok(())
    }

    /// Flush the inner writer's buffer.
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter_str(input: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut f = Osc7Filter::new(&mut out);
            f.filter(input).unwrap();
        }
        out
    }

    #[test]
    fn plain_bytes_pass_through() {
        assert_eq!(filter_str(b"hello world\n"), b"hello world\n");
    }

    #[test]
    fn osc_7_bel_terminated_is_dropped() {
        let input = b"prefix\x1b]7;file://host/home/dev/foo\x07suffix";
        assert_eq!(filter_str(input), b"prefixsuffix");
    }

    #[test]
    fn osc_7_st_terminated_is_dropped() {
        let input = b"prefix\x1b]7;file://host/home/dev/foo\x1b\\suffix";
        assert_eq!(filter_str(input), b"prefixsuffix");
    }

    #[test]
    fn osc_0_title_passes_through() {
        let input = b"\x1b]0;my-title\x07rest";
        assert_eq!(filter_str(input), b"\x1b]0;my-title\x07rest");
    }

    #[test]
    fn osc_2_title_passes_through() {
        let input = b"\x1b]2;my-title\x1b\\rest";
        assert_eq!(filter_str(input), b"\x1b]2;my-title\x1b\\rest");
    }

    #[test]
    fn osc_1337_user_var_passes_through() {
        let input = b"\x1b]1337;SetUserVar=BERTH=abc\x07rest";
        assert_eq!(filter_str(input), b"\x1b]1337;SetUserVar=BERTH=abc\x07rest");
    }

    #[test]
    fn multibyte_ps_doesnt_match_7() {
        // OSC 71 is not OSC 7; must pass through.
        let input = b"\x1b]71;data\x07rest";
        assert_eq!(filter_str(input), b"\x1b]71;data\x07rest");
    }

    #[test]
    fn csi_sequence_unaffected() {
        let input = b"\x1b[31mred\x1b[0m";
        assert_eq!(filter_str(input), b"\x1b[31mred\x1b[0m");
    }

    #[test]
    fn split_across_calls_drops_correctly() {
        let mut out = Vec::new();
        {
            let mut f = Osc7Filter::new(&mut out);
            f.filter(b"\x1b]7;file://").unwrap();
            f.filter(b"host/home/").unwrap();
            f.filter(b"dev\x07after").unwrap();
        }
        assert_eq!(out, b"after");
    }

    #[test]
    fn split_terminator_st() {
        let mut out = Vec::new();
        {
            let mut f = Osc7Filter::new(&mut out);
            f.filter(b"\x1b]7;path\x1b").unwrap();
            f.filter(b"\\done").unwrap();
        }
        assert_eq!(out, b"done");
    }

    #[test]
    fn multiple_osc_in_one_stream() {
        let input = b"a\x1b]7;p1\x07b\x1b]2;title\x07c\x1b]7;p2\x1b\\d";
        assert_eq!(filter_str(input), b"ab\x1b]2;title\x07cd");
    }

    #[test]
    fn bare_esc_then_text_passes_through() {
        // An ESC followed by a non-`]` byte isn't OSC; pass it.
        assert_eq!(filter_str(b"\x1bM"), b"\x1bM");
    }

    #[test]
    fn empty_osc_passes_through() {
        // ESC ] BEL — no Ps. Pass through.
        assert_eq!(filter_str(b"\x1b]\x07"), b"\x1b]\x07");
    }
}
