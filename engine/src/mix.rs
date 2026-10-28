//! The song's mixer: track strips, how they're wired (buses, sends, sidechain
//! keys) and plugin delay compensation. Live playback (dsp.rs) and export
//! (daw::render) both mix the song through `render_tracks`, so an export
//! sounds exactly like what you hear.
//!
//! Wiring. Every track's sound goes to the master or to a bus (a track that
//! others feed), and can also send some of it to buses. Python says who goes
//! where; `Wiring::build` (control side: it allocates) works out the order to
//! process them in (sources before the buses they feed, a sidechain's source
//! before the track it ducks), drops connections that would make a loop, and
//! the delay compensation.
//!
//! Delay compensation. A plugin that delays the sound (a limiter's look-ahead,
//! many VSTs) would put its track behind the beat. Each track therefore reads
//! the timeline early by exactly the delay between its input and the master:
//! its own rack plus whatever lies downstream. Where one track's paths
//! downstream differ (straight to the master, and through a bus with a slow
//! plugin), the faster path gets a delay line so both arrive together. So every
//! track reaches the master in step, with no added delay for the song. (What
//! you play live can't be read early: a slow plugin delays that by its latency,
//! as in any DAW.)

use crate::daw::{fader_gain, render_inst, AutoTarget, Timeline, MAX_TRACKS};
use crate::dsp::{pan_gains, StripSettings};
use crate::fx::Rack;
use crate::instr::{InstSlot, NoteEv};
use crate::jitter::MAX_FRAME;

pub const MAX_SENDS: usize = 4;
/// Connections per track: its output, then its sends.
const CONNS: usize = 1 + MAX_SENDS;

/// One mixer strip with a stereo signal path.
pub struct Strip {
    pub s: StripSettings,
    pub rack: Rack,
    pub inst: Option<Box<InstSlot>>,
    pub l: Vec<f32>,
    pub r: Vec<f32>,
    /// remote strips: last good block, for concealment
    pub last: Vec<f32>,
    pub level: f32,
    /// tracks: where the instrument's notes are read next (a jump means the
    /// loop went round: notes still sounding are let go)
    pub next_read: i64,
}

impl Strip {
    pub fn new(s: StripSettings) -> Self {
        Strip {
            s, rack: Rack::new(), inst: None,
            l: vec![0.0; MAX_FRAME], r: vec![0.0; MAX_FRAME], last: vec![0.0; MAX_FRAME],
            level: 0.0, next_read: i64::MIN,
        }
    }

    pub fn meter(&mut self, n: usize) {
        let pk = self.l[..n].iter().chain(&self.r[..n]).fold(0f32, |a, v| a.max(v.abs()));
        self.level = pk.max(self.level * 0.9);
    }
}

/// Where a track's sound goes, as the song says (track slots).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Route {
    /// a bus, or None for the master
    pub out: Option<usize>,
    /// (bus, level)
    pub sends: Vec<(usize, f32)>,
}

/// What the wiring is built from: per track slot.
pub struct WireDesc<'a> {
    pub active: [bool; MAX_TRACKS],
    pub routes: &'a [Route],
    /// samples each track's rack delays its sound by
    pub rack_lat: [u32; MAX_TRACKS],
    /// and its instrument (a VST instrument can have latency of its own)
    pub inst_lat: [u32; MAX_TRACKS],
    /// (track, the track its compressor listens to)
    pub keys: Vec<(usize, usize)>,
    /// the master rack's delay
    pub master_lat: u32,
    /// longest delay line allowed (samples)
    pub max_pad: usize,
}

/// A fixed stereo delay.
pub struct Delay2 {
    l: Vec<f32>,
    r: Vec<f32>,
    pos: usize,
}

impl Delay2 {
    fn new(d: usize) -> Self {
        Delay2 { l: vec![0.0; d], r: vec![0.0; d], pos: 0 }
    }
    fn run(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = self.l.len();
        for k in 0..l.len() {
            std::mem::swap(&mut l[k], &mut self.l[self.pos]);
            std::mem::swap(&mut r[k], &mut self.r[self.pos]);
            self.pos = (self.pos + 1) % n;
        }
    }
}

