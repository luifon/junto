//! The person's microphone in the calls as one state: whether the setting is closed, the
//! count of the gate of the engine with which it sends, each permission of an automatic
//! open and its owner, and the notice of a changed device (ADR 0739, ADR 0794). Every event
//! is one call of [`step`] under the lock of the calls.

/// What the calls hold of the person's microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Microphone {
    /// The person's setting is closed.
    pub muted: bool,
    /// The count of closes of the gate with which the engine sends while the setting is
    /// open. Every update of the controls carries it.
    pub open: Option<u64>,
    /// The sound off closed a microphone that was open: the count of the gate after that
    /// close, with which the sound on opens it again.
    pub sound_off: Option<u64>,
    /// A key of push to mute closed a microphone that was open, or the sound on gave it its
    /// permission while it was held: the count of the gate with which its release opens.
    pub hold: Option<u64>,
    /// A key of push to mute is held.
    pub held: bool,
    /// An automatic open found its permission from before a close of the gate that the
    /// calls have not read: the notice of the changed device is due when they read it.
    pub refused: bool,
    /// The replacements of the input device that the engine stated and the calls took.
    pub replacements: u64,
}

/// An open microphone, on a gate that has not closed.
impl Default for Microphone {
    fn default() -> Self {
        Self {
            muted: false,
            open: Some(0),
            sound_off: None,
            hold: None,
            held: false,
            refused: false,
            replacements: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// The person closes the microphone.
    Mute,
    /// The person opens it. Nothing refuses it.
    Unmute,
    /// The key of push to mute is held or let go.
    Press,
    Release,
    /// The sound goes off or on, with the setting "Turning the sound off also mutes the
    /// microphone".
    SoundOff,
    SoundOn,
    /// A rule of the call closes the setting: a grant of meeting mode, an end of a block that
    /// the session cannot read, a device that took the place of the open one.
    RuleClose,
    /// The gate closes and the setting stays as it is: the key of push to talk is let go, or
    /// a block of the microphone begins.
    Fence,
    /// The engine states this many replacements of the input device. It closed its gate for
    /// each while the setting was closed.
    Replaced(u64),
    /// The sound on will not open the microphone: the sound went on without the setting, or
    /// the setting changed.
    ForgetSoundOff,
    /// The release of the key will not open it: the call ended while it was held.
    ForgetHold,
}

/// What the caller does after a step, in the same lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Effects {
    /// Close the gate this many times.
    pub closes: u64,
    /// Tell the person that the device of the microphone changed.
    pub device_changed: bool,
}

/// The state after `event`, when the gate of the engine has closed `gate` times, and what
/// the caller does. A close advances the count of the gate by one. An automatic open opens
/// only with a permission that holds the count of the gate of this moment: any close after
/// the permission, also one of the engine that the calls have not read, keeps the
/// microphone closed.
pub(crate) fn step(mut mic: Microphone, event: Event, mut gate: u64) -> (Microphone, Effects) {
    let mut effects = Effects::default();
    let mut close = |mic: &mut Microphone, gate: &mut u64| {
        if !mic.muted {
            mic.muted = true;
            mic.open = None;
            *gate += 1;
            effects.closes += 1;
        }
    };
    match event {
        Event::Mute | Event::RuleClose => {
            close(&mut mic, &mut gate);
            mic.sound_off = None;
            mic.hold = None;
            mic.refused = false;
        }
        Event::Unmute => {
            mic.muted = false;
            mic.open = Some(gate);
            mic.sound_off = None;
            mic.hold = None;
            mic.refused = false;
        }
        Event::Press => {
            mic.held = true;
            if !mic.muted {
                let sent = sending(&mic, gate);
                close(&mut mic, &mut gate);
                mic.hold = Some(sent.unwrap_or(gate));
            }
        }
        Event::Release => {
            mic.held = false;
            if let Some(permission) = mic.hold.take() {
                open_with(&mut mic, permission, gate);
            }
        }
        Event::SoundOff => {
            let open = !mic.muted || mic.hold.is_some();
            let sent = sending(&mic, gate);
            close(&mut mic, &mut gate);
            if open {
                mic.sound_off = mic.hold.take().or(Some(sent.unwrap_or(gate)));
            }
        }
        Event::SoundOn => {
            if let Some(permission) = mic.sound_off.take() {
                if mic.held {
                    mic.hold = Some(permission);
                } else {
                    open_with(&mut mic, permission, gate);
                }
            }
        }
        Event::Fence => {
            let next = gate + 1;
            for count in [&mut mic.open, &mut mic.sound_off, &mut mic.hold] {
                if *count == Some(gate) {
                    *count = Some(next);
                }
            }
            effects.closes += 1;
        }
        Event::Replaced(count) => {
            if count > mic.replacements {
                mic.replacements = count;
                effects.device_changed =
                    !mic.muted || mic.sound_off.is_some() || mic.hold.is_some() || mic.refused;
                close(&mut mic, &mut gate);
                mic.sound_off = None;
                mic.hold = None;
                mic.refused = false;
            }
        }
        Event::ForgetSoundOff => mic.sound_off = None,
        Event::ForgetHold => {
            mic.held = false;
            mic.hold = None;
        }
    }
    (mic, effects)
}

impl Microphone {
    /// The state for another gate, which has closed `to` times: the call started or ended.
    /// A permission that holds the count `from` of the gate before holds the new count, and
    /// any other is dropped.
    pub(crate) fn moved(mut self, from: u64, to: u64, replacements: u64) -> Self {
        for count in [&mut self.sound_off, &mut self.hold] {
            *count = count.filter(|count| *count == from).map(|_| to);
        }
        self.open = (!self.muted).then_some(to);
        self.refused = false;
        self.replacements = replacements;
        self
    }
}

/// `None` when the open microphone sends with the count `gate`, which the close advances, so
/// that an automatic open after it holds the new count; otherwise a count before `gate`,
/// which no later count of the gate matches: a close of the engine that the calls have not
/// read came after the open.
fn sending(mic: &Microphone, gate: u64) -> Option<u64> {
    match mic.open {
        Some(open) if open == gate => None,
        open => Some(open.unwrap_or(0).min(gate.saturating_sub(1))),
    }
}

fn open_with(mic: &mut Microphone, permission: u64, gate: u64) {
    if !mic.muted {
        return;
    }
    if permission == gate {
        mic.muted = false;
        mic.open = Some(gate);
    } else {
        mic.refused = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What happens in a call: an event of the calls, a replacement of the input device by
    /// the engine, which closes its gate before the calls read it, and the read.
    #[derive(Debug, Clone, Copy)]
    enum Happens {
        Calls(Event),
        EngineReplaces,
        CallsRead,
    }

    const HAPPENS: [Happens; 12] = [
        Happens::Calls(Event::Mute),
        Happens::Calls(Event::Unmute),
        Happens::Calls(Event::Press),
        Happens::Calls(Event::Release),
        Happens::Calls(Event::SoundOff),
        Happens::Calls(Event::SoundOn),
        Happens::Calls(Event::RuleClose),
        Happens::Calls(Event::Fence),
        Happens::Calls(Event::ForgetSoundOff),
        Happens::Calls(Event::ForgetHold),
        Happens::EngineReplaces,
        Happens::CallsRead,
    ];

    /// Runs `sequence` from an open microphone, as the calls run it: the gate is the one of
    /// the engine, which also closes for a replacement that the calls have not read, and the
    /// calls read the replacements before an open of the person.
    fn run(sequence: &[Happens]) {
        let (mut mic, mut gate, mut replacements) = (Microphone::default(), 0u64, 0u64);
        // A close of the person, of a rule or of the engine came after the last open of the
        // person: from then on only the person opens the microphone.
        let mut closed = false;
        let mut sound_off = false;
        for happens in sequence {
            let event = match *happens {
                Happens::EngineReplaces => {
                    gate += 1;
                    replacements += 1;
                    closed = true;
                    continue;
                }
                Happens::CallsRead => Event::Replaced(replacements),
                Happens::Calls(Event::SoundOff) if sound_off => continue,
                Happens::Calls(Event::SoundOn) if !sound_off => continue,
                Happens::Calls(event) => event,
            };
            if event == Event::Unmute {
                let (read, effects) = step(mic, Event::Replaced(replacements), gate);
                gate += effects.closes;
                mic = read;
            }
            let before = mic;
            let (after, effects) = step(mic, event, gate);
            gate += effects.closes;
            match event {
                Event::Mute | Event::RuleClose => closed = true,
                Event::Unmute => closed = false,
                Event::SoundOff => sound_off = true,
                Event::SoundOn => sound_off = false,
                _ => {}
            }
            let sends = !after.muted && after.open == Some(gate);
            if event == Event::Unmute {
                assert!(sends, "an open of the person did not send: {sequence:?}");
            }
            if matches!(event, Event::Release | Event::SoundOn) && closed {
                assert!(
                    after.muted || !sends,
                    "an automatic open sent after a close: {sequence:?}"
                );
            }
            if let Event::Replaced(_) = event
                && after.replacements != before.replacements
                && (before.sound_off.is_some() || before.hold.is_some() || before.refused)
            {
                assert!(
                    effects.device_changed,
                    "a replacement cancelled an automatic open without the notice: {sequence:?}"
                );
            }
            assert!(after.muted || after.open.is_some(), "{sequence:?}");
            assert!(after.open.is_none_or(|open| open <= gate), "{sequence:?}");
            mic = after;
        }
        // Without a close of the person, of a rule or of the engine, and without a forgotten
        // permission, the microphone sends exactly while the sound is on and no key is held.
        let kept = sequence.iter().all(|happens| {
            matches!(
                happens,
                Happens::Calls(
                    Event::Press | Event::Release | Event::SoundOff | Event::SoundOn | Event::Fence
                ) | Happens::CallsRead
            )
        });
        if kept {
            let sends = !mic.muted && mic.open == Some(gate);
            assert_eq!(sends, !sound_off && !mic.held, "{sequence:?}");
        }
    }

    /// Every sequence of six events: no automatic open sends after a later close, an open of
    /// the person always sends, and the notice of the changed device shows when an automatic
    /// open was due and a replacement cancelled it.
    #[test]
    fn every_sequence_of_six_events_keeps_the_rules_of_the_microphone() {
        let length = 6;
        let mut indexes = vec![0usize; length];
        let mut sequence = vec![HAPPENS[0]; length];
        let mut runs = 0u64;
        loop {
            for (place, index) in indexes.iter().enumerate() {
                sequence[place] = HAPPENS[*index];
            }
            run(&sequence);
            runs += 1;
            let Some(position) = indexes.iter().rposition(|index| *index + 1 < HAPPENS.len())
            else {
                break;
            };
            indexes[position] += 1;
            for index in &mut indexes[position + 1..] {
                *index = 0;
            }
        }
        assert_eq!(runs, (HAPPENS.len() as u64).pow(length as u32));
    }

    #[test]
    fn the_orders_of_the_reviews_keep_the_microphone_closed() {
        // A grant before the close of the hold: the hold takes no permission.
        let (mic, _) = step(Microphone::default(), Event::RuleClose, 0);
        let (mic, _) = step(mic, Event::Press, 1);
        let (mic, _) = step(mic, Event::Release, 1);
        assert!(mic.muted);
        // A replacement after the open of the release and before the engine takes it: the
        // open carries a count of the gate that the replacement passed.
        let (mic, effects) = step(Microphone::default(), Event::Press, 0);
        assert_eq!(effects.closes, 1);
        let (mic, _) = step(mic, Event::Release, 1);
        assert_eq!(mic.open, Some(1));
        let (mic, effects) = step(mic, Event::Replaced(1), 2);
        assert!(mic.muted && effects.device_changed);
        // The sound off before the key, the sound on while it is held, a replacement: the
        // notice shows, and the release opens nothing.
        let (mic, _) = step(Microphone::default(), Event::SoundOff, 0);
        let (mic, _) = step(mic, Event::Press, 1);
        let (mic, _) = step(mic, Event::SoundOn, 1);
        assert_eq!(mic.hold, Some(1));
        let (mic, effects) = step(mic, Event::Replaced(1), 2);
        assert!(effects.device_changed);
        let (mic, _) = step(mic, Event::Release, 2);
        assert!(mic.muted);
        // An open of the person after a replacement that the calls had not read opens.
        let (mic, _) = step(Microphone::default(), Event::Mute, 0);
        let (mic, _) = step(mic, Event::Replaced(1), 2);
        let (mic, _) = step(mic, Event::Unmute, 2);
        assert_eq!(mic.open, Some(2));
        // A release of the key of push to talk keeps the permission of the sound off.
        let (mic, _) = step(Microphone::default(), Event::SoundOff, 0);
        let (mic, _) = step(mic, Event::Fence, 1);
        let (mic, _) = step(mic, Event::SoundOn, 2);
        assert_eq!(mic.open, Some(2));
    }
}
