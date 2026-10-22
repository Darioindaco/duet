//! Notes played live on a song track, sent to the partner: their side plays
//! them on the same track's instrument, so they hear you play a channel without
//! any of its audio going over the line (and it sounds the same on both sides).
//!
//! Tiny UDP packets, sent the moment a note is played. Each one carries the
//! last few events too, so a lost packet is covered by the next; after the last
//! event it's sent twice more, so a lost final note-off doesn't leave a note
//! hanging. Tracks are named by a key both sides derive from the track's id,
//! since the slot a track sits in can differ per side.

use std::collections::VecDeque;

use crate::instr::NoteEv;
use crate::net;

/// events per packet: the new one and the ones before it
pub const REDUNDANCY: usize = 4;
/// extra sends of the last packet after the last event (by the control loop)
pub const REPEATS: u8 = 2;
const ENTRY: usize = 7; // track key u32, kind u8, a u8, b u8

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ev {
    pub track: u32,
    pub ev: NoteEv,
}

fn encode(e: &Ev) -> Option<[u8; ENTRY]> {
    let (kind, a, b) = match e.ev {
        NoteEv::On(k, v) => (1, k, v),
        NoteEv::Off(k) => (2, k, 0),
        NoteEv::Sustain(on) => (3, on as u8, 0),
        NoteEv::AllOff => (4, 0, 0),
        NoteEv::Release => return None, // the transport's own business
    };
    let mut out = [0u8; ENTRY];
    out[..4].copy_from_slice(&e.track.to_be_bytes());
    out[4..].copy_from_slice(&[kind, a, b]);
    Some(out)
}

fn decode(b: &[u8]) -> Option<Ev> {
    let track = u32::from_be_bytes(b[..4].try_into().ok()?);
    let ev = match b[4] {
        1 => NoteEv::On(b[5] & 127, b[6].clamp(1, 127)),
        2 => NoteEv::Off(b[5] & 127),
        3 => NoteEv::Sustain(b[5] != 0),
        4 => NoteEv::AllOff,
        _ => return None,
    };
    Some(Ev { track, ev })
}

/// The sending side.
pub struct Out {
    seq: u32,
    recent: VecDeque<[u8; ENTRY]>,
    packet: Vec<u8>,
    repeats: u8,
}

impl Out {
    pub fn new() -> Self {
        Out { seq: rand::random(), recent: VecDeque::new(), packet: Vec::new(), repeats: 0 }
    }

    /// The packet to send for this event (None: not one that goes out).
    pub fn push(&mut self, e: Ev) -> Option<&[u8]> {
        let entry = encode(&e)?;
        self.seq = self.seq.wrapping_add(1);
        if self.recent.len() == REDUNDANCY {
            self.recent.pop_front();
        }
        self.recent.push_back(entry);
        let mut body = Vec::with_capacity(1 + REDUNDANCY * ENTRY);
        body.push(self.recent.len() as u8);
        for r in &self.recent {
            body.extend_from_slice(r);
        }
        net::pack(net::T_NOTE, self.seq, &body, &mut self.packet);
        self.repeats = REPEATS;
        Some(&self.packet)
    }

    /// The last packet again, while it has repeats left.
    pub fn repeat(&mut self) -> Option<&[u8]> {
        if self.repeats == 0 {
            return None;
        }
        self.repeats -= 1;
        Some(&self.packet)
    }

    /// A new partner: don't replay what was played to the old one.
    pub fn reset(&mut self) {
        self.recent.clear();
        self.repeats = 0;
    }
}

/// The receiving side: which events are new, and which notes the partner is
/// holding (to let go of if the line drops or too much was lost).
#[derive(Default)]
pub struct In {
    last: Option<u32>,
    held: Vec<(u32, u8)>,
    pedal: Vec<u32>,
}

impl In {
    /// The events to play from this packet, oldest first, into `out`.
    pub fn receive(&mut self, seq: u32, body: &[u8], out: &mut Vec<Ev>) {
        let Some(&n) = body.first() else { return };
        let n = n as usize;
        if n == 0 || n > REDUNDANCY || body.len() < 1 + n * ENTRY {
            return;
        }
        let first = seq.wrapping_sub(n as u32 - 1);
        let start = match self.last {
            // the first packet from this partner: only its newest event is current
            None => n - 1,
            Some(last) => {
                let ahead = seq.wrapping_sub(last) as i32;
                if ahead <= 0 {
                    return; // seen it (a repeat, or out of order)
                }
                if first.wrapping_sub(last) as i32 > 1 {
                    // more went missing than the packet carries: nothing may hang
                    self.release_all(out);
                    0
                } else {
                    n - ahead as usize
                }
            }
        };
        self.last = Some(seq);
        for i in start..n {
            if let Some(e) = decode(&body[1 + i * ENTRY..1 + (i + 1) * ENTRY]) {
                self.play(e, out);
            }
        }
    }

    fn play(&mut self, e: Ev, out: &mut Vec<Ev>) {
        match e.ev {
            NoteEv::On(k, _) => {
                if !self.held.contains(&(e.track, k)) {
                    self.held.push((e.track, k));
                }
                out.push(e);
            }
            NoteEv::Off(k) => {
                self.held.retain(|h| *h != (e.track, k));
                out.push(e);
            }
            NoteEv::Sustain(on) => {
                self.pedal.retain(|t| *t != e.track);
                if on {
                    self.pedal.push(e.track);
                }
                out.push(e);
            }
            // only what the partner holds: the track may be playing other notes here
            NoteEv::AllOff => self.release(|t| t == e.track, out),
            NoteEv::Release => {}
        }
    }

