//! Co-op over deterministic lockstep — ADR 0091.
//!
//! **The simulation is already bit-identical across processes**, which is what
//! `cargo xtask repeat` proves on 62 scenes and what the save format proves
//! across a load. That is the whole premise here: if two machines step the same
//! scene with the same inputs they get the same world, so the only thing that
//! has to cross the wire is **what the players asked for** — about 45 bytes a
//! tick each — and never the world itself.
//!
//! What this crate is:
//!
//! - A [`Session`], hosting or joined, pumped once a frame.
//! - Intents in, everyone's intents out, once every peer's have arrived.
//! - A snapshot channel for joining a game already in progress, which is the
//!   save format from ADR 0088 going down a socket.
//! - A hash exchange, so a desync is *reported* rather than silently played.
//!
//! What it is not: it does not know what a character is, does not step
//! anything, and holds no game state. It moves bytes and agrees on a tick.
//!
//! **TCP, deliberately.** The usual argument for UDP is that a real-time game
//! would rather drop a stale packet than wait for it. Lockstep is the case
//! where that is false: a tick cannot be simulated until every peer's input for
//! it has arrived, so a lost input must be retransmitted and the ones behind it
//! must not be applied first. That is TCP's contract exactly, and `std::net`
//! has it, so this crate needs no dependency to be correct.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};

/// Bumped when the wire format changes. A peer on another version is refused at
/// the door rather than allowed to desync in the third minute.
pub const PROTOCOL: u16 = 1;

/// Ticks between asking for something and it happening.
///
/// Lockstep cannot simulate a tick until every peer's input for it is in, so
/// each peer sends its intent this far ahead and the wire has that long to
/// deliver it. Eight ticks is 133 ms at 60 Hz: enough for a domestic
/// connection, short enough that the controls do not feel detached.
pub const INPUT_DELAY: u64 = 8;

/// Who a peer is. The host is always zero.
pub type PeerId = u8;

/// What one player asked for on one tick.
///
/// **Intent only.** Position, velocity and grounded live in `loom_script`'s
/// `Motion` too, and none of them belong here: they are what the simulation
/// *concluded*, and a peer that sent them would be asserting a world rather
/// than a request. Everything here is a thing a human did with their hands.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Intent {
    /// `[strafe, forward]`, each -1..=1.
    pub move_axis: [f32; 2],
    /// Flat facing, unit length.
    pub forward: [f32; 3],
    /// `forward` turned 90 degrees right.
    pub right: [f32; 3],
    /// Where the crosshair is, including up and down.
    pub aim: [f32; 3],
    /// jump | sprint | fire | interact, one bit each.
    pub buttons: u8,
    /// Which on-screen button was pressed this tick — ADR 0111.
    ///
    /// **A number, not a name, and that is the whole design.** A press has to
    /// travel as intent like every other thing a human did with their hands, and
    /// `Intent` is a fixed-width frame — 45 bytes before this — because a
    /// variable one is a length prefix, an allocation and a parser on the hot
    /// path of a lockstep tick. So the wire carries the **1-based index of the
    /// button among the scene's `Hud` button elements, in world order**, and
    /// zero for none.
    ///
    /// Well defined precisely because lockstep peers run the same scene: both
    /// resolve the index to the same node path, and a script sees a path rather
    /// than a number. 255 buttons is more than any HUD has, and the engine has
    /// no opinion about what they mean.
    pub ui: u8,
}

impl Intent {
    /// Bytes on the wire. Fixed width, little-endian, 46 bytes.
    const WIDTH: usize = 4 * 11 + 2;

    fn encode(&self, out: &mut Vec<u8>) {
        for value in self
            .move_axis
            .iter()
            .chain(&self.forward)
            .chain(&self.right)
            .chain(&self.aim)
        {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.push(self.buttons);
        out.push(self.ui);
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIDTH {
            return None;
        }
        let mut floats = [0.0_f32; 11];
        for (i, slot) in floats.iter_mut().enumerate() {
            *slot = f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().ok()?);
        }
        Some(Self {
            move_axis: [floats[0], floats[1]],
            forward: [floats[2], floats[3], floats[4]],
            right: [floats[5], floats[6], floats[7]],
            aim: [floats[8], floats[9], floats[10]],
            buttons: bytes[Self::WIDTH - 2],
            ui: bytes[Self::WIDTH - 1],
        })
    }