/// The audio thread's copy of the wiring (built on the control side, swapped in whole).
pub struct Wiring {
    pub order: [u8; MAX_TRACKS],
    pub n: usize,
    pub out: [Option<u8>; MAX_TRACKS],
    pub sends: [[Option<(u8, f32)>; MAX_SENDS]; MAX_TRACKS],
    /// read the timeline this much early: audio clips, instrument notes
    pub ahead_audio: [i64; MAX_TRACKS],
    pub ahead_notes: [i64; MAX_TRACKS],
    /// fed by another track (a bus): soloing a track keeps its buses audible
    pub fed: [bool; MAX_TRACKS],
    /// a delay on a connection: [slot * CONNS + (0: out, 1..: sends)]
    pads: Vec<Option<Delay2>>,
    /// the master rack's delay (the click and recording allow for it)
    pub master_lat: i64,
}

impl Wiring {
    /// Every track straight to the master, in slot order, nothing delayed.
    pub fn plain() -> Box<Wiring> {
        let routes = vec![Route::default(); MAX_TRACKS];
        Wiring::build(&WireDesc {
            active: [true; MAX_TRACKS], routes: &routes, rack_lat: [0; MAX_TRACKS], inst_lat: [0; MAX_TRACKS],
            keys: vec![], master_lat: 0, max_pad: 0,
        })
    }

    /// Work out the order and the delay compensation. Allocates: control side only.
    pub fn build(d: &WireDesc) -> Box<Wiring> {
        let alive = |i: usize| i < MAX_TRACKS && d.active[i];
        // edges src -> dest: routing (out, sends) and sidechain (key source -> keyed track)
        let mut adj = vec![Vec::<usize>::new(); MAX_TRACKS];
        let add = |adj: &mut Vec<Vec<usize>>, a: usize, b: usize| -> bool {
            if !alive(a) || !alive(b) || a == b {
                return false;
            }
            if reaches(adj, b, a) {
                return false; // would make a loop
            }
            if !adj[a].contains(&b) {
                adj[a].push(b);
            }
            true
        };
        let mut out = [None; MAX_TRACKS];
        let mut sends = [[None; MAX_SENDS]; MAX_TRACKS];
        for (i, r) in d.routes.iter().enumerate().take(MAX_TRACKS) {
            if !alive(i) {
                continue;
            }
            if let Some(o) = r.out {
                if add(&mut adj, i, o) {
                    out[i] = Some(o as u8);
                }
            }
            let mut k = 0;
            for &(b, level) in &r.sends {
                if k < MAX_SENDS && Some(b) != out[i].map(|o| o as usize) && add(&mut adj, i, b) {
                    sends[i][k] = Some((b as u8, level));
                    k += 1;
                }
            }
        }
        for &(t, src) in &d.keys {
            add(&mut adj, src, t);
        }
        // order: sources first (Kahn, lowest slot first among ready ones)
        let mut indeg = [0usize; MAX_TRACKS];
        for a in 0..MAX_TRACKS {
            for &b in &adj[a] {
                indeg[b] += 1;
            }
        }
        let mut order = [0u8; MAX_TRACKS];
        let mut n = 0;
        let mut done = [false; MAX_TRACKS];
        while n < MAX_TRACKS {
            let Some(i) = (0..MAX_TRACKS).find(|&i| !done[i] && indeg[i] == 0) else { break };
            done[i] = true;
            order[n] = i as u8;
            n += 1;
            for &b in &adj[i] {
                indeg[b] -= 1;
            }
        }
        // delay compensation, from the master back: down[i] = delay from track i's
        // output to the master input (the longest of its paths)
        let lat = |i: usize| d.rack_lat[i] as i64;
        let mut down = [0i64; MAX_TRACKS];
        let mut pads: Vec<Option<Delay2>> = (0..MAX_TRACKS * CONNS).map(|_| None).collect();
        let via = |down: &[i64; MAX_TRACKS], dest: Option<usize>| dest.map_or(0, |b| lat(b) + down[b]);
        for &i in order[..n].iter().rev() {
            let i = i as usize;
            let dests: Vec<Option<usize>> = std::iter::once(out[i].map(|o| o as usize))
                .chain(sends[i].iter().flatten().map(|&(b, _)| Some(b as usize)))
                .collect();
            down[i] = dests.iter().map(|&e| via(&down, e)).max().unwrap_or(0);
            // dests[k] is connection k: the output, then the sends in order
            for (conn, &e) in dests.iter().enumerate() {
                let pad = (down[i] - via(&down, e)).max(0) as usize;
                if pad > 0 && d.max_pad > 0 {
                    pads[i * CONNS + conn] = Some(Delay2::new(pad.min(d.max_pad)));
                }
            }
        }
        let mut fed = [false; MAX_TRACKS];
        for i in 0..MAX_TRACKS {
            if let Some(o) = out[i] {
                fed[o as usize] = true;
            }
            for &(b, _) in sends[i].iter().flatten() {
                fed[b as usize] = true;
            }
        }
        let ahead_audio: [i64; MAX_TRACKS] = std::array::from_fn(|i| lat(i) + down[i]);
        let ahead_notes: [i64; MAX_TRACKS] = std::array::from_fn(|i| ahead_audio[i] + d.inst_lat[i] as i64);
        Box::new(Wiring { order, n, out, sends, ahead_audio, ahead_notes, fed, pads, master_lat: d.master_lat as i64 })
    }

