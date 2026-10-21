//! The velocity and controller lane under the piano roll (M5 slice 4b;
//! owner's V A, L A, T A, R B).
//!
//! - `geometry`: where values and points are on the screen;
//! - `gesture`: the pointer gestures as pure arithmetic;
//! - `paint`: how the lane looks;
//! - `ui`: the header (lane tabs and tools), the pointer, and committing a
//!   gesture to the document.
//!
//! One lane shows at a time, switched by tabs. Like the notes, a gesture
//! only previews while the button is down; the document changes once, on
//! release, as one undo step.

use std::sync::Arc;

use lumen_core::time::Ticks;
use lumen_project::edit::velocity;
use lumen_project::model::{Clip, ClipContent};

pub(crate) mod geometry;
pub(crate) mod gesture;
pub(crate) mod paint;
mod ui;

pub(in crate::note_editor) use ui::{LaneInput, reserved};

use geometry::LaneKind;
use gesture::{LaneGesture, LaneTool};
use paint::{BendHandle, LanePoint};

/// The lane's state, part of the editor's.
#[derive(Default)]
pub(crate) struct LaneState {
    /// The lane is switched off (the editor's "Lane" button).
    pub hidden: bool,
    pub kind: LaneKind,
    pub tool: LaneTool,
    /// Controllers added with "+ CC", shown as tabs for this session.
    pub extra: Vec<u8>,
    pub gesture: Option<LaneGesture>,
    /// The picked events of a controller lane (indices into the clip's
    /// events), valid for `picked_for` only.
    pub picked: Vec<usize>,
    picked_for: Option<Arc<Clip>>,
    /// The line just drawn with the Curve tool, bendable until anything
    /// else changes.
    pub bend: Option<BendHandle>,
    bend_for: Option<Arc<Clip>>,
    /// The clip the keyboard's Delete belongs to: the lane's picked points
    /// after a press in the lane, the roll's notes after a press there.
    pub focus: bool,
    cache: Option<Cache>,
}

/// What the lane draws, rebuilt when the clip or the lane changes.
struct Cache {
    clip: Arc<Clip>,
    kind: LaneKind,
    stems: Vec<(Ticks, Vec<usize>)>,
    points: Vec<LanePoint>,
}

/// The events of a MIDI clip.
pub(crate) fn events_of(clip: &Clip) -> &[lumen_project::model::ClipEvent] {
    match &clip.content {
        ClipContent::Midi { events, .. } => events,
        _ => &[],
    }
}

impl LaneState {
    /// Brings the cache and the picks up to date with `clip`: picks and the
    /// bend handle belong to one version of the clip and go with it.
    pub fn sync(&mut self, clip: &Arc<Clip>) {
        let same = |c: &Option<Arc<Clip>>| c.as_ref().is_some_and(|c| Arc::ptr_eq(c, clip));
        if !same(&self.picked_for) {
            self.picked.clear();
            self.picked_for = Some(clip.clone());
        }
        if !same(&self.bend_for) {
            self.bend = None;
            self.bend_for = Some(clip.clone());
        }
        let fresh = self
            .cache
            .as_ref()
            .is_some_and(|c| Arc::ptr_eq(&c.clip, clip) && c.kind == self.kind);
        if !fresh {
            let notes = super::notes_of(clip);
            // Every note and event, those outside the clip's window too: the
            // editor shows the clip's whole content (ADR-0059).
            let stems = if self.kind == LaneKind::Velocity {
                velocity::stems(notes)
            } else {
                Vec::new()
            };
            let points = match self.kind.lane() {
                Some(lane) => events_of(clip)
                    .iter()
                    .enumerate()
                    .filter_map(|(index, e)| {
                        lane.value_of(e.kind).map(|value| LanePoint {
                            at: e.at,
                            value,
                            index,
                        })
                    })
                    .collect(),
                None => Vec::new(),
            };
            self.cache = Some(Cache {
                clip: clip.clone(),
                kind: self.kind,
                stems,
                points,
            });
        }
    }

    /// After an edit the lane made: the picks and the bend handle now belong
    /// to `clip`, the clip as the edit left it.
    pub fn rebind(&mut self, clip: &Arc<Clip>, picked: Vec<usize>, bend: Option<BendHandle>) {
        self.picked = picked;
        self.picked_for = Some(clip.clone());
        self.bend = bend;
        self.bend_for = Some(clip.clone());
    }

    /// Shows another lane: what belonged to the old one is dropped.
    pub fn choose(&mut self, kind: LaneKind) {
        self.kind = kind;
        self.gesture = None;
        self.bend = None;
        self.picked.clear();
    }

    /// Uses another tool: a curve just drawn can no longer be bent.
    pub fn use_tool(&mut self, tool: LaneTool) {
        self.tool = tool;
        self.bend = None;
    }

    /// Ends a gesture without changing anything. True if one was running.
    pub fn cancel_gesture(&mut self) -> bool {
        self.gesture.take().is_some()
    }
}
