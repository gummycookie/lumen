//! Synthetic projects and temporary folders for tests and benchmarks.
//! Not a stable API.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use lumen_core::time::{PPQ, TempoPoint, Ticks};

use crate::model::{
    Clip, ClipContent, Color, Device, DeviceKind, Id, Media, Note, PluginRef, Project,
    TimeSignature, Track, TrackKind,
};

/// A temporary folder removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "lumen-test-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A small deterministic random number generator (xorshift) for tests.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // Xorshift must not start at zero.
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// A small but varied project; different seeds give different content.
pub fn small_project(seed: u64) -> Project {
    synthetic(seed, 6, 4, 8, 64)
}

/// Track 0 is a group containing tracks 1–4; odd tracks are MIDI, even
/// tracks audio. Every other track gets one plugin with `state_bytes` of state.
pub fn synthetic(
    seed: u64,
    tracks: usize,
    clips_per_track: usize,
    notes_per_clip: usize,
    state_bytes: usize,
) -> Project {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut next_id = 0_u64;
    let mut id = || {
        next_id += 1;
        Id(next_id)
    };
    const PALETTE: [u32; 6] = [0xC0504D, 0x4F81BD, 0x9BBB59, 0x8064A2, 0xF79646, 0x4BACC6];
    let group = id();
    let mut media = Vec::new();
    let mut all_tracks = Vec::with_capacity(tracks);
    for index in 0..tracks {
        let kind = match index {
            0 => TrackKind::Group,
            i if i % 2 == 1 => TrackKind::Midi,
            _ => TrackKind::Audio,
        };
        let track_id = if index == 0 { group } else { id() };
        let source = (kind == TrackKind::Audio).then(|| {
            let media_id = id();
            media.push(Media {
                id: media_id,
                path: format!("Samples/Take {index}.wav"),
                original_path: None,
                frames: 480_000,
                channels: 2,
                sample_rate: 48_000,
                size_bytes: 1_920_044,
            });
            media_id
        });
        let devices = if kind != TrackKind::Group {
            // Arguments run in order, so the generator is drawn as before.
            vec![Device::new(
                id(),
                PluginRef::Vst3 {
                    class_id: format!("{:032X}", rng.next()),
                    name: "Synthetic".into(),
                    vendor: "Lumen Tests".into(),
                    kind: DeviceKind::Effect,
                },
                rng.below(4) == 0,
                (0..state_bytes).map(|_| rng.next() as u8).collect(),
            )]
        } else {
            Vec::new()
        };
        let clips = if kind == TrackKind::Group {
            Vec::new()
        } else {
            (0..clips_per_track)
                .map(|k| {
                    Arc::new(Clip {
                        id: id(),
                        name: format!("Clip {k}"),
                        start: Ticks::quarters(4 * k as i64) + Ticks(rng.below(PPQ as u64) as i64),
                        length: Ticks::quarters(2),
                        muted: rng.below(10) == 0,
                        color: None,
                        locked: false,
                        fade_in: Default::default(),
                        fade_out: Default::default(),
                        loop_length: None,
                        content: match source {
                            Some(media) => ClipContent::Audio {
                                media,
                                offset: rng.below(48_000) as i64,
                                gain_db: -(rng.below(240) as f32) / 10.0,
                            },
                            None => ClipContent::Midi {
                                notes: (0..notes_per_clip)
                                    .map(|_| {
                                        Note::new(
                                            Ticks(rng.below(2 * PPQ as u64) as i64),
                                            Ticks(PPQ / 4),
                                            36 + rng.below(48) as u8,
                                            1 + rng.below(127) as u8,
                                        )
                                    })
                                    .collect(),
                                events: Vec::new(),
                            },
                        },
                    })
                })
                .collect()
        };
        all_tracks.push(Arc::new(Track {
            parent: (1..=4).contains(&index).then_some(group),
            gain_db: -(rng.below(120) as f32) / 10.0,
            pan: (rng.below(201) as f32 - 100.0) / 100.0,
            mute: rng.below(8) == 0,
            devices,
            clips,
            ..Track::new(
                track_id,
                format!("Track {index}"),
                kind,
                Color(PALETTE[index % PALETTE.len()]),
            )
        }));
    }
    let master = Arc::new(Track::master(id()));
    Project {
        name: format!("Synthetic {seed}"),
        sample_rate: 48_000,
        tempo: vec![
            TempoPoint {
                at: Ticks::ZERO,
                bpm: 120.0,
            },
            TempoPoint {
                at: Ticks::quarters(64),
                bpm: 128.0,
            },
        ],
        time_signature: TimeSignature {
            numerator: 4,
            denominator: 4,
        },
        tracks: all_tracks,
        master,
        pan_law: Default::default(),
        media,
        markers: Vec::new(),
        transport: Default::default(),
        view: Default::default(),
        mappings: Vec::new(),
    }
}