    /// A send's level changed (the shape didn't): no rebuild needed.
    pub fn set_send(&mut self, track: usize, bus: usize, level: f32) {
        if let Some(s) = self.sends.get_mut(track).and_then(|ss| ss.iter_mut().flatten().find(|(b, _)| *b as usize == bus)) {
            s.1 = level;
        }
    }
}

/// Is `to` reachable from `from`?
fn reaches(adj: &[Vec<usize>], from: usize, to: usize) -> bool {
    let mut seen = [false; MAX_TRACKS];
    let mut stack = vec![from];
    while let Some(x) = stack.pop() {
        if x == to {
            return true;
        }
        if !std::mem::replace(&mut seen[x], true) {
            stack.extend(adj[x].iter().copied());
        }
    }
    false
}

/// Working buffers: what each bus has been fed this block, each track's
/// sound before its fader (a sidechain listens to it) and after (stems).
pub struct TrackBufs {
    in_l: Vec<Vec<f32>>,
    in_r: Vec<Vec<f32>>,
    key_l: Vec<Vec<f32>>,
    key_r: Vec<Vec<f32>>,
    pub post_l: Vec<Vec<f32>>,
    pub post_r: Vec<Vec<f32>>,
    tmp_l: Vec<f32>,
    tmp_r: Vec<f32>,
    /// what the live strips sent each bus this block (only playback has these)
    live_l: Vec<Vec<f32>>,
    live_r: Vec<Vec<f32>>,
    live_on: [bool; MAX_TRACKS],
}

impl TrackBufs {
    pub fn new() -> Self {
        let v = || (0..MAX_TRACKS).map(|_| vec![0f32; MAX_FRAME]).collect::<Vec<_>>();
        TrackBufs { in_l: v(), in_r: v(), key_l: v(), key_r: v(), post_l: v(), post_r: v(),
                    tmp_l: vec![0.0; MAX_FRAME], tmp_r: vec![0.0; MAX_FRAME],
                    live_l: v(), live_r: v(), live_on: [false; MAX_TRACKS] }
    }

    /// A new block: no live sends yet.
    pub fn clear_live(&mut self) {
        self.live_on = [false; MAX_TRACKS];
    }

    /// A live strip (yours or the partner's) sends `l`/`r` to a bus, times
    /// `gl`/`gr` (its fader, pan and send level). Realtime-safe.
    pub fn live_into(&mut self, bus: usize, l: &[f32], r: &[f32], gl: f32, gr: f32) {
        if bus >= MAX_TRACKS {
            return;
        }
        let n = l.len();
        if !self.live_on[bus] {
            self.live_on[bus] = true;
            self.live_l[bus][..n].fill(0.0);
            self.live_r[bus][..n].fill(0.0);
        }
        for k in 0..n {
            self.live_l[bus][k] += l[k] * gl;
            self.live_r[bus][k] += r[k] * gr;
        }
    }

    /// A track's sound for a sidechain (before its fader).
    pub fn key(&self, slot: usize, n: usize) -> crate::fx::Key<'_> {
        (slot < MAX_TRACKS).then(|| (&self.key_l[slot][..n], &self.key_r[slot][..n]))
    }
}

/// Left/right gains for a balance control: the far side turns down, the near one stays.
fn balance(pan: f32) -> (f32, f32) {
    ((1.0 - pan).min(1.0), (1.0 + pan).min(1.0))
}

/// Where the transport is in this block: one stretch, or two when a loop goes round.
pub struct Play {
    /// (offset in the block, timeline position, length)
    pub segs: [(usize, i64, usize); 2],
    pub nseg: usize,
    /// (start, end) while looping
    pub looping: Option<(i64, i64)>,
}