    /// Whether the jump bit is set.
    #[must_use]
    pub fn jump(&self) -> bool {
        self.buttons & 1 != 0
    }

    /// Whether the sprint bit is set.
    #[must_use]
    pub fn sprint(&self) -> bool {
        self.buttons & 2 != 0
    }

    /// Whether the fire bit is set.
    #[must_use]
    pub fn fire(&self) -> bool {
        self.buttons & 4 != 0
    }

    /// Whether the interact bit is set.
    #[must_use]
    pub fn interact(&self) -> bool {
        self.buttons & 8 != 0
    }

    /// Pack four booleans into the button byte.
    #[must_use]
    pub fn with_buttons(mut self, jump: bool, sprint: bool, fire: bool, interact: bool) -> Self {
        self.buttons = u8::from(jump)
            | (u8::from(sprint) << 1)
            | (u8::from(fire) << 2)
            | (u8::from(interact) << 3);
        self
    }
}

/// One thing that crossed the wire.
#[derive(Debug, Clone, PartialEq)]
enum Message {
    /// Client asks in, naming the protocol it speaks.
    Join { protocol: u16 },
    /// Host answers: who you are, and the tick the world will be at.
    Welcome { peer: PeerId, tick: u64 },
    /// The world, for a peer joining one already running.
    Snapshot { bytes: Vec<u8> },
    /// One peer's request for one tick.
    Input { tick: u64, peer: PeerId, intent: Intent },
    /// The peer set changes *at a named tick*, so every peer changes it at the
    /// same one. A membership change applied at different ticks on different
    /// machines is a desync that looks like a network fault.
    Roster { at: u64, peers: Vec<PeerId> },
    /// What one peer's world hashed to, for catching a desync early.
    Check { tick: u64, hash: u64 },
    /// A peer leaving on purpose, so it is not mistaken for a drop.
    Bye,
}

impl Message {
    fn tag(&self) -> u8 {
        match self {
            Self::Join { .. } => 1,
            Self::Welcome { .. } => 2,
            Self::Snapshot { .. } => 3,
            Self::Input { .. } => 4,
            Self::Roster { .. } => 5,
            Self::Check { .. } => 6,
            Self::Bye => 7,
        }
    }

