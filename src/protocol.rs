// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Local control channel between the wrapper and the Java bridge.
//!
//! Every packet is one type byte, a payload, and a NUL terminator. The
//! listener binds to IPv4 loopback only; a JVM must present the one-time key
//! generated for its launch before any other packet is accepted.

use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use crate::error::{Error, Result};

pub const START: u8 = 100;
pub const STOP: u8 = 101;
pub const RESTART: u8 = 102;
pub const PING: u8 = 103;
pub const STOP_PENDING: u8 = 104;
pub const START_PENDING: u8 = 105;
pub const STARTED: u8 = 106;
pub const STOPPED: u8 = 107;
pub const KEY: u8 = 110;
pub const BAD_KEY: u8 = 111;
pub const LOW_LOG_LEVEL: u8 = 112;
pub const SERVICE_CONTROL: u8 = 114;
pub const PROPERTIES: u8 = 115;
pub const LOG_BASE: u8 = 116;
pub const LOGFILE: u8 = 134;
pub const PAUSE: u8 = 138;
pub const RESUME: u8 = 139;
pub const GC: u8 = 140;

pub const MAX_PACKET_SIZE: usize = 1024 * 1024;
const MAX_PENDING_HANDSHAKES: usize = 8;
const EVENT_QUEUE_CAPACITY: usize = 256;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const BAD_KEY_MESSAGE: &str = "Authentication failed.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub code: u8,
    pub message: Vec<u8>,
}

impl Packet {
    #[must_use]
    pub fn text(code: u8, message: &str) -> Self {
        Self {
            code,
            message: message.as_bytes().to_vec(),
        }
    }