/// A project of `tracks` at 120 BPM in 4/4, for tests that need only their
/// content: ids from 1000 are free for the caller; the master is 999.
pub fn project_of(name: &str, tracks: Vec<Track>) -> Project {
    Project {
        name: name.into(),
        sample_rate: 48_000,
        tempo: vec![TempoPoint {
            at: Ticks::ZERO,
            bpm: 120.0,
        }],
        time_signature: TimeSignature {
            numerator: 4,
            denominator: 4,
        },
        tracks: tracks.into_iter().map(Arc::new).collect(),
        master: Arc::new(Track::master(Id(999))),
        pan_law: Default::default(),
        media: Vec::new(),
        markers: Vec::new(),
        transport: Default::default(),
        view: Default::default(),
        mappings: Vec::new(),
    }
}

/// A MIDI track with the synth on `preset` and one clip of `notes` and
/// `events` from `start` to `start + length` (in quarter notes), with the
/// content repeating every `repeat` quarters when given. Ids count from
/// `id`: the track, its device, its clip.
pub fn midi_track(
    id: u64,
    name: &str,
    preset: &str,
    (start, length): (i64, i64),
    repeat: Option<i64>,
    notes: Vec<Note>,
    events: Vec<crate::model::ClipEvent>,
) -> Track {
    let mut track = Track::new(Id(id), name, TrackKind::Midi, Color(0x4F81BD));
    track.devices = vec![Device::synth(Id(id + 1), preset)];
    track.clips = vec![Arc::new(Clip {
        id: Id(id + 2),
        name: name.into(),
        start: Ticks::quarters(start),
        length: Ticks::quarters(length),
        muted: false,
        color: None,
        locked: false,
        fade_in: Default::default(),
        fade_out: Default::default(),
        loop_length: repeat.map(Ticks::quarters),
        content: ClipContent::Midi { notes, events },
    })];
    track
}

/// The project for the manual test MT-MIDI-2 (`tests/projects/midi-demo.lumen`):
/// notes cannot be drawn until the piano roll (M5 slice 3), so the project
/// holds what the slice has to prove. 120 BPM, eight bars:
/// - **Chords** (Poly): Am, F, C, G over four bars, repeated by the clip's
///   loop; the C is short with the sustain pedal down for the bar, so it
///   rings on until the pedal comes up;
/// - **Pad** (Pad): one chord for all eight bars, for chase: start playback
///   anywhere and it sounds at once;
/// - **Bass** (Bass): eighth notes on the chords' roots and fifths, four
///   bars repeated;
/// - **Lead** (Keys): a note in bar 5 that bends up two semitones while the
///   modulation wheel opens the filter, then comes back;
/// - **Hardware** (Pluck, sent to Microsoft GS Wavetable Synth, channel 1):
///   an arpeggio of the same chords in bars 1 to 4, in eighth notes, its
///   root on every downbeat.
///
/// The loop brace (bars 1 to 4) is set and off.
pub fn midi_demo() -> Project {
    use crate::model::{ClipEvent, EventKind, MidiTo};
    let q = Ticks::quarters;
    let note = |start: i64, length: i64, pitch: u8, velocity: u8| {
        Note::new(q(start), q(length), pitch, velocity)
    };
    let chord = |bar: i64, length: i64, pitches: &[u8]| -> Vec<Note> {
        pitches
            .iter()
            .map(|&p| note(bar * 4, length, p, 88))
            .collect()
    };
    let pedal = |beat: i64, value: u8| ClipEvent {
        at: q(beat),
        kind: EventKind::Controller { number: 64, value },
    };
    let mut chords = chord(0, 4, &[57, 60, 64]);
    chords.extend(chord(1, 4, &[53, 57, 60]));
    chords.extend(chord(2, 1, &[48, 55, 60, 64]));
    chords.extend(chord(3, 4, &[55, 59, 62]));
    let chords = midi_track(
        1,
        "Chords",
        "poly",
        (0, 32),
        Some(16),
        chords,
        vec![pedal(8, 127), pedal(12, 0)],
    );
    let pad = midi_track(
        10,
        "Pad",
        "pad",
        (0, 32),
        None,
        vec![note(0, 32, 48, 70), note(0, 32, 55, 70)],
        vec![],
    );
    // Roots of Am, F, C, G, one per bar: root, root, fifth, root, twice.
    let bass_line: Vec<Note> = [33_u8, 29, 36, 31]
        .iter()
        .enumerate()
        .flat_map(|(bar, &root)| {
            [0, 0, 7, 0, 0, 0, 7, 0]
                .iter()
                .enumerate()
                .map(move |(i, &up)| {
                    Note::new(
                        Ticks(q(bar as i64 * 4).0 + q(1).0 / 2 * i as i64),
                        Ticks(q(1).0 / 2 - 1_000),
                        root + up,
                        100,
                    )
                })
        })
        .collect();
    let bass = midi_track(20, "Bass", "bass", (0, 32), Some(16), bass_line, vec![]);
    let bend = |beat: i64, sixteenth: i64, value: i16| ClipEvent {
        at: Ticks(q(beat).0 + q(1).0 / 4 * sixteenth),
        kind: EventKind::PitchBend { value },
    };
    let wheel = |beat: i64, value: u8| ClipEvent {
        at: q(beat),
        kind: EventKind::Controller { number: 1, value },
    };
    let lead = midi_track(
        30,
        "Lead",
        "keys",
        (16, 8),
        None,
        vec![note(0, 4, 72, 100), note(4, 4, 67, 90)],
        vec![
            bend(0, 0, 0),
            bend(1, 0, 2048),
            bend(2, 0, 4096),
            bend(3, 0, 8191),
            bend(4, 0, 0),
            wheel(0, 0),
            wheel(2, 64),
            wheel(4, 127),
            wheel(6, 0),
        ],
    );
    // The chords' notes again, one bar each, eighth notes going up and
    // back: the root (the chord's lowest note) on every downbeat.
    let arpeggio: Vec<Note> = [
        [57_u8, 60, 64, 69],
        [53, 57, 60, 65],
        [60, 64, 67, 72],
        [55, 59, 62, 67],
    ]
    .iter()
    .enumerate()
    .flat_map(|(bar, tones)| {
        [0_usize, 1, 2, 3, 2, 1, 2, 1]
            .into_iter()
            .enumerate()
            .map(move |(i, tone)| {
                Note::new(
                    Ticks(q(bar as i64 * 4).0 + q(1).0 / 2 * i as i64),
                    Ticks(q(1).0 / 2 - 2_000),
                    tones[tone],
                    96,
                )
            })
    })
    .collect();
    let mut hardware = midi_track(40, "Hardware", "pluck", (0, 16), None, arpeggio, vec![]);
    hardware.midi_to = Some(MidiTo {
        port: "Microsoft GS Wavetable Synth".into(),
        channel: 0,
    });
    let mut project = project_of("MIDI demo", vec![chords, pad, bass, lead, hardware]);
    project.transport.loop_start = Ticks::ZERO;
    project.transport.loop_end = q(16);
    project.transport.loop_enabled = false;
    project
}

