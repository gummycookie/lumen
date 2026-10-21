//! What the end-to-end tests may see and do in the clip editor and its
//! lane (M5 slices 3c and 4b).

use super::*;

/// The clip editor (M5 slice 3c), for the end-to-end tests.
impl App {
    /// The clip open in the editor.
    pub fn editor_open_clip(&self) -> Option<Id> {
        self.note_editor.clip
    }

    /// The draw tool is chosen (else the pointer).
    pub fn editor_draw_tool(&self) -> bool {
        self.note_editor.tool == crate::note_editor::Tool::Draw
    }

    /// The picked notes, as indices into the open clip's notes.
    pub fn editor_picked(&self) -> Vec<usize> {
        self.note_editor.picked().to_vec()
    }

    /// The editor is in its own window.
    pub fn editor_popped(&self) -> bool {
        self.note_editor.popped
    }

    /// A gesture is in progress (a button is down in the notes).
    pub fn editor_gesture(&self) -> bool {
        self.note_editor.gesture.is_some()
    }

    /// The detail panel shows the Clip tab.
    pub fn clip_tab_shown(&self) -> bool {
        self.detail_tab == crate::app::detail::DetailTab::Clip
    }

    /// Where the notes are drawn, if the editor has been.
    pub fn editor_body(&self) -> Option<egui::Rect> {
        self.note_editor.last_view.map(|v| v.body)
    }

    /// The clip editor's own horizontal view (its zoom and scroll), where it
    /// drew last: separate from [`App::view`], the arrangement's (ADR-0048).
    pub fn editor_view(&self) -> Option<TimeView> {
        self.note_editor.last_view.map(|v| v.time)
    }

    /// The point at the start of clip time `local`, in the row of `pitch`
    /// (the row's middle), where the editor drew it last.
    pub fn editor_point(&self, local: Ticks, pitch: u8) -> Option<egui::Pos2> {
        let v = self.note_editor.last_view?;
        let y = v.row_top(i32::from(pitch)) + v.row_height / 2.0;
        Some(egui::pos2(v.x_of(local), y))
    }

    /// The lane under the roll is showing (it was drawn last frame).
    pub fn lane_shown(&self) -> bool {
        self.note_editor.last_lane.is_some() && !self.note_editor.lane.hidden
    }

    /// The name of the lane's tab chosen ("Velocity", "Mod wheel", ...).
    pub fn lane_name(&self) -> String {
        self.note_editor.lane.kind.name()
    }

    /// The lane's plotting area, where it drew last.
    pub fn lane_body(&self) -> Option<egui::Rect> {
        self.note_editor.last_lane.map(|v| v.body)
    }

    /// The point where the open lane shows `value` at clip time `local`.
    pub fn lane_point(&self, local: Ticks, value: i32) -> Option<egui::Pos2> {
        let v = self.note_editor.last_lane?;
        Some(egui::pos2(v.x_of(local), v.y_of(value)))
    }

    /// Where the bend handle's dot is, when a curve was just drawn.
    pub fn lane_bend_dot(&self) -> Option<egui::Pos2> {
        let v = self.note_editor.last_lane?;
        let b = self.note_editor.lane.bend?;
        Some(crate::note_editor::lane::paint::bend_dot(&v, &b))
    }

    /// A gesture is in progress in the lane.
    pub fn lane_gesture(&self) -> bool {
        self.note_editor.lane.gesture.is_some()
    }

    /// The picked points of the controller lane (indices into the clip's
    /// events).
    pub fn lane_picked(&self) -> Vec<usize> {
        self.note_editor.lane.picked.clone()
    }

    /// Makes `pitch` visible in the editor, as scrolling would.
    pub fn editor_show_pitch(&mut self, pitch: u8) {
        if let Some(v) = self.note_editor.last_view {
            self.note_editor.scroll = crate::note_editor::view::scroll_to_centre(
                i32::from(pitch),
                v.row_height,
                v.body.height(),
            );
        }
    }
}

impl App {
    /// The grid step that `t` falls in now (`[start, end)`).
    pub fn grid_cell_at(&self, t: Ticks) -> (Ticks, Ticks) {
        self.grid_cell(t)
    }

    /// [`Self::grid_cell_at`] as the piano roll's own zoom sets it.
    pub fn editor_grid_cell_at(&self, t: Ticks) -> Option<(Ticks, Ticks)> {
        let zoom = self.note_editor.last_view?.time.zoom;
        Some(self.grid_cell_at_zoom(t, zoom))
    }
}