    fn release(&mut self, which: impl Fn(u32) -> bool, out: &mut Vec<Ev>) {
        for &(track, k) in self.held.iter().filter(|h| which(h.0)) {
            out.push(Ev { track, ev: NoteEv::Off(k) });
        }
        for &track in self.pedal.iter().filter(|t| which(**t)) {
            out.push(Ev { track, ev: NoteEv::Sustain(false) });
        }
        self.held.retain(|h| !which(h.0));
        self.pedal.retain(|t| !which(*t));
    }

    /// Let go of everything the partner was holding.
    pub fn release_all(&mut self, out: &mut Vec<Ev>) {
        self.release(|_| true, out);
    }

    /// Forget the sequence (a new partner, a reconnect), after release_all.
    pub fn reset(&mut self) {
        self.last = None;
    }

    pub fn held(&self) -> &[(u32, u8)] {
        &self.held
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(track: u32, k: u8) -> Ev {
        Ev { track, ev: NoteEv::On(k, 100) }
    }
    fn off(track: u32, k: u8) -> Ev {
        Ev { track, ev: NoteEv::Off(k) }
    }

    /// (seq, body) of a packet from Out
    fn parts(p: &[u8]) -> (u32, Vec<u8>) {
        let (t, seq, body) = net::unpack(p).unwrap();
        assert_eq!(t, net::T_NOTE);
        (seq, body.to_vec())
    }

    #[test]
    fn every_event_arrives_once_in_order() {
        let (mut o, mut i) = (Out::new(), In::default());
        let mut got = vec![];
        let sent = [on(7, 60), on(7, 64), off(7, 60), off(7, 64), on(9, 36)];
        for e in sent {
            let (seq, body) = parts(o.push(e).unwrap());
            i.receive(seq, &body, &mut got);
            i.receive(seq, &body, &mut got); // a repeat changes nothing
        }
        assert_eq!(got, sent);
        assert_eq!(i.held(), &[(9, 36)]);
    }

    #[test]
    fn a_lost_packet_is_covered_by_the_next() {
        let (mut o, mut i) = (Out::new(), In::default());
        let mut got = vec![];
        let (s, b) = parts(o.push(on(1, 60)).unwrap());
        i.receive(s, &b, &mut got);
        o.push(off(1, 60)); // lost
        o.push(on(1, 62)); // lost
        let (s, b) = parts(o.push(off(1, 62)).unwrap());
        i.receive(s, &b, &mut got);
        assert_eq!(got, [on(1, 60), off(1, 60), on(1, 62), off(1, 62)]);
        assert!(i.held().is_empty());
    }

    #[test]
    fn the_last_note_off_is_repeated() {
        let mut o = Out::new();
        let first = o.push(off(1, 60)).unwrap().to_vec();
        assert_eq!(o.repeat(), Some(&first[..]));
        assert_eq!(o.repeat(), Some(&first[..]));
        assert_eq!(o.repeat(), None);
    }

    #[test]
    fn too_much_lost_lets_held_notes_go() {
        let (mut o, mut i) = (Out::new(), In::default());
        let mut got = vec![];
        let (s, b) = parts(o.push(on(1, 60)).unwrap());
        i.receive(s, &b, &mut got);
        for k in 0..REDUNDANCY as u8 + 1 {
            o.push(on(2, 40 + k)); // all lost, the note-off of 60 among them
        }
        o.push(off(1, 60));
        let (s, b) = parts(o.push(on(3, 50)).unwrap());
        got.clear();
        i.receive(s, &b, &mut got);
        assert_eq!(got[0], off(1, 60), "let go of first: {got:?}");
        assert!(got.contains(&on(3, 50)));
    }

    #[test]
    fn all_off_and_a_dropped_line_only_touch_the_partners_notes() {
        let (mut o, mut i) = (Out::new(), In::default());
        let mut got = vec![];
        for e in [on(1, 60), on(2, 61), Ev { track: 1, ev: NoteEv::Sustain(true) }, Ev { track: 1, ev: NoteEv::AllOff }] {
            let (s, b) = parts(o.push(e).unwrap());
            i.receive(s, &b, &mut got);
        }
        assert_eq!(&got[3..], [off(1, 60), Ev { track: 1, ev: NoteEv::Sustain(false) }]);
        got.clear();
        i.release_all(&mut got);
        assert_eq!(got, [off(2, 61)]);
        assert!(o.push(Ev { track: 1, ev: NoteEv::Release }).is_none(), "the transport's release stays home");
    }

    #[test]
    fn a_new_partner_starts_fresh() {
        let (mut o, mut i) = (Out::new(), In::default());
        let mut got = vec![];
        o.push(on(1, 60));
        o.push(off(1, 60));
        let (s, b) = parts(o.push(on(1, 62)).unwrap());
        i.receive(s, &b, &mut got);
        assert_eq!(got, [on(1, 62)], "only the newest event of a first packet");
    }
}