/// The timeline stretches a track reads this block: the transport's, `ahead`
/// samples early, wrapped into the loop. Returns how many.
fn read_segs(play: &Play, ahead: i64, out: &mut [(usize, i64, usize); 6]) -> usize {
    let mut m = 0;
    for &(off, p, len) in &play.segs[..play.nseg] {
        let (mut off, mut q, mut left) = (off, p + ahead, len);
        while left > 0 && m < out.len() {
            let take = match play.looping {
                Some((ls, le)) if le > ls => {
                    if q >= le {
                        q = ls + (q - le) % (le - ls);
                    }
                    left.min((le - q) as usize)
                }
                _ => left,
            };
            out[m] = (off, q, take);
            m += 1;
            off += take;
            q += take as i64;
            left -= take;
        }
    }
    m
}

/// The song's tracks for one block of `n` samples: each plays its stretch of
/// the timeline (or, stopped, only what's played live and tails), runs its
/// rack, and goes through its fader and pan to its bus or the master
/// (`mix_l` / `mix_r`, added to), plus its sends. Realtime-safe.
#[allow(clippy::too_many_arguments)]
/// Set everything the song's automation lanes move to where they are at `pos`
/// (once per block: before the tracks are rendered). Playing or not, so a
/// stopped song sounds the way it would at the playhead.
pub fn apply_automation(tl: &Timeline, pos: i64, tracks: &mut [Strip], master_rack: &mut Rack) {
    for lane in &tl.autos {
        let Some(v) = lane.value_at(pos) else { continue };
        match lane.target {
            AutoTarget::Fader(t) => {
                if let Some(s) = tracks.get_mut(t) {
                    s.s.fader = fader_gain(v);
                }
            }
            AutoTarget::Pan(t) => {
                if let Some(s) = tracks.get_mut(t) {
                    s.s.pan = 2.0 * v - 1.0;
                }
            }
            AutoTarget::Fx(t, pos, k) => {
                let rack = match t {
                    Some(t) => match tracks.get_mut(t) {
                        Some(s) => &mut s.rack,
                        None => continue,
                    },
                    None => &mut *master_rack,
                };
                if let Some(slot) = rack.slots.get_mut(pos) {
                    if k < slot.cfg.p.len() {
                        slot.cfg.p[k] = v;
                    }
                }
            }
            AutoTarget::Inst(t, k) => {
                if let Some(inst) = tracks.get_mut(t).and_then(|s| s.inst.as_mut()) {
                    if k < inst.cfg.p.len() {
                        inst.cfg.p[k] = v;
                    }
                }
            }
        }
    }
}