    /// Length-prefixed, so a stream can be cut back into messages. TCP is a
    /// stream of bytes and not of sends, and a reader that assumed otherwise
    /// would work on loopback and fail on a real link.
    fn encode(&self) -> Vec<u8> {
        let mut body = vec![self.tag()];
        match self {
            Self::Join { protocol } => body.extend_from_slice(&protocol.to_le_bytes()),
            Self::Welcome { peer, tick } => {
                body.push(*peer);
                body.extend_from_slice(&tick.to_le_bytes());
            }
            Self::Snapshot { bytes } => body.extend_from_slice(bytes),
            Self::Input { tick, peer, intent } => {
                body.extend_from_slice(&tick.to_le_bytes());
                body.push(*peer);
                intent.encode(&mut body);
            }
            Self::Roster { at, peers } => {
                body.extend_from_slice(&at.to_le_bytes());
                body.extend_from_slice(peers);
            }
            Self::Check { tick, hash } => {
                body.extend_from_slice(&tick.to_le_bytes());
                body.extend_from_slice(&hash.to_le_bytes());
            }
            Self::Bye => {}
        }
        let mut out = Vec::with_capacity(body.len() + 4);
        out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    fn decode(body: &[u8]) -> Option<Self> {
        let (tag, rest) = body.split_first()?;
        let u64_at = |at: usize| -> Option<u64> {
            Some(u64::from_le_bytes(rest.get(at..at + 8)?.try_into().ok()?))
        };
        match tag {
            1 => Some(Self::Join {
                protocol: u16::from_le_bytes(rest.get(0..2)?.try_into().ok()?),
            }),
            2 => Some(Self::Welcome { peer: *rest.first()?, tick: u64_at(1)? }),
            3 => Some(Self::Snapshot { bytes: rest.to_vec() }),
            4 => Some(Self::Input {
                tick: u64_at(0)?,
                peer: *rest.get(8)?,
                intent: Intent::decode(rest.get(9..)?)?,
            }),
            5 => Some(Self::Roster { at: u64_at(0)?, peers: rest.get(8..)?.to_vec() }),
            6 => Some(Self::Check { tick: u64_at(0)?, hash: u64_at(8)? }),
            7 => Some(Self::Bye),
            _ => None,
        }
    }
}

/// One connection, with the half-read bytes that have not become a message yet.
struct Link {
    stream: TcpStream,
    peer: PeerId,
    inbox: Vec<u8>,
    /// Set when the socket has gone. Kept rather than removed immediately so a
    /// drop is reported once, in order, with everything else.
    gone: bool,
}

impl Link {
    fn new(stream: TcpStream, peer: PeerId) -> std::io::Result<Self> {
        stream.set_nonblocking(true)?;
        // Lockstep sends one small message per tick and needs it now. Nagle
        // would hold it back waiting for company that never comes.
        stream.set_nodelay(true)?;
        Ok(Self { stream, peer, inbox: Vec::new(), gone: false })
    }

    fn send(&mut self, message: &Message) {
        if self.gone {
            return;
        }
        if self.stream.write_all(&message.encode()).is_err() {
            self.gone = true;
        }
    }

