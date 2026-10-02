//! Quarantine acceptance (admin path) for [`Bindings`].

use super::*;

impl Bindings {
    pub(super) fn note_quarantine(&mut self, seq: u64, ev: &Event) {
        if let Event::Quarantine(q) = ev {
            self.latest_quarantine = Some((seq, q.quarantine.clone()));
        }
    }

    /// Admin acknowledgement of quarantined tails, naming the seq of the latest
    /// `journal.quarantine` record seen (`Journal::latest_pending_quarantine`);
    /// one acceptance clears every earlier quarantine too, but a quarantine
    /// appended after that seq keeps the journal read-only. `floor` is an explicit
    /// admin-chosen serial floor; it can only raise the floor (default: the
    /// clamped quarantine value already in the state) and may exceed the
    /// current last serial by at most 2^32. The constraining facts come from
    /// the signed quarantine record, never from the marker file.
    pub fn accept_truncate(
        &mut self,
        journal: &mut Journal,
        ack: AdminAck,
        quarantine_seq: Option<u64>,
        floor: Option<u64>,
    ) -> Result<(), BindError> {
        self.refuse_degraded()?;
        let max = self.last_serial.saturating_add(MAX_FLOOR_JUMP);
        let floor = floor.unwrap_or(self.last_serial);
        if floor > max {
            return Err(BindError::FloorTooHigh { floor, max });
        }
        let pending = self.quarantine_pending();
        let (marker_only, quarantine) = match (quarantine_seq, &self.latest_quarantine) {
            // File list comes from the signed quarantine record (the marker may be gone).
            (Some(seq), Some((latest, files))) if pending && seq == *latest => (false, files.clone()),
            (Some(seq), _) => return Err(BindError::NoSuchQuarantine(seq)),
            // Nothing pending: an explicit admin clear of a stale/unreadable marker.
            (None, _) if !pending && journal.lost().is_some() => (true, Vec::new()),
            (None, _) => return Err(BindError::NothingToAccept),
        };
        let ev = Event::Accept(AcceptBody {
            quarantine_seq,
            marker_only,
            serial_floor: floor,
            quarantine,
            by: ack.by.clone(),
        });
        self.write(journal, false, ev)?;
        journal.clear_lost(&ack)?;
        Ok(())
    }

    /// A quarantine record newer than the newest accepted one exists.
    pub(super) fn quarantine_pending(&self) -> bool {
        let latest = self.latest_quarantine.as_ref().map(|(s, _)| *s);
        latest.is_some() && self.accepted_through != latest
    }
}