/// The project for the manual test MT-AUTO-1 (`tests/projects/automation-demo.lumen`):
/// there is no lane interface until M7 slice A1, so the project holds what
/// slice A0 has to prove, by ear. 120 BPM, eight bars, three tracks of
/// [`midi_demo`]:
/// - **Pad** swells in from near silence over two bars (a fader lane in dB),
///   holds, and falls 18 dB over the last two;
/// - **Chords** sweeps from the left to the right and back, every two bars
///   (a pan lane);
/// - **Bass** drops out for the third bar and the seventh (a mute lane of
///   steps).
pub fn automation_demo() -> Project {
    use crate::automation::{AutomationLane, Target};
    use lumen_core::automation::{Lane, Point, Shape};
    let q = Ticks::quarters;
    let lane = |target: Target, points: &[(i64, f32, Shape)]| {
        AutomationLane::new(
            target,
            Lane::new(
                points
                    .iter()
                    .map(|&(beat, value, shape)| Point::new(q(beat), value).with_shape(shape))
                    .collect(),
            )
            .expect("the demo's lanes are in time order"),
        )
    };
    let mut project = midi_demo();
    project.name = "Automation demo".into();
    project
        .tracks
        .retain(|t| ["Chords", "Pad", "Bass"].contains(&t.name.as_str()));
    for track in &mut project.tracks {
        let track = Arc::make_mut(track);
        track.automation = match track.name.as_str() {
            "Pad" => vec![lane(
                Target::Volume,
                &[
                    (0, -60.0, Shape::Linear),
                    (8, 0.0, Shape::Linear),
                    (24, 0.0, Shape::Linear),
                    (32, -18.0, Shape::Linear),
                ],
            )],
            "Chords" => vec![lane(
                Target::Pan,
                &[
                    (0, -1.0, Shape::Linear),
                    (8, 1.0, Shape::Linear),
                    (16, -1.0, Shape::Linear),
                    (24, 1.0, Shape::Linear),
                    (32, -1.0, Shape::Linear),
                ],
            )],
            _ => vec![lane(
                Target::Mute,
                &[
                    (0, 0.0, Shape::Step),
                    (8, 1.0, Shape::Step),
                    (12, 0.0, Shape::Step),
                    (24, 1.0, Shape::Step),
                    (28, 0.0, Shape::Step),
                ],
            )],
        };
    }
    project
}