    /// Read whatever has arrived and cut it into whole messages.
    fn receive(&mut self) -> Vec<Message> {
        let mut buffer = [0_u8; 8192];
        loop {
            match self.stream.read(&mut buffer) {
                Ok(0) => {
                    self.gone = true;
                    break;
                }
                Ok(n) => self.inbox.extend_from_slice(&buffer[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    self.gone = true;
                    break;
                }
            }
        }

        let mut out = Vec::new();
        while let Some(header) = self.inbox.get(0..4) {
            let Ok(header) = <[u8; 4]>::try_from(header) else { break };
            let length = u32::from_le_bytes(header) as usize;
            if self.inbox.len() < 4 + length {
                break;
            }
            if let Some(message) = Message::decode(&self.inbox[4..4 + length]) {
                out.push(message);
            }
            self.inbox.drain(0..4 + length);
        }
        out
    }
}

/// Something the game loop should know about.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A peer joined and wants the world as it stands. Answer with
    /// [`Session::send_snapshot`].
    NeedsSnapshot(PeerId),
    /// The world arrived, for a peer that has just joined one in progress.
    Snapshot { tick: u64, bytes: Vec<u8> },
    /// Somebody joined or left, effective at this tick on every machine.
    Roster { at: u64, peers: Vec<PeerId> },
    /// Two peers disagree about what the world looks like. The game is no
    /// longer shared; say so rather than play on.
    Desync { tick: u64, peer: PeerId, ours: u64, theirs: u64 },
    /// A peer's socket went away.
    Dropped(PeerId),
}

/// A hosted or joined game.
pub struct Session {
    listener: Option<TcpListener>,
    links: Vec<Link>,
    /// This machine's id. Zero when hosting.
    me: PeerId,
    hosting: bool,
    /// Who is expected to send an input, and from which tick.
    roster: BTreeSet<PeerId>,
    roster_from: u64,
    inputs: BTreeMap<u64, BTreeMap<PeerId, Intent>>,
    /// Our own hashes, kept so a late `Check` can still be compared.
    hashes: BTreeMap<u64, u64>,
    events: Vec<Event>,
    /// Next id to hand out, host only.
    next_peer: PeerId,
}

impl Session {
    /// Open a game others can join.
    ///
    /// # Errors
    /// If the address will not bind.
    pub fn host<A: ToSocketAddrs>(address: A) -> std::io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener: Some(listener),
            links: Vec::new(),
            me: 0,
            hosting: true,
            roster: BTreeSet::from([0]),
            roster_from: 0,
            inputs: BTreeMap::new(),
            hashes: BTreeMap::new(),
            events: Vec::new(),
            next_peer: 1,
        })
    }

    /// Join somebody else's.
    ///
    /// # Errors
    /// If the host will not answer.
    pub fn join<A: ToSocketAddrs>(address: A) -> std::io::Result<Self> {
        let stream = TcpStream::connect(address)?;
        let mut link = Link::new(stream, 0)?;
        link.send(&Message::Join { protocol: PROTOCOL });
        Ok(Self {
            listener: None,
            links: vec![link],
            // Not ours yet — the host names us in its `Welcome`.
            me: PeerId::MAX,
            hosting: false,
            roster: BTreeSet::new(),
            roster_from: 0,
            inputs: BTreeMap::new(),
            hashes: BTreeMap::new(),
            events: Vec::new(),
            next_peer: 0,
        })
    }

    /// This machine's peer id, once the host has named it.
    #[must_use]
    pub fn me(&self) -> PeerId {
        self.me
    }

    /// Whether this machine is the host.
    #[must_use]
    pub fn hosting(&self) -> bool {
        self.hosting
    }

    /// Who is expected to send inputs.
    #[must_use]
    pub fn roster(&self) -> Vec<PeerId> {
        self.roster.iter().copied().collect()
    }

    /// Accept arrivals, read sockets, and turn what came in into events.
    ///
    /// Call once a frame, before [`Session::ready`].
    pub fn poll(&mut self, tick: u64) {
        self.accept(tick);

        let mut relay = Vec::new();
        let mut received = Vec::new();
        for link in &mut self.links {
            for message in link.receive() {
                received.push((link.peer, message));
            }
        }
        for (from, message) in received {
            self.handle(from, message, tick, &mut relay);
        }

        // A host is the only path between two clients, so what one says the
        // others must hear.
        if self.hosting {
            for message in &relay {
                for link in &mut self.links {
                    link.send(message);
                }
            }
        }

        let dropped: Vec<PeerId> =
            self.links.iter().filter(|l| l.gone).map(|l| l.peer).collect();
        for peer in dropped {
            self.links.retain(|l| l.peer != peer);
            if self.hosting && self.roster.remove(&peer) {
                self.announce_roster(tick);
            }
            self.events.push(Event::Dropped(peer));
        }
    }

    fn accept(&mut self, tick: u64) {
        // Drained first, because everything below mutates `self` and the
        // listener is part of it.
        let mut incoming = Vec::new();
        if let Some(listener) = self.listener.as_ref() {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => incoming.push(stream),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }
        }

        for stream in incoming {
            {
                {
                    let peer = self.next_peer;
                    self.next_peer = self.next_peer.saturating_add(1);
                    match Link::new(stream, peer) {
                        Ok(link) => self.links.push(link),
                        Err(_) => continue,
                    }
                    // Named and rostered now; the world follows when the game
                    // hands us a snapshot.
                    if let Some(link) = self.links.last_mut() {
                        link.send(&Message::Welcome { peer, tick });
                    }
                    self.roster.insert(peer);
                    self.announce_roster(tick);
                    self.events.push(Event::NeedsSnapshot(peer));
                }
            }
        }
    }

    /// **Effective at a named future tick, not on arrival.** Every peer must
    /// start expecting the new member's input on the same tick or they will
    /// disagree about when a tick is complete — which stalls one machine and
    /// not the others, and looks exactly like a network fault.
    fn announce_roster(&mut self, tick: u64) {
        let at = tick + INPUT_DELAY * 2;
        self.roster_from = at;
        let peers: Vec<PeerId> = self.roster.iter().copied().collect();
        let message = Message::Roster { at, peers: peers.clone() };
        for link in &mut self.links {
            link.send(&message);
        }
        self.events.push(Event::Roster { at, peers });
    }

    fn handle(&mut self, from: PeerId, message: Message, tick: u64, relay: &mut Vec<Message>) {
        match message {
            Message::Join { protocol } => {
                if protocol != PROTOCOL
                    && let Some(link) = self.links.iter_mut().find(|l| l.peer == from)
                {
                    // A peer speaking another version would desync at the first
                    // thing that changed. Refused at the door instead.
                    link.send(&Message::Bye);
                    link.gone = true;
                }
            }
            Message::Welcome { peer, .. } => self.me = peer,
            Message::Snapshot { bytes } => {
                self.events.push(Event::Snapshot { tick, bytes });
            }
            Message::Input { tick: at, peer, intent } => {
                self.inputs.entry(at).or_default().insert(peer, intent);
                if self.hosting {
                    relay.push(Message::Input { tick: at, peer, intent });
                }
            }
            Message::Roster { at, peers } => {
                self.roster = peers.iter().copied().collect();
                self.roster_from = at;
                self.events.push(Event::Roster { at, peers });
            }
            Message::Check { tick: at, hash } => {
                if let Some(&ours) = self.hashes.get(&at)
                    && ours != hash
                {
                    self.events.push(Event::Desync { tick: at, peer: from, ours, theirs: hash });
                }
            }
            Message::Bye => {
                if let Some(link) = self.links.iter_mut().find(|l| l.peer == from) {
                    link.gone = true;
                }
            }
        }
    }

    /// Ask for something, [`INPUT_DELAY`] ticks from now.
    ///
    /// Recorded locally as well as sent: a peer is a participant in its own
    /// game and must not wait for the wire to tell it what it just did.
    pub fn send_intent(&mut self, tick: u64, intent: Intent) {
        let at = tick + INPUT_DELAY;
        let me = self.me;
        self.inputs.entry(at).or_default().insert(me, intent);
        let message = Message::Input { tick: at, peer: me, intent };
        for link in &mut self.links {
            link.send(&message);
        }
    }

    /// Everyone's intents for `tick`, or `None` while any are outstanding.
    ///
    /// **`None` means wait, not skip.** Simulating a tick without a peer's
    /// input is simulating a different game from the one they are playing.
    #[must_use]
    pub fn ready(&self, tick: u64) -> Option<Vec<(PeerId, Intent)>> {
        // Before a roster takes effect, the peers it names are not yet expected.
        let expected: BTreeSet<PeerId> = if tick < self.roster_from {
            self.inputs.get(&tick).map(|have| have.keys().copied().collect())?
        } else {
            self.roster.clone()
        };
        let have = self.inputs.get(&tick)?;
        if expected.is_empty() || !expected.iter().all(|peer| have.contains_key(peer)) {
            return None;
        }
        Some(have.iter().map(|(peer, intent)| (*peer, *intent)).collect())
    }

    /// Say what this machine's world hashed to, and forget the ticks behind it.
    pub fn report(&mut self, tick: u64, hash: u64) {
        self.hashes.insert(tick, hash);
        let message = Message::Check { tick, hash };
        for link in &mut self.links {
            link.send(&message);
        }
        // Ticks already simulated are never needed again, and a co-op session
        // that ran all evening should not be carrying every input it ever saw.
        let keep = tick.saturating_sub(INPUT_DELAY * 4);
        self.inputs.retain(|at, _| *at >= keep);
        self.hashes.retain(|at, _| *at >= keep);
    }

    /// Hand the world to a peer that has just joined.
    pub fn send_snapshot(&mut self, peer: PeerId, bytes: Vec<u8>) {
        let message = Message::Snapshot { bytes };
        if let Some(link) = self.links.iter_mut().find(|l| l.peer == peer) {
            link.send(&message);
        }
    }

    /// Everything that has happened since this was last called.
    pub fn drain(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Leave politely, so the others do not have to wait for a timeout.
    pub fn leave(&mut self) {
        for link in &mut self.links {
            link.send(&Message::Bye);
        }
    }

    /// The address being listened on, which is how a test finds a free port.
    ///
    /// # Errors
    /// If the socket has no address, or this session is not hosting.
    pub fn address(&self) -> std::io::Result<std::net::SocketAddr> {
        self.listener.as_ref().map_or_else(
            || Err(std::io::Error::other("not hosting")),
            TcpListener::local_addr,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field survives the wire, including the sign bits that a lazy
    /// encoder loses.
    #[test]
    fn an_intent_round_trips() {
        let intent = Intent {
            move_axis: [-1.0, 0.75],
            forward: [0.0, 0.0, -1.0],
            right: [1.0, 0.0, 0.0],
            aim: [0.1, -0.9, 0.42],
            buttons: 0,
            // A pressed on-screen button rides the same frame — ADR 0111. Not
            // zero, so a decoder that forgot the byte fails here rather than in
            // a game where one player's menu click never reached the other.
            ui: 7,
        }
        .with_buttons(true, false, true, false);

        let mut bytes = Vec::new();
        intent.encode(&mut bytes);
        assert_eq!(bytes.len(), Intent::WIDTH);
        let back = Intent::decode(&bytes).expect("decodes");
        assert_eq!(back, intent);
        assert!(back.jump() && back.fire());
        assert!(!back.sprint() && !back.interact());
    }

    /// **A truncated packet must not panic.** TCP hands over whatever arrived,
    /// and a decoder that indexed blindly would take the process down on a
    /// short read rather than wait for the rest.
    #[test]
    fn a_short_message_decodes_to_nothing_rather_than_panicking() {
        let full = Message::Input { tick: 9, peer: 2, intent: Intent::default() }.encode();
        for cut in 0..full.len() {
            // Skip the 4-byte length prefix; `decode` takes the body.
            let body = &full[4..];
            let short = &body[..cut.min(body.len())];
            let _ = Message::decode(short);
        }
    }

    #[test]
    fn every_message_round_trips() {
        let messages = [
            Message::Join { protocol: PROTOCOL },
            Message::Welcome { peer: 3, tick: 1234 },
            Message::Snapshot { bytes: vec![1, 2, 3, 4] },
            Message::Input { tick: 77, peer: 1, intent: Intent::default() },
            Message::Roster { at: 40, peers: vec![0, 1, 2] },
            Message::Check { tick: 5, hash: 0xdead_beef_cafe_f00d },
            Message::Bye,
        ];
        for message in messages {
            let framed = message.encode();
            let length = u32::from_le_bytes(framed[0..4].try_into().expect("prefix")) as usize;
            assert_eq!(length, framed.len() - 4, "length prefix is the body");
            assert_eq!(Message::decode(&framed[4..]).expect("decodes"), message);
        }
    }

    /// Pump both ends until something arrives or we give up. A loopback socket
    /// is fast but not instant, and a test that read once would be flaky.
    fn settle(host: &mut Session, client: &mut Session, tick: u64) {
        for _ in 0..200 {
            host.poll(tick);
            client.poll(tick);
            if client.me() != PeerId::MAX && !host.roster().is_empty() {
                // One more round so the roster reaches the client too.
                host.poll(tick);
                client.poll(tick);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// The whole point, end to end: two processes' worth of session over a real
    /// socket, each seeing the other's request for the same tick.
    #[test]
    fn two_peers_exchange_intents_for_the_same_tick() {
        let mut host = Session::host("127.0.0.1:0").expect("bind");
        let address = host.address().expect("address");
        let mut client = Session::join(address).expect("connect");
        settle(&mut host, &mut client, 0);

        assert!(host.hosting());
        assert_eq!(host.me(), 0);
        assert_eq!(client.me(), 1, "the host names the client");

        let at = 100;
        let mine = Intent::default().with_buttons(true, false, false, false);
        let theirs = Intent::default().with_buttons(false, false, true, false);
        host.send_intent(at, mine);
        client.send_intent(at, theirs);
        settle(&mut host, &mut client, at);

        let want = at + INPUT_DELAY;
        let on_host = host.ready(want).expect("host has both");
        let on_client = client.ready(want).expect("client has both");
        assert_eq!(on_host.len(), 2, "{on_host:?}");
        assert_eq!(on_host, on_client, "both machines see the same inputs");
        assert!(on_host.iter().any(|(p, i)| *p == 0 && i.jump()));
        assert!(on_host.iter().any(|(p, i)| *p == 1 && i.fire()));
    }

    /// **A missing input is a wait, not a skip.** This is the property the
    /// whole design rests on: simulating without a peer's input is simulating
    /// a different game from the one they are playing.
    #[test]
    fn a_tick_is_not_ready_until_every_peer_has_spoken() {
        let mut host = Session::host("127.0.0.1:0").expect("bind");
        let address = host.address().expect("address");
        let mut client = Session::join(address).expect("connect");
        settle(&mut host, &mut client, 0);

        let at = 500;
        host.send_intent(at, Intent::default());
        for _ in 0..20 {
            host.poll(at);
            client.poll(at);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            host.ready(at + INPUT_DELAY).is_none(),
            "one of two peers has spoken; that is not a complete tick"
        );

        client.send_intent(at, Intent::default());
        settle(&mut host, &mut client, at);
        assert!(host.ready(at + INPUT_DELAY).is_some(), "now both have");
    }

    /// Disagreement is reported rather than played through.
    #[test]
    fn a_mismatched_hash_is_reported_as_a_desync() {
        let mut host = Session::host("127.0.0.1:0").expect("bind");
        let address = host.address().expect("address");
        let mut client = Session::join(address).expect("connect");
        settle(&mut host, &mut client, 0);

        host.report(42, 0x1111_1111_1111_1111);
        client.report(42, 0x2222_2222_2222_2222);
        settle(&mut host, &mut client, 42);

        let found = host.drain().into_iter().chain(client.drain()).any(|event| {
            matches!(event, Event::Desync { tick: 42, ours, theirs, .. } if ours != theirs)
        });
        assert!(found, "a disagreement at tick 42 should be reported");
    }

    /// A joiner needs the world, and the host is told to send it.
    #[test]
    fn a_joining_peer_is_asked_for_a_snapshot() {
        let mut host = Session::host("127.0.0.1:0").expect("bind");
        let address = host.address().expect("address");
        let mut client = Session::join(address).expect("connect");
        settle(&mut host, &mut client, 7);

        let asked: Vec<PeerId> = host
            .drain()
            .into_iter()
            .filter_map(|e| match e {
                Event::NeedsSnapshot(peer) => Some(peer),
                _ => None,
            })
            .collect();
        assert_eq!(asked, vec![1], "the host is told who needs the world");

        host.send_snapshot(1, b"{\"tick\":7}".to_vec());
        settle(&mut host, &mut client, 7);
        let got = client
            .drain()
            .into_iter()
            .any(|e| matches!(e, Event::Snapshot { bytes, .. } if bytes == b"{\"tick\":7}"));
        assert!(got, "the joiner receives the world");
    }

    /// **Membership changes at a tick everyone agrees on.** Applied on arrival
    /// instead, two machines would start expecting a third peer's input on
    /// different ticks: one stalls, the other runs on, and it looks like a
    /// network fault rather than the logic error it is.
    #[test]
    fn a_roster_change_takes_effect_at_a_named_future_tick() {
        let mut host = Session::host("127.0.0.1:0").expect("bind");
        let address = host.address().expect("address");
        let mut client = Session::join(address).expect("connect");
        settle(&mut host, &mut client, 1000);

        let announced: Vec<u64> = host
            .drain()
            .into_iter()
            .filter_map(|e| match e {
                Event::Roster { at, .. } => Some(at),
                _ => None,
            })
            .collect();
        assert!(!announced.is_empty(), "a join changes the roster");
        assert!(
            announced.iter().all(|at| *at > 1000),
            "the change is in the future, not on arrival: {announced:?}"
        );
    }
}
