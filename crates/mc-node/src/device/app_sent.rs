//! Messages the app sends, kept in the device's own history so its screen shows the whole
//! conversation (`docs/spec/ui.md` §9). The app retries with the same timestamp: one entry per
//! message, its status following the attempts. An acknowledgement marks it delivered and still
//! goes to the app. Without one for two minutes after the last attempt, it shows as sent: the
//! app may still be trying, and only it knows when it gives up.

use mc_core::{Duration, Instant};

use super::{Device, Storage, UiEvent};
use crate::history::{HISTORY_TEXT, Msg, Peer, Status};
use crate::node::Sent;

/// App messages followed at a time.
pub const APP_SENT: usize = 8;
/// After this long without an acknowledgement, the entry is left as sent.
const GIVE_UP: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy)]
pub(super) struct AppSent {
    seq: u32,
    prefix: [u8; 6],
    ts: u32,
    acks: [u32; 4],
    last: Instant,
}

impl Device {
    /// The app sent plain text to a contact (`SEND_TXT_MSG`, type 0).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn app_sent_text(
        &mut self,
        now: Instant,
        prefix: &[u8; 6],
        attempt: u8,
        ts: u32,
        text: &[u8],
        sent: &Sent,
        st: &mut impl Storage,
    ) {
        let status = Status::Sending {
            attempt: attempt.min(3),
        };
        if let Some(e) = self
            .app_sent
            .iter_mut()
            .find(|e| e.prefix == *prefix && e.ts == ts)
        {
            // A retry of a message already shown.
            e.acks.rotate_right(1);
            e.acks[0] = sent.ack;
            e.last = now;
            let seq = e.seq;
            if let Some(slot) = self.history.set_status(seq, status) {
                self.save_history_slot(slot, st);
            }
            self.notify(UiEvent::Status { seq });
            return;
        }
        let text = text.get(..text.len().min(HISTORY_TEXT)).unwrap_or(&[]);
        let (seq, slot) = self.history.push(Msg {
            seq: 0,
            peer: Peer::Contact(*prefix),
            outgoing: true,
            ts,
            at: self.node.time(now),
            status,
            path_len: if sent.flood { 0 } else { 0xFF },
            snr4: 0,
            text: heapless::Vec::from_slice(text).unwrap_or_default(),
        });
        self.save_history_slot(slot, st);
        if self.app_sent.is_full() {
            let old = self.app_sent.remove(0);
            self.leave_as_sent(old.seq, st);
        }
        let _ = self.app_sent.push(AppSent {
            seq,
            prefix: *prefix,
            ts,
            acks: [sent.ack, 0, 0, 0],
            last: now,
        });
        self.notify(UiEvent::Message {
            seq,
            peer: Peer::Contact(*prefix),
            outgoing: true,
        });
    }

    /// The app sent plain text to a channel (`SEND_CHANNEL_TXT_MSG`): nothing acknowledges it.
    pub(super) fn app_sent_channel(
        &mut self,
        now: Instant,
        idx: u8,
        ts: u32,
        text: &[u8],
        st: &mut impl Storage,
    ) {
        let text = text.get(..text.len().min(HISTORY_TEXT)).unwrap_or(&[]);
        let (seq, slot) = self.history.push(Msg {
            seq: 0,
            peer: Peer::Channel(idx),
            outgoing: true,
            ts,
            at: self.node.time(now),
            status: Status::Sent,
            path_len: 0,
            snr4: 0,
            text: heapless::Vec::from_slice(text).unwrap_or_default(),
        });
        self.save_history_slot(slot, st);
        self.notify(UiEvent::Message {
            seq,
            peer: Peer::Channel(idx),
            outgoing: true,
        });
    }

    /// An acknowledgement for a message from the app: its entry shows delivered.
    pub(super) fn app_sent_confirmed(&mut self, ack: u32, rtt_ms: u32, st: &mut impl Storage) {
        let Some(i) = self
            .app_sent
            .iter()
            .position(|e| e.acks.iter().any(|&a| a != 0 && a == ack))
        else {
            return;
        };
        let e = self.app_sent.remove(i);
        if let Some(slot) = self.history.set_status(e.seq, Status::Delivered { rtt_ms }) {
            self.save_history_slot(slot, st);
        }
        self.notify(UiEvent::Status { seq: e.seq });
    }

    /// App messages not acknowledged for two minutes are left as sent.
    pub(super) fn app_sent_tick(&mut self, now: Instant, st: &mut impl Storage) {
        while let Some(i) = self.app_sent.iter().position(|e| now >= e.last + GIVE_UP) {
            let e = self.app_sent.remove(i);
            self.leave_as_sent(e.seq, st);
        }
    }

    fn leave_as_sent(&mut self, seq: u32, st: &mut impl Storage) {
        if let Some(slot) = self.history.set_status(seq, Status::Sent) {
            self.save_history_slot(slot, st);
        }
        self.notify(UiEvent::Status { seq });
    }
}