pub fn render_tracks(tracks: &mut [Strip], w: &mut Wiring, b: &mut TrackBufs, tl: Option<&Timeline>,
                     play: Option<&Play>, n: usize, mix_l: &mut [f32], mix_r: &mut [f32]) {
    for s in 0..MAX_TRACKS {
        // a bus starts with what the live strips sent it (playing live: never in an export)
        if b.live_on[s] {
            let TrackBufs { in_l, in_r, live_l, live_r, .. } = &mut *b;
            in_l[s][..n].copy_from_slice(&live_l[s][..n]);
            in_r[s][..n].copy_from_slice(&live_r[s][..n]);
        } else {
            b.in_l[s][..n].fill(0.0);
            b.in_r[s][..n].fill(0.0);
        }
    }
    let solo = tracks.iter().any(|t| t.s.active && t.s.solo);
    let mut segs = [(0usize, 0i64, 0usize); 6];
    for idx in 0..w.n {
        let i = w.order[idx] as usize;
        let Some(strip) = tracks.get_mut(i) else { continue };
        if !strip.s.active {
            strip.level = 0.0;
            b.post_l[i][..n].fill(0.0);
            b.post_r[i][..n].fill(0.0);
            b.key_l[i][..n].fill(0.0);
            b.key_r[i][..n].fill(0.0);
            continue;
        }
        let (l, r) = (&mut strip.l[..n], &mut strip.r[..n]);
        l.fill(0.0);
        r.fill(0.0);
        match (play, tl) {
            (Some(pl), Some(tl)) => {
                if let Some(inst) = strip.inst.as_mut() {
                    let m = read_segs(pl, w.ahead_notes[i], &mut segs);
                    for &(off, p, len) in &segs[..m] {
                        if p != strip.next_read {
                            inst.inst.note(NoteEv::Release); // the loop went round
                        }
                        render_inst(tl, i, inst, p, &mut l[off..off + len], &mut r[off..off + len]);
                        strip.next_read = p + len as i64;
                    }
                }
                let m = read_segs(pl, w.ahead_audio[i], &mut segs);
                for &(off, p, len) in &segs[..m] {
                    tl.render(i, p, &mut l[off..off + len], &mut r[off..off + len]);
                }
            }
            _ => {
                strip.next_read = i64::MIN;
                if let Some(inst) = strip.inst.as_mut() {
                    // stopped: still playable live, and tails ring out
                    let InstSlot { cfg, inst } = &mut **inst;
                    inst.process(&cfg.p, l, r);
                }
            }
        }
        // what buses were fed
        for k in 0..n {
            l[k] += b.in_l[i][k];
            r[k] += b.in_r[i][k];
        }
        let g = strip.s.gain;
        if g != 1.0 {
            for k in 0..n {
                l[k] *= g;
                r[k] *= g;
            }
        }
        {
            let (kl, kr) = (&b.key_l, &b.key_r);
            let keys = |slot: usize| (slot < MAX_TRACKS).then(|| (&kl[slot][..n], &kr[slot][..n]));
            strip.rack.process(l, r, &keys);
        }
        b.key_l[i][..n].copy_from_slice(l);
        b.key_r[i][..n].copy_from_slice(r);
        strip.meter(n);
        // fader and pan (a bus is fed by tracks, or by the live strips' sends)
        let fed = w.fed[i] || b.live_on[i];
        let audible = !strip.s.mute && (!solo || strip.s.solo || fed);
        // a track's own sound gets the pan law (-3 dB each side in the middle);
        // a bus passes on what's already panned: its pan is a balance, unity in the middle
        let (gl, gr) = if fed { balance(strip.s.pan) } else { pan_gains(strip.s.pan) };
        let f = strip.s.fader;
        for k in 0..n {
            b.post_l[i][k] = strip.l[k] * f * gl;
            b.post_r[i][k] = strip.r[k] * f * gr;
        }
        if !audible {
            continue; // (a muted track still has its stem)
        }
        // to its bus or the master, and its sends; delayed where compensation says so
        for c in 0..CONNS {
            let (dest, level) = if c == 0 {
                (w.out[i].map(|o| o as usize), 1.0)
            } else {
                match w.sends[i][c - 1] {
                    Some((bus, level)) => (Some(bus as usize), level),
                    None => continue,
                }
            };
            let (src_l, src_r): (&[f32], &[f32]) = match w.pads[i * CONNS + c].as_mut() {
                Some(pad) => {
                    b.tmp_l[..n].copy_from_slice(&b.post_l[i][..n]);
                    b.tmp_r[..n].copy_from_slice(&b.post_r[i][..n]);
                    pad.run(&mut b.tmp_l[..n], &mut b.tmp_r[..n]);
                    (&b.tmp_l[..n], &b.tmp_r[..n])
                }
                None => (&b.post_l[i][..n], &b.post_r[i][..n]),
            };
            let (dl, dr): (&mut [f32], &mut [f32]) = match dest {
                Some(bus) if bus != i => {
                    // (a bus is processed after everything feeding it)
                    let (il, ir) = (&mut b.in_l[bus], &mut b.in_r[bus]);
                    for k in 0..n {
                        il[k] += src_l[k] * level;
                        ir[k] += src_r[k] * level;
                    }
                    continue;
                }
                _ => (&mut mix_l[..n], &mut mix_r[..n]),
            };
            for k in 0..n {
                dl[k] += src_l[k] * level;
                dr[k] += src_r[k] * level;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(routes: &[Route], rack_lat: [u32; MAX_TRACKS]) -> WireDesc<'_> {
        WireDesc { active: [true; MAX_TRACKS], routes, rack_lat, inst_lat: [0; MAX_TRACKS], keys: vec![], master_lat: 0, max_pad: 48000 }
    }

    #[test]
    fn buses_come_after_what_feeds_them_and_loops_are_refused() {
        let mut routes = vec![Route::default(); MAX_TRACKS];
        routes[0] = Route { out: Some(5), sends: vec![(6, 0.5)] };
        routes[5] = Route { out: Some(6), sends: vec![] };
        routes[6] = Route { out: Some(0), sends: vec![] }; // would loop back: dropped
        let w = Wiring::build(&desc(&routes, [0; MAX_TRACKS]));
        let pos = |s: usize| w.order[..w.n].iter().position(|&x| x as usize == s).unwrap();
        assert!(pos(0) < pos(5) && pos(5) < pos(6));
        assert_eq!(w.out[6], None, "the loop isn't wired");
        assert!(w.fed[5] && w.fed[6] && !w.fed[0]);
        assert_eq!(w.n, MAX_TRACKS);
    }

    #[test]
    fn delay_compensation_lines_paths_up() {
        // track 0 -> bus 5 (a 100-sample plugin) -> master, and a send from 0 straight
        // to bus 6 (no delay); track 1 straight to the master with a 30-sample rack
        let mut routes = vec![Route::default(); MAX_TRACKS];
        routes[0] = Route { out: Some(5), sends: vec![(6, 1.0)] };
        let mut lat = [0; MAX_TRACKS];
        lat[5] = 100;
        lat[1] = 30;
        let w = Wiring::build(&desc(&routes, lat));
        assert_eq!(w.ahead_audio[0], 100, "track 0 reads 100 early");
        assert_eq!(w.ahead_audio[1], 30);
        assert_eq!(w.ahead_audio[5], 100);
        assert!(w.pads[0 * CONNS].is_none(), "the slow path gets no delay");
        assert_eq!(w.pads[0 * CONNS + 1].as_ref().unwrap().l.len(), 100, "the fast one waits for it");
    }

    #[test]
    fn read_segs_wrap_into_the_loop() {
        let play = Play { segs: [(0, 900, 128), (0, 0, 0)], nseg: 1, looping: Some((0, 1000)) };
        let mut out = [(0, 0, 0); 6];
        let m = read_segs(&play, 50, &mut out);
        assert_eq!(&out[..m], &[(0, 950, 50), (50, 0, 78)]);
        let m = read_segs(&play, 2050, &mut out); // further ahead than the loop is long
        assert_eq!(&out[..m], &[(0, 950, 50), (50, 0, 78)]);
        let open = Play { segs: [(0, 900, 128), (0, 0, 0)], nseg: 1, looping: None };
        let m = read_segs(&open, 50, &mut out);
        assert_eq!(&out[..m], &[(0, 950, 128)]);
    }

    #[test]
    fn live_sends_feed_a_bus_for_one_block() {
        // bus 5 (fader 1, centre) gets a live strip's signal; nothing else plays
        let mut tracks: Vec<Strip> = (0..MAX_TRACKS).map(|_| Strip::new(crate::dsp::StripSettings::track())).collect();
        tracks[5].s.active = true;
        tracks[5].s.fader = 1.0;
        let routes = vec![Route::default(); MAX_TRACKS];
        let mut w = Wiring::build(&desc(&routes, [0; MAX_TRACKS]));
        let mut b = TrackBufs::new();
        let n = 64;
        let (mut ml, mut mr) = (vec![0.0; n], vec![0.0; n]);
        b.clear_live();
        b.live_into(5, &vec![0.5; n], &vec![0.25; n], 1.0, 1.0);
        b.live_into(5, &vec![0.5; n], &vec![0.25; n], 1.0, 1.0); // a second strip adds up
        render_tracks(&mut tracks, &mut w, &mut b, None, None, n, &mut ml, &mut mr);
        assert!((ml[10] - 1.0).abs() < 1e-6 && (mr[10] - 0.5).abs() < 1e-6, "got {} / {}", ml[10], mr[10]);
        // the next block, without sends: silence (no leftovers)
        b.clear_live();
        let (mut ml, mut mr) = (vec![0.0; n], vec![0.0; n]);
        render_tracks(&mut tracks, &mut w, &mut b, None, None, n, &mut ml, &mut mr);
        assert!(ml.iter().chain(&mr).all(|&v| v == 0.0));
    }

    #[test]
    fn a_delay_line_delays() {
        let mut d = Delay2::new(3);
        let (mut l, mut r) = (vec![1.0, 2.0, 3.0, 4.0, 5.0], vec![0.0; 5]);
        d.run(&mut l, &mut r);
        assert_eq!(l, [0.0, 0.0, 0.0, 1.0, 2.0]);
    }
}
