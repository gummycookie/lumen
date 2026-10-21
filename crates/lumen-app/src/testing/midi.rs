//! What the end-to-end tests may play and see of live MIDI input, the
//! audition and the computer keyboard.

use super::*;

impl App {
    /// Sets MIDI From on the selected MIDI tracks, as the menu would.
    pub fn set_midi_from(&mut self, from: lumen_project::model::MidiFrom) {
        let ids = self.selected_midi_tracks();
        self.document.set_midi_from(&ids, from);
    }

    /// Sets Monitor on the selected MIDI tracks, as the menu would.
    pub fn set_monitor_of_selected(&mut self, monitor: lumen_project::model::Monitor) {
        let ids = self.selected_midi_tracks();
        self.document.set_monitor(&ids, monitor);
    }

    /// Plays `msg` live, as the computer keyboard does (M5 slice 2).
    pub fn play_live(&mut self, msg: lumen_core::midi::Msg) -> bool {
        self.play_key(msg)
    }

    /// Previews `pitch` on `track` as the piano roll does (M5 slice 3d).
    pub fn audition(&mut self, track: Id, pitch: u8, velocity: u8) -> bool {
        self.audition_play(track, pitch, velocity)
    }

    /// Previews a chord of `(pitch, velocity)` notes on `track`, as a press
    /// on picked notes does: each note its own tone, so one on a key the
    /// track's clip is sounding on hardware is skipped and the rest play.
    pub fn audition_chord(&mut self, track: Id, tones: &[(u8, u8)]) -> bool {
        let tones: Vec<crate::audition::Tone> = tones
            .iter()
            .map(|&(pitch, velocity)| crate::audition::Tone { pitch, velocity })
            .collect();
        self.audition_tones(track, &tones, true)
    }

    /// Lets go of the preview (the end of a gesture).
    pub fn release_audition(&mut self) {
        self.audition_release();
    }

    /// Whether a previewed note is sounding.
    pub fn audition_sounding(&self) -> bool {
        self.audition.is_sounding()
    }

    /// Turns the piano roll's audition on or off, in the settings.
    pub fn set_audition_setting(&mut self, on: bool) {
        self.settings.midi.audition = on;
    }

    /// Edits the notes of a clip as the piano roll will (M5 slice 3a's
    /// `Document` methods), for tests of notes committed while playing.
    pub fn edit_notes(&mut self, clip: Id, selection: &[usize], edit: NoteEdit) {
        use crate::document::Resize;
        use lumen_project::edit::notes::End;
        let base = self.document.project().clone();
        match edit {
            NoteEdit::Move { time, pitch } => {
                self.document
                    .move_notes(&base, clip, selection, time, pitch, None);
            }
            NoteEdit::Delete => {
                self.document.delete_notes(clip, selection);
            }
            NoteEdit::Mute(muted) => {
                self.document.mute_notes(clip, selection, Some(muted));
            }
            NoteEdit::Lengthen(by) => {
                let resize = Resize {
                    end: End::End,
                    by,
                    min: Ticks::ZERO,
                };
                self.document
                    .resize_notes(&base, clip, selection, resize, None);
            }
        }
    }

    /// Whether the computer keyboard mode is on (M5 slice 2).
    pub fn computer_keyboard_on(&self) -> bool {
        self.keyboard.is_on()
    }

    /// The notes the computer keyboard holds now, lowest first.
    pub fn computer_keyboard_held(&self) -> Vec<u8> {
        self.keyboard.held_notes()
    }

    /// The messages the computer keyboard sent since the last call.
    pub fn computer_keyboard_sent(&mut self) -> Vec<lumen_core::midi::Msg> {
        self.keyboard.take_sent()
    }

    /// How bright the MIDI chip's IN and OUT lights were when last drawn
    /// (0 at rest, 1 at a flash, fading over `motion::NOTE`).
    pub fn midi_chip_lights(&self) -> (f32, f32) {
        (
            self.transport.midi_lights.input,
            self.transport.midi_lights.output,
        )
    }

    /// How bright the light after a MIDI track's name was when last drawn
    /// (0 at rest, and always 0 while the track does not listen).
    pub fn header_light(&self, ctx: &egui::Context, track: Id) -> f32 {
        crate::arrangement::listen_light_shown(ctx, track)
    }

    /// How bright the popover's light for a port was when last drawn
    /// (`None` when the popover is not showing it).
    pub fn midi_row_light(&self, direction: lumen_midi::Direction, name: &str) -> Option<f32> {
        self.transport
            .midi_lights
            .rows
            .iter()
            .find(|(id, _)| id.direction == direction && id.name == name)
            .map(|(_, b)| *b)
    }

    /// The MIDI switches the settings hold: (direction, name, track, remote).
    pub fn saved_midi_ports(&self) -> Vec<(String, String, bool, bool)> {
        let mut ports: Vec<_> = self
            .settings
            .midi
            .ports
            .iter()
            .map(|p| (p.direction.clone(), p.name.clone(), p.track, p.remote))
            .collect();
        ports.sort();
        ports
    }

    /// The control waiting for a controller to move, to be mapped (M7 slice A5).
    pub fn controller_learning(&self) -> Option<(Id, lumen_project::automation::Target)> {
        self.controllers.learning
    }

    /// Maps controller `number` on `channel` to `track`'s `target`, as learning
    /// it would.
    pub fn give_mapping(
        &mut self,
        track: Id,
        target: lumen_project::automation::Target,
        channel: u8,
        number: u8,
    ) -> Result<(), &'static str> {
        self.document
            .set_mapping(track, target, channel, number)
            .map_err(|refusal| refusal.reason())
    }

    /// Makes `track`'s `target` wait for a controller, as its menu would.
    pub fn begin_learning(&mut self, track: Id, target: lumen_project::automation::Target) {
        self.start_learning(track, target);
    }

    /// Whether the Mappings window is open.
    pub fn mappings_window_open(&self) -> bool {
        self.controllers.window
    }

    /// How many controller messages there was no room for.
    pub fn controllers_dropped(&self) -> u64 {
        self.controllers.dropped()
    }
}