    /// Reads one complete packet from a blocking reader. Used by tests and
    /// simple clients; the runtime path uses [`Framer`].
    pub fn read_from(reader: &mut impl Read) -> Result<Option<Self>> {
        let mut framer = Framer::default();
        let mut byte = [0_u8; 1];
        loop {
            match reader.read(&mut byte) {
                Ok(0) => {
                    return if framer.is_idle() {
                        Ok(None)
                    } else {
                        Err(Error::Protocol("connection closed inside a packet".into()))
                    };
                }
                Ok(_) => {
                    if let Some(packet) = framer.feed(&byte)?.pop() {
                        return Ok(Some(packet));
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub fn write_to(&self, writer: &mut impl Write) -> Result<()> {
        if self.message.len() > MAX_PACKET_SIZE {
            return Err(Error::Protocol(format!(
                "packet larger than {MAX_PACKET_SIZE} bytes"
            )));
        }
        if self.message.contains(&0) {
            return Err(Error::Protocol(
                "a packet payload cannot contain a NUL byte".into(),
            ));
        }
        let mut frame = Vec::with_capacity(self.message.len() + 2);
        frame.push(self.code);
        frame.extend_from_slice(&self.message);
        frame.push(0);
        writer.write_all(&frame)?;
        writer.flush()?;
        Ok(())
    }

    #[must_use]
    pub fn message_lossy(&self) -> String {
        String::from_utf8_lossy(&self.message).into_owned()
    }
}

/// Incremental packet decoder. Partial packets survive across reads, so a
/// packet split over several TCP segments or interrupted by a read timeout is
/// never misinterpreted.
#[derive(Debug, Default)]
pub struct Framer {
    pending_code: Option<u8>,
    partial: Vec<u8>,
}

impl Framer {
    /// Feeds received bytes and returns every packet completed by them.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Packet>> {
        let mut completed = Vec::new();
        for &byte in bytes {
            match self.pending_code {
                None => self.pending_code = Some(byte),
                Some(code) if byte == 0 => {
                    completed.push(Packet {
                        code,
                        message: std::mem::take(&mut self.partial),
                    });
                    self.pending_code = None;
                }
                Some(_) => {
                    if self.partial.len() >= MAX_PACKET_SIZE {
                        return Err(Error::Protocol(format!(
                            "packet larger than {MAX_PACKET_SIZE} bytes"
                        )));
                    }
                    self.partial.push(byte);
                }
            }
        }
        Ok(completed)
    }

    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.pending_code.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiveEvent {
    Packet(Packet),
    Disconnected,
}

/// An authenticated JVM connection. Packets are decoded on a reader thread
/// and delivered through a bounded channel so the supervisor can wait on the
/// channel together with its other event sources.
pub struct Connection {
    writer: TcpStream,
    events: Receiver<ReceiveEvent>,
}

impl Connection {
    fn spawn(stream: TcpStream, framer: Framer, initial: Vec<Packet>) -> Result<Self> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(None)?;
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        let reader = stream.try_clone()?;
        let (sender, events) = crossbeam_channel::bounded(EVENT_QUEUE_CAPACITY);
        // Packets that arrived in the same read as the key must not be lost.
        for packet in initial {
            sender.try_send(ReceiveEvent::Packet(packet)).map_err(|_| {
                Error::Protocol("too many packets before the handshake completed".into())
            })?;
        }
        thread::Builder::new()
            .name("jss-protocol-reader".into())
            .spawn(move || reader_loop(reader, framer, &sender))?;
        Ok(Self {
            writer: stream,
            events,
        })
    }

    pub fn send(&mut self, code: u8, message: &str) -> Result<()> {
        Packet::text(code, message).write_to(&mut self.writer)
    }

    /// Channel of decoded packets; yields [`ReceiveEvent::Disconnected`] once
    /// and then disconnects when the peer closes the socket.
    #[must_use]
    pub fn events(&self) -> &Receiver<ReceiveEvent> {
        &self.events
    }

    /// Blocking receive used by tests and simple callers.
    pub fn receive(&self, timeout: Duration) -> Option<ReceiveEvent> {
        self.events.recv_timeout(timeout).ok()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.writer.shutdown(Shutdown::Both);
    }
}

fn reader_loop(mut stream: TcpStream, mut framer: Framer, sender: &Sender<ReceiveEvent>) {
    let mut buffer = [0_u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => match framer.feed(&buffer[..count]) {
                Ok(packets) => {
                    for packet in packets {
                        if sender.send(ReceiveEvent::Packet(packet)).is_err() {
                            return;
                        }
                    }
                }
                Err(_) => break,
            },
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::Interrupted | ErrorKind::WouldBlock | ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    let _ = sender.send(ReceiveEvent::Disconnected);
}

struct PendingHandshake {
    stream: TcpStream,
    framer: Framer,
    deadline: Instant,
}

pub struct BackendListener {
    listener: TcpListener,
    port: u16,
    pending: Vec<PendingHandshake>,
}

impl BackendListener {
    pub fn bind(configured: Option<u16>, minimum: u16, maximum: u16) -> Result<Self> {
        if let Some(port) = configured.filter(|port| *port != 0) {
            return Self::bind_port(port);
        }
        if maximum < minimum {
            return Err(Error::Config(format!(
                "wrapper.port.max ({maximum}) is lower than wrapper.port.min ({minimum})"
            )));
        }
        let mut last_error = None;
        for port in minimum..=maximum {
            match Self::bind_port(port) {
                Ok(listener) => return Ok(listener),
                Err(Error::Io(error)) if error.kind() == ErrorKind::AddrInUse => {
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.map_or_else(
            || Error::Protocol("no free port for the control channel".into()),
            Error::Io,
        ))
    }

    fn bind_port(port: u16) -> Result<Self> {
        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            port,
            pending: Vec::new(),
        })
    }

    #[cfg(test)]
    fn from_listener(listener: TcpListener) -> Self {
        let port = listener.local_addr().expect("listener address").port();
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        Self {
            listener,
            port,
            pending: Vec::new(),
        }
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Accepts new loopback connections and advances every pending handshake
    /// without blocking. Returns the first connection that presented the
    /// expected key. A connection that sends a wrong key, sends anything else
    /// first, or stays silent for `handshake_timeout` is dropped.
    pub fn poll_authentication(
        &mut self,
        expected_key: &str,
        handshake_timeout: Duration,
    ) -> Result<Option<Connection>> {
        loop {
            match self.listener.accept() {
                Ok((stream, peer)) => {
                    if !peer.ip().is_loopback() || self.pending.len() >= MAX_PENDING_HANDSHAKES {
                        continue;
                    }
                    if stream.set_nonblocking(true).is_err() || stream.set_nodelay(true).is_err() {
                        continue;
                    }
                    self.pending.push(PendingHandshake {
                        stream,
                        framer: Framer::default(),
                        deadline: Instant::now() + handshake_timeout,
                    });
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }

        let now = Instant::now();
        let mut index = 0;
        while index < self.pending.len() {
            match self.advance_handshake(index, expected_key, now)? {
                HandshakeStep::Keep => index += 1,
                HandshakeStep::Drop => {
                    self.pending.swap_remove(index);
                }
                HandshakeStep::Authenticated(initial) => {
                    let handshake = self.pending.swap_remove(index);
                    return Ok(Some(Connection::spawn(
                        handshake.stream,
                        handshake.framer,
                        initial,
                    )?));
                }
            }
        }
        Ok(None)
    }

    fn advance_handshake(
        &mut self,
        index: usize,
        expected_key: &str,
        now: Instant,
    ) -> Result<HandshakeStep> {
        let handshake = &mut self.pending[index];
        if now >= handshake.deadline {
            return Ok(HandshakeStep::Drop);
        }
        let mut buffer = [0_u8; 512];
        let count = match handshake.stream.read(&mut buffer) {
            Ok(0) => return Ok(HandshakeStep::Drop),
            Ok(count) => count,
            Err(error)
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) =>
            {
                return Ok(HandshakeStep::Keep);
            }
            Err(_) => return Ok(HandshakeStep::Drop),
        };
        let packets = match handshake.framer.feed(&buffer[..count]) {
            Ok(packets) => packets,
            Err(_) => return Ok(HandshakeStep::Drop),
        };
        let mut packets = packets.into_iter();
        let Some(first) = packets.next() else {
            return Ok(HandshakeStep::Keep);
        };
        if first.code == KEY && first.message.as_slice() == expected_key.as_bytes() {
            Ok(HandshakeStep::Authenticated(packets.collect()))
        } else {
            let _ = handshake.stream.set_nonblocking(false);
            let _ = handshake
                .stream
                .set_write_timeout(Some(Duration::from_millis(250)));
            let _ = Packet::text(BAD_KEY, BAD_KEY_MESSAGE).write_to(&mut handshake.stream);
            Ok(HandshakeStep::Drop)
        }
    }

    /// Blocking helper: polls until a JVM authenticates or `timeout` elapses.
    pub fn authenticate(&mut self, expected_key: &str, timeout: Duration) -> Result<Connection> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(connection) = self.poll_authentication(expected_key, timeout)? {
                return Ok(connection);
            }
            if Instant::now() >= deadline {
                return Err(Error::Protocol(
                    "the JVM did not authenticate the control channel in time".into(),
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

enum HandshakeStep {
    Keep,
    Drop,
    Authenticated(Vec<Packet>),
}

pub fn generate_key() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| Error::Protocol(format!("could not generate the channel key: {error}")))?;
    let mut key = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(key, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{BAD_KEY, BackendListener, Framer, KEY, PING, Packet, ReceiveEvent, START};

    #[test]
    fn packet_round_trip() {
        let packet = Packet::text(START, "start");
        let mut bytes = Vec::new();
        packet.write_to(&mut bytes).expect("encode packet");
        assert_eq!(bytes, [START, b's', b't', b'a', b'r', b't', 0]);
        assert_eq!(
            Packet::read_from(&mut bytes.as_slice()).expect("decode packet"),
            Some(packet)
        );
    }

    #[test]
    fn rejects_embedded_nul() {
        let packet = Packet {
            code: START,
            message: b"bad\0message".to_vec(),
        };
        assert!(packet.write_to(&mut Vec::new()).is_err());
    }

    #[test]
    fn framer_keeps_partial_packets_across_feeds() {
        let mut framer = Framer::default();
        assert!(framer.feed(&[PING, b'p', b'i']).expect("feed").is_empty());
        assert!(!framer.is_idle());
        let packets = framer
            .feed(&[b'n', b'g', 0, START, b's', 0, STARTED_PARTIAL])
            .expect("feed");
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0], Packet::text(PING, "ping"));
        assert_eq!(packets[1], Packet::text(START, "s"));
        assert!(!framer.is_idle());
        let packets = framer.feed(&[0]).expect("feed");
        assert_eq!(packets, [Packet::text(STARTED_PARTIAL, "")]);
        assert!(framer.is_idle());
    }

    const STARTED_PARTIAL: u8 = 106;

    #[test]
    fn slow_key_is_still_authenticated() {
        let mut listener = BackendListener::from_listener(
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind test listener"),
        );
        let port = listener.port();
        let client = thread::spawn(move || {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect");
            thread::sleep(Duration::from_millis(400));
            // Send the key in two fragments to exercise the framer.
            stream.write_all(&[KEY, b's', b'e']).expect("send key head");
            stream.flush().expect("flush");
            thread::sleep(Duration::from_millis(100));
            stream.write_all(b"cret\0").expect("send key tail");
            stream.flush().expect("flush");
            Packet::text(PING, "ping")
                .write_to(&mut stream)
                .expect("send ping");
            thread::sleep(Duration::from_millis(300));
        });
        let connection = listener
            .authenticate("secret", Duration::from_secs(3))
            .expect("slow key must authenticate");
        assert_eq!(
            connection.receive(Duration::from_secs(2)),
            Some(ReceiveEvent::Packet(Packet::text(PING, "ping")))
        );
        client.join().expect("client thread");
    }

    #[test]
    fn wrong_key_is_rejected_and_next_connection_can_authenticate() {
        let mut listener = BackendListener::from_listener(
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind test listener"),
        );
        let port = listener.port();
        let rejected = thread::spawn(move || {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect");
            Packet::text(KEY, "wrong")
                .write_to(&mut stream)
                .expect("send wrong key");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("read timeout");
            let mut response = Vec::new();
            let _ = stream.read_to_end(&mut response);
            response
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && listener.pending.is_empty() {
            listener
                .poll_authentication("secret", Duration::from_secs(1))
                .expect("poll");
            thread::sleep(Duration::from_millis(10));
        }
        while Instant::now() < deadline && !listener.pending.is_empty() {
            assert!(
                listener
                    .poll_authentication("secret", Duration::from_secs(1))
                    .expect("poll")
                    .is_none()
            );
            thread::sleep(Duration::from_millis(10));
        }
        let response = rejected.join().expect("rejected client");
        assert_eq!(response.first(), Some(&BAD_KEY));

        connect_with_key(port, "secret");
        listener
            .authenticate("secret", Duration::from_secs(2))
            .expect("correct key authenticates after a rejection");
    }

    #[test]
    fn silent_connection_is_dropped_after_the_handshake_timeout() {
        let mut listener = BackendListener::from_listener(
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind test listener"),
        );
        let port = listener.port();
        let _silent = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect");
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && listener.pending.is_empty() {
            listener
                .poll_authentication("secret", Duration::from_millis(200))
                .expect("poll");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(listener.pending.len(), 1);
        thread::sleep(Duration::from_millis(300));
        listener
            .poll_authentication("secret", Duration::from_millis(200))
            .expect("poll");
        assert!(listener.pending.is_empty());
    }

    #[test]
    fn peer_shutdown_is_reported_once() {
        let mut listener = BackendListener::from_listener(
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind test listener"),
        );
        let port = listener.port();
        connect_with_key(port, "secret");
        let connection = listener
            .authenticate("secret", Duration::from_secs(2))
            .expect("authenticated connection");
        assert_eq!(
            connection.receive(Duration::from_secs(2)),
            Some(ReceiveEvent::Disconnected)
        );
        assert_eq!(connection.receive(Duration::from_millis(200)), None);
    }

    fn connect_with_key(port: u16, key: &'static str) {
        thread::spawn(move || {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect");
            Packet::text(KEY, key)
                .write_to(&mut stream)
                .expect("send key");
            stream.flush().expect("flush key");
        })
        .join()
        .expect("client thread");
    }
}
