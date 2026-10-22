//! MIDI keyboards (midir). Connections live on a thread of their own, so
//! they're only ever touched where they were made; incoming messages go to a
//! callback that turns them into notes for the audio thread.

use std::sync::{mpsc, Arc, Mutex};

use crate::instr::NoteEv;

enum Req {
    List(mpsc::Sender<Vec<String>>),
    Open(Option<String>, mpsc::Sender<Result<(), String>>),
}

pub type Sink = Arc<dyn Fn(NoteEv) + Send + Sync>;

pub struct MidiThread {
    tx: Mutex<mpsc::Sender<Req>>,
}

impl MidiThread {
    pub fn spawn(sink: Sink) -> Self {
        let (tx, rx) = mpsc::channel::<Req>();
        std::thread::Builder::new()
            .name("duet-midi".into())
            .spawn(move || {
                let mut conn: Option<midir::MidiInputConnection<()>> = None;
                for req in rx {
                    match req {
                        Req::List(reply) => {
                            let names = midir::MidiInput::new("Duet")
                                .map(|m| m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect())
                                .unwrap_or_default();
                            let _ = reply.send(names);
                        }
                        Req::Open(name, reply) => {
                            drop(conn.take()); // closes the old one
                            let res = match name {
                                None => Ok(()),
                                Some(name) => open(&name, sink.clone()).map(|c| conn = Some(c)),
                            };
                            let _ = reply.send(res);
                        }
                    }
                }
            })
            .expect("spawn midi thread");
        MidiThread { tx: Mutex::new(tx) }
    }

    pub fn list(&self) -> Vec<String> {
        let (tx, rx) = mpsc::channel();
        if self.tx.lock().unwrap().send(Req::List(tx)).is_err() {
            return vec![];
        }
        rx.recv().unwrap_or_default()
    }

    /// Listen to the keyboard called `name` (None: to none).
    pub fn open(&self, name: Option<String>) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        self.tx.lock().unwrap().send(Req::Open(name, tx)).map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }
}

fn open(name: &str, sink: Sink) -> Result<midir::MidiInputConnection<()>, String> {
    let mut input = midir::MidiInput::new("Duet").map_err(|e| e.to_string())?;
    input.ignore(midir::Ignore::All); // no sysex, clock or active sensing
    let ports = input.ports();
    let port = ports
        .iter()
        .find(|p| input.port_name(p).ok().as_deref() == Some(name))
        .ok_or_else(|| format!("{name} isn't connected"))?;
    input
        .connect(port, "Duet in", move |_t, msg, _| {
            if let Some(ev) = parse(msg) {
                sink(ev);
            }
        }, ())
        .map_err(|e| e.to_string())
}

/// The messages Duet plays: notes, the sustain pedal, all-notes-off.
pub fn parse(msg: &[u8]) -> Option<NoteEv> {
    let (&status, data) = msg.split_first()?;
    match (status & 0xF0, data) {
        (0x90, [k, v, ..]) if *v > 0 => Some(NoteEv::On(*k & 127, *v & 127)),
        (0x90, [k, _, ..]) | (0x80, [k, ..]) => Some(NoteEv::Off(*k & 127)),
        (0xB0, [64, v, ..]) => Some(NoteEv::Sustain(*v >= 64)),
        (0xB0, [120 | 123, ..]) => Some(NoteEv::AllOff),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_messages_become_notes() {
        assert_eq!(parse(&[0x90, 60, 100]), Some(NoteEv::On(60, 100)));
        assert_eq!(parse(&[0x93, 60, 0]), Some(NoteEv::Off(60)), "note on with velocity 0 is a note off, any channel");
        assert_eq!(parse(&[0x80, 61, 40]), Some(NoteEv::Off(61)));
        assert_eq!(parse(&[0xB0, 64, 127]), Some(NoteEv::Sustain(true)));
        assert_eq!(parse(&[0xB0, 64, 0]), Some(NoteEv::Sustain(false)));
        assert_eq!(parse(&[0xB0, 123, 0]), Some(NoteEv::AllOff));
        assert_eq!(parse(&[0xB0, 7, 100]), None);
        assert_eq!(parse(&[0xE0, 0, 64]), None);
        assert_eq!(parse(&[]), None);
    }
}
