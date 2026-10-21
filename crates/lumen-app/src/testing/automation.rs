//! What the end-to-end tests may see and do with automation lanes (M7).

use super::*;

impl App {
    /// Gives track `track` a lane for `target` (M7 slice A0: the lane
    /// interface arrives in slice A1, so end-to-end tests make lanes here).
    /// Returns why not when the lane is refused.
    pub fn give_lane(
        &mut self,
        track: Id,
        target: lumen_project::automation::Target,
        lane: lumen_core::automation::Lane,
    ) -> Result<(), &'static str> {
        self.document
            .set_lane(track, target, lane, false)
            .map_err(|refusal| refusal.reason())
    }

    /// Switches a lane on or off, as the lane's switch in slice A4 will.
    pub fn switch_lane(
        &mut self,
        track: Id,
        target: lumen_project::automation::Target,
        on: bool,
    ) -> Result<(), &'static str> {
        self.document
            .set_lane_enabled(track, target, on)
            .map_err(|refusal| refusal.reason())
    }

    /// Whether the user has taken `track`'s lane for `target` over by moving
    /// the control it drives.
    pub fn lane_taken_over(&self, track: Id, target: lumen_project::automation::Target) -> bool {
        self.document.overrides().contains(track, target)
    }

    /// Sets what a track does with the controls that move while the song plays:
    /// "read", "touch", "latch" or "write".
    pub fn set_automation_mode(&mut self, track: Id, mode: &str) {
        use crate::auto_record::Mode;
        let mode = Mode::ALL
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(mode))
            .expect("a mode: read, touch, latch or write");
        self.arrangement.auto.set_mode(track, mode);
    }

    /// What a track does with the controls that move while the song plays.
    pub fn automation_mode(&self, track: Id) -> &'static str {
        self.arrangement.auto.mode_of(track).name()
    }

    /// Whether the controls and the chip show a lane as overridden: taken over
    /// and not being written.
    pub fn shows_lane_overridden(
        &self,
        track: Id,
        target: lumen_project::automation::Target,
    ) -> bool {
        lumen_project::routing::all_tracks(self.document.project())
            .find(|t| t.id == track)
            .is_some_and(|t| self.arrangement.taken_over(&self.document, t, target))
    }

    /// Whether a lane is being written now.
    pub fn recording_automation(
        &self,
        track: Id,
        target: lumen_project::automation::Target,
    ) -> bool {
        self.arrangement.auto.recording.contains(&(track, target))
    }

    /// Takes a lane over, as moving the control it drives does.
    pub fn take_over_lane(&mut self, track: Id, target: lumen_project::automation::Target) {
        self.document.override_lane(track, target);
    }

    /// Lets a lane taken over play again, as the chip's Re-enable does.
    pub fn reenable_lane(&mut self, track: Id, target: lumen_project::automation::Target) {
        self.document.reenable_lane(track, target);
    }

    /// Takes a lane away, as the lane chip's menu does.
    pub fn take_lane_away(
        &mut self,
        track: Id,
        target: lumen_project::automation::Target,
    ) -> Result<(), &'static str> {
        self.document
            .remove_lane(track, target)
            .map_err(|refusal| refusal.reason())
    }

    /// The parameter `track`'s shown lane draws, if it shows one (M7 slice A1).
    pub fn automation_shown(&self, track: Id) -> Option<lumen_project::automation::Target> {
        self.arrangement.auto.shown.get(&track).copied()
    }

    /// Where `track`'s shown lane is on screen: its whole row.
    pub fn automation_row(&self, ctx: &egui::Context, track: Id) -> Option<egui::Rect> {
        self.lane_frame_of(ctx, track).map(|(_, frame)| frame.row)
    }

    /// Where a point at `quarters` of `value` would be drawn on `track`'s
    /// shown lane, for pressing there.
    pub fn automation_spot(
        &self,
        ctx: &egui::Context,
        track: Id,
        quarters: f64,
        value: f32,
    ) -> Option<egui::Pos2> {
        let (target, frame) = self.lane_frame_of(ctx, track)?;
        Some(egui::pos2(
            frame.x(crate::ruler::ticks_at(quarters)),
            frame.y(target, value),
        ))
    }

    /// Where point `index` of `track`'s shown lane is drawn.
    pub fn automation_point_pos(
        &self,
        ctx: &egui::Context,
        track: Id,
        index: usize,
    ) -> Option<egui::Pos2> {
        let (target, frame) = self.lane_frame_of(ctx, track)?;
        let held = self
            .document
            .project()
            .tracks
            .iter()
            .find(|t| t.id == track)?;
        let point = held.lane(target)?.lane.points().get(index).copied()?;
        Some(frame.pos(target, &point))
    }

    /// The points picked on a lane: its track and their places in the lane.
    pub fn automation_picked(&self) -> Option<(Id, Vec<usize>)> {
        self.arrangement
            .auto
            .picks
            .as_ref()
            .map(|p| (p.track, p.list()))
    }

    /// The live value drawn beside the lane gesture under way, if any.
    pub fn automation_tip(&self) -> Option<String> {
        self.arrangement.auto.tip.clone()
    }

    /// The tool the lanes have in hand: "Pointer", "Pencil" or a shape's name.
    pub fn automation_tool(&self) -> String {
        use crate::arrangement::auto_state::{Tool, wave_name};
        match self.arrangement.auto.tool {
            Tool::Pointer => "Pointer".to_owned(),
            Tool::Pencil => "Pencil".to_owned(),
            Tool::Shape(wave) => wave_name(wave).to_owned(),
        }
    }

    /// Where the dot that bends the segment leaving point `index` of
    /// `track`'s shown lane is drawn, if it has one.
    pub fn automation_bend_pos(
        &self,
        ctx: &egui::Context,
        track: Id,
        index: usize,
    ) -> Option<egui::Pos2> {
        let (target, frame) = self.lane_frame_of(ctx, track)?;
        let held = self
            .document
            .project()
            .tracks
            .iter()
            .find(|t| t.id == track)?;
        frame.bend_dot(target, &held.lane(target)?.lane, index)
    }
}
